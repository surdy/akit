//! The harness capability registry (issue #33).
//!
//! This module is the single source of truth for *how each supported CLI
//! harness discovers project-level customizations*. Everything downstream — the
//! install planner (#32), materialization (#31), and the CLI/embedding surface
//! (#34) — reads its target paths, coverage, symlink safety, minimum versions,
//! and reload guidance from here rather than hardcoding `.github/...` paths.
//!
//! ## Design contract
//!
//! - **Skills** are portable `SKILL.md` directories. Several harnesses read the
//!   *same* project directory (notably `.agents/skills` and `.claude/skills`),
//!   so a single materialization can serve multiple harnesses. Each
//!   [`SkillPath`] records exactly which harnesses cover it.
//! - **Custom agents** have *no* shared path: every harness uses a proprietary
//!   directory and file format, so an agent must be materialized once per
//!   selected harness from an explicit native variant. Each [`AgentTarget`]
//!   records that native destination.
//! - A capability is only **enabled** when its discovery behavior is backed by
//!   [`Evidence`] we trust (official docs, official source, or an isolated live
//!   behavioral test). Ambiguous behavior (e.g. OpenCode's `agent/` vs
//!   `agents/` directory) is marked [`AgentTarget::needs_probe`] so the caller
//!   resolves it against the installed version rather than guessing.
//! - **Symlink** discovery is only claimed where verified; every other target
//!   materializes as a copy (see [`SkillPath::symlink_verified`] /
//!   [`AgentTarget::symlink_verified`]).
//!
//! The matrix below is derived from the July-2026 official-source audit; see
//! `docs/harness-registry.md` for the per-cell citations and verification
//! evidence.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A supported CLI harness. This is the canonical, stable identifier used
/// across the wire (lockfile, embedding API, CLI flags). Unknown/future ids are
/// rejected by [`HarnessId::from_str`] rather than silently ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessId {
    Copilot,
    Claude,
    Codex,
    Gemini,
    Opencode,
}

impl HarnessId {
    /// Every supported harness, in stable registry order. This order is
    /// load-bearing: the planner uses it as the deterministic tie-breaker when
    /// several skill destinations cover the same number of harnesses.
    pub const ALL: [HarnessId; 5] = [
        HarnessId::Copilot,
        HarnessId::Claude,
        HarnessId::Codex,
        HarnessId::Gemini,
        HarnessId::Opencode,
    ];

    /// The lowercase wire token (`"copilot"`, `"claude"`, …).
    pub const fn as_str(self) -> &'static str {
        match self {
            HarnessId::Copilot => "copilot",
            HarnessId::Claude => "claude",
            HarnessId::Codex => "codex",
            HarnessId::Gemini => "gemini",
            HarnessId::Opencode => "opencode",
        }
    }

    /// Human-facing label for pickers, plans, and messages.
    pub const fn label(self) -> &'static str {
        match self {
            HarnessId::Copilot => "GitHub Copilot CLI",
            HarnessId::Claude => "Claude Code",
            HarnessId::Codex => "OpenAI Codex CLI",
            HarnessId::Gemini => "Gemini CLI",
            HarnessId::Opencode => "OpenCode",
        }
    }
}

impl fmt::Display for HarnessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unknown/unsupported harness id. Carries the
/// supported list so the CLI/embedding surface can render an actionable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownHarness {
    /// The token that failed to parse.
    pub token: String,
}

impl fmt::Display for UnknownHarness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let supported = HarnessId::ALL
            .iter()
            .map(|h| h.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            f,
            "unknown harness '{}' (supported: {supported})",
            self.token
        )
    }
}

impl std::error::Error for UnknownHarness {}

impl FromStr for HarnessId {
    type Err = UnknownHarness;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "copilot" => Ok(HarnessId::Copilot),
            "claude" => Ok(HarnessId::Claude),
            "codex" => Ok(HarnessId::Codex),
            "gemini" => Ok(HarnessId::Gemini),
            "opencode" => Ok(HarnessId::Opencode),
            _ => Err(UnknownHarness {
                token: s.to_string(),
            }),
        }
    }
}

/// The two kinds of customization the registry describes. MCP servers are a
/// deliberate later phase and are not part of this enum yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Primitive {
    Skill,
    Agent,
}

/// The evidence backing a capability entry. A capability is only *enabled* when
/// its discovery behavior is proven; unproven behavior is disabled with a
/// reason rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Evidence {
    /// Documented in the vendor's official documentation.
    OfficialDocs,
    /// Proven by reading the harness's official open-source implementation.
    OfficialSource,
    /// Proven by an isolated temporary-project behavioral test.
    LiveVerified,
    /// Not yet proven; the capability must be treated as disabled/conservative.
    Unverified,
}

impl Evidence {
    /// Whether this evidence level is sufficient to *enable* a capability.
    /// Everything except [`Evidence::Unverified`] enables the target.
    pub const fn is_sufficient(self) -> bool {
        !matches!(self, Evidence::Unverified)
    }
}

/// How a harness picks up a newly materialized customization within a running
/// session. Drives the exact post-install guidance the UI shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reload {
    /// Detected automatically while the session is running (file-watched).
    Live,
    /// Requires an explicit in-session reload command (e.g. `/skills reload`).
    Command,
    /// Requires restarting the harness/starting a new session.
    Restart,
    /// Reload behavior is not documented; treat conservatively (assume restart).
    Unknown,
}

impl Reload {
    /// Concise, harness-agnostic guidance string for post-install messaging.
    /// Callers may prepend the harness label and primitive.
    pub const fn guidance(self) -> &'static str {
        match self {
            Reload::Live => "picked up automatically; no restart needed",
            Reload::Command => "run the harness's reload command to pick it up this session",
            Reload::Restart => "restart the harness to load it",
            Reload::Unknown => "restart the harness if it does not appear",
        }
    }
}

/// A project-level directory family that one or more harnesses scan for skills.
///
/// The `covers` list is the crux of the shared-path optimization: placing a
/// `SKILL.md` at this path makes the skill discoverable by *every* harness in
/// `covers` with a single materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPath {
    /// Project-relative directory that holds `<name>/SKILL.md`, e.g.
    /// `.agents/skills`.
    pub dir: &'static str,
    /// Harnesses that discover a `SKILL.md` under `dir`.
    pub covers: &'static [HarnessId],
    /// Whether a symlinked `<name>` entry under `dir` is verified to be followed
    /// by *every* covering harness. When false, materialize as a copy.
    pub symlink_verified: bool,
    /// Evidence backing this path's coverage claim.
    pub evidence: Evidence,
}

impl SkillPath {
    /// Whether `harness` discovers skills under this path.
    pub fn covers(&self, harness: HarnessId) -> bool {
        self.covers.contains(&harness)
    }
}

/// A single harness's native custom-agent destination. Unlike skills, these are
/// never shared: each selected harness gets its own materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTarget {
    /// Which harness this destination belongs to.
    pub harness: HarnessId,
    /// Project-relative directory the native agent file lives in, e.g.
    /// `.claude/agents`. For [`AgentTarget::needs_probe`] targets this is the
    /// registry's best default, subject to capability probing.
    pub dir: &'static str,
    /// Filename extension appended to the destination basename (no leading dot),
    /// e.g. `agent.md`, `md`, `toml`.
    pub ext: &'static str,
    /// The on-disk format the harness expects. akit copies variant bytes as-is;
    /// this is used only to validate that a catalog variant declares the right
    /// native format for the target.
    pub format: AgentFormat,
    /// Whether a symlinked agent file is verified to be followed by this harness.
    pub symlink_verified: bool,
    /// How the harness reloads a new agent file.
    pub reload: Reload,
    /// Minimum harness version required for this destination to work, if a
    /// specific version gate is known.
    pub min_version: Option<&'static str>,
    /// Evidence backing this destination.
    pub evidence: Evidence,
    /// True when the exact directory/naming is version-sensitive and must be
    /// resolved by probing the installed harness (e.g. OpenCode `agent/` vs
    /// `agents/`). Callers must not rely on `dir` blindly for these.
    pub needs_probe: bool,
}

impl AgentTarget {
    /// Whether this target is enabled for planning without any further probing.
    /// A probe-required or unverified target is not blindly usable.
    pub fn is_enabled(&self) -> bool {
        self.evidence.is_sufficient() && !self.needs_probe
    }

    /// The project-relative destination path for an agent installed under
    /// `basename` (the catalog-declared destination stem).
    pub fn destination(&self, basename: &str) -> String {
        format!("{}/{basename}.{}", self.dir, self.ext)
    }
}

/// The on-disk format a harness's custom-agent file uses. akit never converts
/// between these; a catalog variant must already be authored in the target
/// harness's native format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentFormat {
    /// Markdown body with YAML frontmatter (Copilot, Claude, Gemini, OpenCode).
    MarkdownYaml,
    /// TOML document (Codex).
    Toml,
}

// ── The registry data ────────────────────────────────────────────────────────
//
// Skill path coverage (July-2026 official-source audit):
//   .github/skills   → Copilot
//   .claude/skills   → Copilot, Claude, OpenCode
//   .agents/skills   → Copilot, Codex, Gemini, OpenCode
//   .gemini/skills   → Gemini
//   .opencode/skills → OpenCode
//
// Symlink-following for skills is officially confirmed only for Claude and
// Codex. Because `.claude/skills` is also read by Copilot/OpenCode (unverified)
// and `.agents/skills` by Copilot/Gemini/OpenCode (unverified), no *shared*
// path can currently be symlink-verified end-to-end — so all shared paths
// default to copy. `.gemini/skills` (Gemini-only) and `.opencode/skills`
// (OpenCode-only) are dead-ends no other harness reads and are intentionally
// omitted: the planner reaches Gemini/OpenCode via `.agents/skills`.

const SKILL_PATHS: &[SkillPath] = &[
    SkillPath {
        dir: ".agents/skills",
        covers: &[
            HarnessId::Copilot,
            HarnessId::Codex,
            HarnessId::Gemini,
            HarnessId::Opencode,
        ],
        symlink_verified: false,
        evidence: Evidence::OfficialDocs,
    },
    SkillPath {
        dir: ".claude/skills",
        covers: &[HarnessId::Copilot, HarnessId::Claude, HarnessId::Opencode],
        symlink_verified: false,
        evidence: Evidence::OfficialDocs,
    },
];

// Custom-agent native destinations. Every harness has a distinct proprietary
// directory + format; none are shared. Reload/version/symlink cells reflect the
// audit's "confirmed vs undetermined" columns — undetermined reload is `Unknown`
// (treated as restart), and symlink is only claimed where verified.
const AGENT_TARGETS: &[AgentTarget] = &[
    AgentTarget {
        harness: HarnessId::Copilot,
        dir: ".github/agents",
        ext: "agent.md",
        format: AgentFormat::MarkdownYaml,
        symlink_verified: false,
        reload: Reload::Restart,
        min_version: None,
        evidence: Evidence::OfficialDocs,
        needs_probe: false,
    },
    AgentTarget {
        harness: HarnessId::Claude,
        dir: ".claude/agents",
        ext: "md",
        format: AgentFormat::MarkdownYaml,
        symlink_verified: false,
        reload: Reload::Live,
        min_version: None,
        evidence: Evidence::OfficialDocs,
        needs_probe: false,
    },
    AgentTarget {
        harness: HarnessId::Codex,
        dir: ".codex/agents",
        ext: "toml",
        format: AgentFormat::Toml,
        symlink_verified: false,
        reload: Reload::Unknown,
        min_version: None,
        evidence: Evidence::OfficialDocs,
        needs_probe: false,
    },
    AgentTarget {
        harness: HarnessId::Gemini,
        dir: ".gemini/agents",
        ext: "md",
        format: AgentFormat::MarkdownYaml,
        symlink_verified: false,
        reload: Reload::Unknown,
        min_version: None,
        evidence: Evidence::OfficialDocs,
        needs_probe: false,
    },
    // OpenCode's official docs are internally inconsistent about `.opencode/agent`
    // vs `.opencode/agents`; the exact directory must be resolved against the
    // installed version, so this target is probe-gated.
    AgentTarget {
        harness: HarnessId::Opencode,
        dir: ".opencode/agent",
        ext: "md",
        format: AgentFormat::MarkdownYaml,
        symlink_verified: false,
        reload: Reload::Unknown,
        min_version: None,
        evidence: Evidence::OfficialDocs,
        needs_probe: true,
    },
];

/// All registered skill paths, in the order the planner should prefer on ties
/// (neutral `.agents/skills` first, then `.claude/skills`).
pub fn skill_paths() -> &'static [SkillPath] {
    SKILL_PATHS
}

/// The set of harnesses that discover skills at `dir`, if akit manages that path.
pub fn skill_path(dir: &str) -> Option<&'static SkillPath> {
    SKILL_PATHS.iter().find(|p| p.dir == dir)
}

/// All registered native custom-agent destinations, one per harness.
pub fn agent_targets() -> &'static [AgentTarget] {
    AGENT_TARGETS
}

/// The native custom-agent destination for `harness`.
pub fn agent_target(harness: HarnessId) -> &'static AgentTarget {
    AGENT_TARGETS
        .iter()
        .find(|t| t.harness == harness)
        .expect("every HarnessId has exactly one agent target")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_harness_ids_case_insensitively() {
        assert_eq!("copilot".parse::<HarnessId>(), Ok(HarnessId::Copilot));
        assert_eq!("Claude".parse::<HarnessId>(), Ok(HarnessId::Claude));
        assert_eq!("  CODEX ".parse::<HarnessId>(), Ok(HarnessId::Codex));
        assert_eq!("opencode".parse::<HarnessId>(), Ok(HarnessId::Opencode));
    }

    #[test]
    fn rejects_unknown_harness_with_supported_list() {
        let err = "cursor".parse::<HarnessId>().unwrap_err();
        assert_eq!(err.token, "cursor");
        let msg = err.to_string();
        assert!(msg.contains("cursor"));
        // The supported list must be actionable.
        for h in HarnessId::ALL {
            assert!(msg.contains(h.as_str()), "message should list {h}");
        }
    }

    #[test]
    fn wire_token_roundtrips_through_serde() {
        for h in HarnessId::ALL {
            let json = serde_json::to_string(&h).unwrap();
            assert_eq!(json, format!("\"{}\"", h.as_str()));
            let back: HarnessId = serde_json::from_str(&json).unwrap();
            assert_eq!(back, h);
        }
    }

    #[test]
    fn all_is_in_stable_registry_order() {
        // The planner's tie-break depends on this exact order.
        assert_eq!(
            HarnessId::ALL,
            [
                HarnessId::Copilot,
                HarnessId::Claude,
                HarnessId::Codex,
                HarnessId::Gemini,
                HarnessId::Opencode,
            ]
        );
    }

    #[test]
    fn agents_skills_covers_four_harnesses_but_not_claude() {
        let p = skill_path(".agents/skills").expect("registered");
        assert!(p.covers(HarnessId::Copilot));
        assert!(p.covers(HarnessId::Codex));
        assert!(p.covers(HarnessId::Gemini));
        assert!(p.covers(HarnessId::Opencode));
        assert!(!p.covers(HarnessId::Claude));
    }

    #[test]
    fn claude_skills_reaches_claude_and_two_others() {
        let p = skill_path(".claude/skills").expect("registered");
        assert!(p.covers(HarnessId::Claude));
        assert!(p.covers(HarnessId::Copilot));
        assert!(p.covers(HarnessId::Opencode));
        assert!(!p.covers(HarnessId::Codex));
        assert!(!p.covers(HarnessId::Gemini));
    }

    #[test]
    fn every_harness_is_covered_by_some_skill_path() {
        for h in HarnessId::ALL {
            assert!(
                skill_paths().iter().any(|p| p.covers(h)),
                "no skill path covers {h}"
            );
        }
    }

    #[test]
    fn claude_is_only_reachable_via_claude_skills() {
        // Load-bearing for the "all five needs two paths" invariant: Claude
        // reads no compatibility alias, so exactly one registered path reaches it.
        let reaching: Vec<_> = skill_paths()
            .iter()
            .filter(|p| p.covers(HarnessId::Claude))
            .collect();
        assert_eq!(reaching.len(), 1);
        assert_eq!(reaching[0].dir, ".claude/skills");
    }

    #[test]
    fn shared_skill_paths_default_to_copy() {
        // No shared path is symlink-verified end-to-end yet.
        for p in skill_paths() {
            assert!(
                !p.symlink_verified,
                "{} claims verified symlink without proof",
                p.dir
            );
        }
    }

    #[test]
    fn every_harness_has_exactly_one_agent_target() {
        for h in HarnessId::ALL {
            let matches: Vec<_> = agent_targets().iter().filter(|t| t.harness == h).collect();
            assert_eq!(matches.len(), 1, "{h} must have exactly one agent target");
        }
    }

    #[test]
    fn no_two_agent_targets_share_a_directory() {
        // Agents are never shared across harnesses.
        for (i, a) in agent_targets().iter().enumerate() {
            for b in &agent_targets()[i + 1..] {
                assert_ne!(a.dir, b.dir, "agent dirs must be harness-proprietary");
            }
        }
    }

    #[test]
    fn codex_agent_is_toml_others_markdown() {
        assert_eq!(agent_target(HarnessId::Codex).format, AgentFormat::Toml);
        for h in [
            HarnessId::Copilot,
            HarnessId::Claude,
            HarnessId::Gemini,
            HarnessId::Opencode,
        ] {
            assert_eq!(agent_target(h).format, AgentFormat::MarkdownYaml);
        }
    }

    #[test]
    fn opencode_agent_requires_probe_and_is_not_blindly_enabled() {
        let t = agent_target(HarnessId::Opencode);
        assert!(t.needs_probe);
        assert!(!t.is_enabled());
    }

    #[test]
    fn documented_agent_targets_are_enabled() {
        for h in [
            HarnessId::Copilot,
            HarnessId::Claude,
            HarnessId::Codex,
            HarnessId::Gemini,
        ] {
            assert!(agent_target(h).is_enabled(), "{h} agent should be enabled");
        }
    }

    #[test]
    fn agent_destination_uses_dir_ext_and_basename() {
        assert_eq!(
            agent_target(HarnessId::Copilot).destination("reviewer"),
            ".github/agents/reviewer.agent.md"
        );
        assert_eq!(
            agent_target(HarnessId::Claude).destination("reviewer"),
            ".claude/agents/reviewer.md"
        );
        assert_eq!(
            agent_target(HarnessId::Codex).destination("reviewer"),
            ".codex/agents/reviewer.toml"
        );
    }

    #[test]
    fn copilot_agents_need_restart_claude_agents_live() {
        assert_eq!(agent_target(HarnessId::Copilot).reload, Reload::Restart);
        assert_eq!(agent_target(HarnessId::Claude).reload, Reload::Live);
    }

    #[test]
    fn evidence_sufficiency_gates_enablement() {
        assert!(Evidence::OfficialDocs.is_sufficient());
        assert!(Evidence::OfficialSource.is_sufficient());
        assert!(Evidence::LiveVerified.is_sufficient());
        assert!(!Evidence::Unverified.is_sufficient());
    }
}
