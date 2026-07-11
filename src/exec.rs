//! The command-execution seam for remote-aware operations (issue #62).
//!
//! akit resolves a project's Git root and probes harness binaries/versions by
//! running short commands. Locally these run through [`LocalRunner`]; an
//! embedding host (madari) implements [`CommandRunner`] over its existing
//! SSH/ControlMaster channel so the *same* logic probes the **remote** host —
//! akit never learns to invoke SSH itself, and never infers remote capabilities
//! from the local machine.
//!
//! The seam is deliberately tiny (one `run`) and text-oriented: callers pass an
//! explicit `program` + `args` (never a shell string), so nothing here composes
//! or interprets a shell, and credentials are never embedded in commands.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// Captured result of a single command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Process exit code (`None` maps to `-1` for signal-terminated locals).
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    /// Whether the command exited `0`.
    pub fn success(&self) -> bool {
        self.status == 0
    }

    /// Trimmed stdout, the common case for single-line probes.
    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim()
    }
}

/// Runs short, non-interactive commands for capability discovery. Implemented by
/// [`LocalRunner`] here and by the embedding host over SSH for remote projects.
///
/// Implementations MUST NOT interpret `program`/`args` through a shell; they are
/// an argv vector executed directly on the target host.
pub trait CommandRunner {
    /// Run `program args...` with an optional working directory, capturing
    /// stdout/stderr. A non-zero exit is returned as `Ok` with the status; only
    /// a failure to *spawn* (or transport error) is an `Err`.
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Result<CommandOutput>;
}

/// The local command runner, backed by [`std::process::Command`].
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalRunner;

impl CommandRunner for LocalRunner {
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Result<CommandOutput> {
        let mut command = Command::new(program);
        command.args(args);
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        let output = command
            .output()
            .with_context(|| format!("spawning `{program}`"))?;
        Ok(CommandOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Resolve a project root by asking Git for the working-tree top level, falling
/// back to `cwd` itself when the directory is not inside a Git work tree (or Git
/// is unavailable). This mirrors the local [`crate::project::Project`] discovery
/// but works over any [`CommandRunner`], so a remote pane's root resolves on the
/// remote host via `git -C <cwd> rev-parse --show-toplevel`.
pub fn resolve_project_root(runner: &dyn CommandRunner, cwd: &Path) -> Result<PathBuf> {
    let cwd_str = cwd.to_string_lossy();
    let out = runner.run(
        "git",
        &["-C", &cwd_str, "rev-parse", "--show-toplevel"],
        Some(cwd),
    )?;
    if out.success() {
        let top = out.stdout_trimmed();
        if !top.is_empty() {
            return Ok(PathBuf::from(top));
        }
    }
    // Not a Git work tree (or git missing): the pane's cwd is the project root.
    Ok(cwd.to_path_buf())
}

/// The result of probing one harness binary on a target host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessProbe {
    pub harness: crate::harness::HarnessId,
    /// The binary name that was probed (defaults to the harness id).
    pub binary: String,
    /// Whether the binary was found and reported a version.
    pub present: bool,
    /// The raw version string the binary reported, if any.
    pub version: Option<String>,
}

/// The conventional CLI binary name for a harness. Kept deliberately simple:
/// each of the five harnesses ships a binary matching its id.
pub fn harness_binary(harness: crate::harness::HarnessId) -> &'static str {
    harness.as_str()
}

/// Probe a harness on the target host by running `<binary> --version`, capturing
/// presence and the reported version. The probe runs through the supplied
/// [`CommandRunner`], so a remote runner reports the **remote** host's binary and
/// version — never the local installation.
pub fn probe_harness(
    runner: &dyn CommandRunner,
    harness: crate::harness::HarnessId,
) -> Result<HarnessProbe> {
    let binary = harness_binary(harness);
    // A spawn failure (binary absent) is a clean "not present", not an error.
    let out = match runner.run(binary, &["--version"], None) {
        Ok(out) => out,
        Err(_) => {
            return Ok(HarnessProbe {
                harness,
                binary: binary.to_string(),
                present: false,
                version: None,
            });
        }
    };
    let version = if out.success() {
        let v = out.stdout_trimmed();
        (!v.is_empty()).then(|| v.to_string())
    } else {
        None
    };
    Ok(HarnessProbe {
        harness,
        binary: binary.to_string(),
        present: version.is_some(),
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted runner: matches on the program+args and returns canned output.
    struct ScriptRunner {
        top_level: Option<String>,
    }

    impl CommandRunner for ScriptRunner {
        fn run(&self, program: &str, args: &[&str], _cwd: Option<&Path>) -> Result<CommandOutput> {
            assert_eq!(program, "git");
            assert!(args.contains(&"rev-parse"));
            match &self.top_level {
                Some(top) => Ok(CommandOutput {
                    status: 0,
                    stdout: format!("{top}\n"),
                    stderr: String::new(),
                }),
                None => Ok(CommandOutput {
                    status: 128,
                    stdout: String::new(),
                    stderr: "fatal: not a git repository".to_string(),
                }),
            }
        }
    }

    #[test]
    fn resolve_root_uses_git_top_level() {
        let runner = ScriptRunner {
            top_level: Some("/home/u/proj".to_string()),
        };
        let root = resolve_project_root(&runner, Path::new("/home/u/proj/sub")).unwrap();
        assert_eq!(root, PathBuf::from("/home/u/proj"));
    }

    #[test]
    fn resolve_root_falls_back_to_cwd_outside_git() {
        let runner = ScriptRunner { top_level: None };
        let cwd = Path::new("/tmp/loose");
        let root = resolve_project_root(&runner, cwd).unwrap();
        assert_eq!(root, cwd.to_path_buf());
    }

    #[test]
    fn local_runner_captures_output() {
        let runner = LocalRunner;
        let out = runner.run("echo", &["hello"], None).unwrap();
        assert!(out.success());
        assert_eq!(out.stdout_trimmed(), "hello");
    }

    /// A runner that answers `--version` for a chosen binary and fails to spawn
    /// everything else, so we can probe presence deterministically.
    struct ProbeRunner {
        present_binary: Option<&'static str>,
        version: &'static str,
    }

    impl CommandRunner for ProbeRunner {
        fn run(&self, program: &str, args: &[&str], _cwd: Option<&Path>) -> Result<CommandOutput> {
            assert_eq!(args, &["--version"]);
            match self.present_binary {
                Some(b) if b == program => Ok(CommandOutput {
                    status: 0,
                    stdout: format!("{}\n", self.version),
                    stderr: String::new(),
                }),
                _ => anyhow::bail!("no such binary: {program}"),
            }
        }
    }

    #[test]
    fn probe_reports_present_version() {
        use crate::harness::HarnessId;
        let runner = ProbeRunner {
            present_binary: Some(HarnessId::Claude.as_str()),
            version: "claude 1.2.3",
        };
        let probe = probe_harness(&runner, HarnessId::Claude).unwrap();
        assert!(probe.present);
        assert_eq!(probe.version.as_deref(), Some("claude 1.2.3"));
        assert_eq!(probe.binary, HarnessId::Claude.as_str());
    }

    #[test]
    fn probe_reports_absent_when_binary_missing() {
        use crate::harness::HarnessId;
        let runner = ProbeRunner {
            present_binary: None,
            version: "",
        };
        let probe = probe_harness(&runner, HarnessId::Codex).unwrap();
        assert!(!probe.present);
        assert_eq!(probe.version, None);
    }
}
