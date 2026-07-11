//! Per-host harness capability verification (issue #62).
//!
//! Combines a **live** binary + version probe on the target host (via the
//! [`crate::exec`] seam) with akit's **static** registry capability facts to
//! decide whether a harness is actually supported on a specific host/version.
//! No model/LLM is involved: "verified" means the binary is present, any known
//! version gate is satisfied, and akit statically supports at least one
//! primitive (skill or agent) for the harness.
//!
//! An embedding host (madari) runs this against a remote host through its SSH
//! [`CommandRunner`] and enables kit support for that host **only** once the
//! outcome is `verified` — remote capability is never inferred from the local
//! machine.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::exec::{CommandRunner, probe_harness};
use crate::harness::{self, HarnessId};

/// Capability of one harness on a specific host, combining a live probe with the
/// static registry facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostVerification {
    pub harness: HarnessId,
    /// Opaque host identity supplied by the caller (e.g. `user@host`); `local`
    /// for the local machine. A verification is scoped to this + the version.
    pub host_key: String,
    /// The binary name that was probed.
    pub binary: String,
    /// Whether the binary was found and reported a version.
    pub present: bool,
    /// The raw version string the binary reported, if any.
    pub version: Option<String>,
    /// The agent-target min-version gate for this harness, if one is known.
    pub min_version: Option<String>,
    /// Whether the reported version satisfies the gate (always true when there
    /// is no gate; false when a gate exists but no version could be read).
    pub version_ok: bool,
    /// Whether akit statically supports skills for this harness.
    pub skill_supported: bool,
    /// Whether akit statically supports agents for this harness on this version.
    pub agent_supported: bool,
    /// Final decision: `present && (skill_supported || agent_supported)`.
    pub verified: bool,
    /// Human-readable explanation of the decision.
    pub detail: String,
}

/// Verify a single harness on a host identified by `host_key`.
pub fn verify_harness(
    runner: &dyn CommandRunner,
    harness: HarnessId,
    host_key: &str,
) -> Result<HostVerification> {
    let probe = probe_harness(runner, harness)?;
    let extracted = probe.version.as_deref().and_then(extract_version);

    // Skills have no version gate; agents may. Gate each primitive separately so
    // an unmet agent gate never suppresses otherwise-working skill support.
    let skill_supported = skill_supported(harness);

    let agent_target = harness::agent_target(harness);
    let agent_base = agent_target.is_enabled();
    let min_version = if agent_base {
        agent_target.min_version.map(str::to_string)
    } else {
        None
    };
    let version_ok = match (&min_version, &extracted) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(want), Some(have)) => version_ge(have, want),
    };
    let agent_supported = agent_base && version_ok;

    let verified = probe.present && (skill_supported || agent_supported);
    let detail = describe(
        harness,
        host_key,
        &probe.binary,
        probe.present,
        &min_version,
        version_ok,
        skill_supported,
        agent_supported,
    );

    Ok(HostVerification {
        harness,
        host_key: host_key.to_string(),
        binary: probe.binary,
        present: probe.present,
        version: probe.version,
        min_version,
        version_ok,
        skill_supported,
        agent_supported,
        verified,
        detail,
    })
}

/// Verify every registry harness on a host, in registry order.
pub fn verify_all(runner: &dyn CommandRunner, host_key: &str) -> Result<Vec<HostVerification>> {
    HarnessId::ALL
        .iter()
        .map(|&h| verify_harness(runner, h, host_key))
        .collect()
}

/// Whether akit statically supports skills for `harness` (some registered path
/// covers it with sufficient evidence).
fn skill_supported(harness: HarnessId) -> bool {
    harness::skill_paths()
        .iter()
        .any(|p| p.covers(harness) && p.evidence.is_sufficient())
}

#[allow(clippy::too_many_arguments)]
fn describe(
    harness: HarnessId,
    host_key: &str,
    binary: &str,
    present: bool,
    min_version: &Option<String>,
    version_ok: bool,
    skill_supported: bool,
    agent_supported: bool,
) -> String {
    let label = harness.label();
    if !present {
        return format!("{label}: `{binary}` not found on {host_key}");
    }
    if let Some(min) = min_version
        && !version_ok
    {
        return format!("{label} on {host_key} is below the required version {min}");
    }
    if !skill_supported && !agent_supported {
        return format!("{label}: no verified skill or agent target");
    }
    let mut prims = Vec::new();
    if skill_supported {
        prims.push("skills");
    }
    if agent_supported {
        prims.push("agents");
    }
    format!("{label} verified on {host_key} ({})", prims.join(" + "))
}

/// Extract the first dotted-numeric version token from a `--version` line, e.g.
/// `"claude 1.2.3"` → `1.2.3`, `"gh version 2.23.0 (…)"` → `2.23.0`,
/// `"1.0.70-0"` → `1.0.70`.
pub fn extract_version(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let tok = raw[start..i].trim_end_matches('.');
            if !tok.is_empty() {
                return Some(tok.to_string());
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Whether dotted-numeric `have >= want`, comparing component-by-component with
/// missing trailing components treated as `0` (so `1.2 >= 1.2.0`).
pub fn version_ge(have: &str, want: &str) -> bool {
    let hp = parts(have);
    let wp = parts(want);
    for i in 0..hp.len().max(wp.len()) {
        let h = hp.get(i).copied().unwrap_or(0);
        let w = wp.get(i).copied().unwrap_or(0);
        if h != w {
            return h > w;
        }
    }
    true
}

fn parts(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::{CommandOutput, CommandRunner};
    use std::path::Path;

    struct FakeRunner {
        present: Vec<&'static str>,
        version: &'static str,
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str], _cwd: Option<&Path>) -> Result<CommandOutput> {
            assert_eq!(args, &["--version"]);
            if self.present.contains(&program) {
                Ok(CommandOutput {
                    status: 0,
                    stdout: format!("{program} {}\n", self.version),
                    stderr: String::new(),
                })
            } else {
                anyhow::bail!("not found: {program}")
            }
        }
    }

    #[test]
    fn extract_and_compare_versions() {
        assert_eq!(extract_version("claude 1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(extract_version("v2.23.0 (2020)").as_deref(), Some("2.23.0"));
        assert_eq!(extract_version("1.0.70-0").as_deref(), Some("1.0.70"));
        assert_eq!(extract_version("no digits here"), None);
        assert!(version_ge("2.23.0", "2.23"));
        assert!(version_ge("1.3", "1.2.9"));
        assert!(!version_ge("1.2.0", "1.2.1"));
        assert!(version_ge("1.2", "1.2.0"));
    }

    #[test]
    fn present_harness_with_static_support_is_verified() {
        let runner = FakeRunner {
            present: vec![HarnessId::Claude.as_str()],
            version: "1.2.3",
        };
        let v = verify_harness(&runner, HarnessId::Claude, "user@host").unwrap();
        assert!(v.present);
        assert!(v.verified, "{}", v.detail);
        assert_eq!(v.host_key, "user@host");
        assert_eq!(v.version.as_deref(), Some("claude 1.2.3"));
    }

    #[test]
    fn absent_harness_is_not_verified() {
        let runner = FakeRunner {
            present: vec![],
            version: "1.0",
        };
        let v = verify_harness(&runner, HarnessId::Codex, "user@host").unwrap();
        assert!(!v.present);
        assert!(!v.verified);
        assert!(v.detail.contains("not found"));
    }

    #[test]
    fn verify_all_covers_every_registry_harness() {
        let runner = FakeRunner {
            present: vec![],
            version: "",
        };
        let all = verify_all(&runner, "local").unwrap();
        assert_eq!(all.len(), HarnessId::ALL.len());
        assert!(all.iter().all(|v| !v.verified));
    }
}
