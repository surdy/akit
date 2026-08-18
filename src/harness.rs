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
//!   [`SkillPath`] records exactly which harnesses cover it. Per-harness reload
//!   and version facts live on [`SkillSupport`], because those are properties of
//!   the *harness*, not of the shared directory.
//! - **Custom agents** have *no* shared path: every harness uses a proprietary
//!   directory and file format, so an agent must be materialized once per
//!   selected harness from an explicit native variant. Each [`AgentTarget`]
//!   records that native destination.
//! - A capability is only **enabled** when its discovery behavior is backed by
//!   [`Evidence`] we trust (official docs, official source, or an isolated live
//!   behavioral test). Behavior that is version-sensitive in a way no static
//!   default can cover is marked [`AgentTarget::needs_probe`] so the caller
//!   resolves it against the installed version rather than guessing. No target
//!   currently needs one — OpenCode's `agent/` vs `agents/` ambiguity was
//!   resolved against the source in #46 (see `AGENT_TARGETS`) — but the flag and
//!   its planner path stay, because the next ambiguity will need them.
//! - **Symlink** discovery is only claimed where verified; every other target
//!   materializes as a copy (see [`SkillPath::symlink_verified`] /
//!   [`AgentTarget::symlink_verified`]).
//! - Reload behavior that no primary source establishes is recorded as
//!   [`Reload::Unknown`] rather than guessed, and the UI degrades to an honest
//!   "restart if it does not appear" hint for those cells.
//!
//! The matrix below is derived from the August-2026 primary-source audit; see
//! `docs/harness-registry.md` for the per-cell citation URLs and the quote each
//! claim rests on.

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

    /// Whether this harness is *confirmed* to follow a symlinked skill directory
    /// for discovery. Only Claude and Codex are confirmed (see the `SKILL_PATHS`
    /// registry note); every other harness is treated as copy-only, so a forced
    /// `install --symlink` never yields a skill a covering harness can't discover.
    ///
    /// This is the per-harness capability that [`SkillPath::symlink_verified`]
    /// collapses (AND-ed) across a shared path's coverers — it lets the planner,
    /// under an explicit symlink request, symlink a materialization exactly when
    /// *every* harness it serves is a confirmed follower.
    pub const fn follows_skill_symlink(self) -> bool {
        matches!(self, HarnessId::Claude | HarnessId::Codex)
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
    /// Whether the install planner may *choose* this path.
    ///
    /// The registry records **every** verified discovery directory so the matrix
    /// is auditable and queryable (`akit` must be able to answer "does harness X
    /// read directory Y?" for a path it did not write). Single-harness
    /// proprietary directories are nevertheless **coverage-redundant**: every
    /// harness that reads one also reads a shared path, so planning into them
    /// would only add duplicate materializations. Those are registered with
    /// `plannable: false` and excluded from [`planner_skill_paths`].
    pub plannable: bool,
}

impl SkillPath {
    /// Whether `harness` discovers skills under this path.
    pub fn covers(&self, harness: HarnessId) -> bool {
        self.covers.contains(&harness)
    }
}

/// Per-harness **skill** capability: the reload and version facts that
/// [`AgentTarget`] already carries for agents (issue #46).
///
/// Skill *destinations* are shared across harnesses ([`SkillPath`]), but
/// reload behavior and version gates are properties of the **harness**, not of
/// the directory — Claude Code and Copilot CLI both read `.claude/skills`, yet
/// pick a new skill up differently. So skills get one entry per harness rather
/// than per path, which is also what post-install guidance needs (it reports per
/// served harness).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSupport {
    /// The harness these facts describe.
    pub harness: HarnessId,
    /// How this harness picks up a newly materialized skill directory.
    pub reload: Reload,
    /// Minimum harness version that supports project-level skills at all, when a
    /// specific version gate is known.
    pub min_version: Option<&'static str>,
    /// Evidence backing the reload/version cells above (the *coverage* claim has
    /// its own evidence on [`SkillPath`]).
    pub evidence: Evidence,
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
// Every cell below is backed by a primary source cited in
// `docs/harness-registry.md` (August-2026 re-audit, issue #46). The doc holds the
// URLs and the supporting quote; the comments here record only the conclusion and
// which side of a docs-vs-source disagreement we followed.
//
// Skill path coverage:
//   .agents/skills   → Copilot, Codex, Gemini, OpenCode   (shared)
//   .claude/skills   → Copilot, Claude, OpenCode          (shared)
//   .github/skills   → Copilot                            (redundant alias)
//   .codex/skills    → Codex                              (redundant alias)
//   .gemini/skills   → Gemini                             (redundant alias)
//   .opencode/skills → OpenCode                           (redundant alias)
//
// All six are *registered* so the matrix is complete and queryable, but only the
// two shared paths are `plannable` — every harness that reads a single-harness
// alias also reads a shared path, so planning into an alias would only duplicate
// bytes. See `SkillPath::plannable`.
//
// Symlink-following for skills is confirmed for Claude and Codex (and, from
// source, OpenCode — see the doc's symlink note; not yet promoted to
// `HarnessId::follows_skill_symlink`, which stays the conservative gate). Because
// `.claude/skills` is also read by Copilot (symlink behavior undetermined) and
// `.agents/skills` by Copilot/Gemini (likewise), no *shared* path is
// symlink-verified end-to-end, so all shared paths default to copy.

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
        plannable: true,
    },
    SkillPath {
        dir: ".claude/skills",
        covers: &[HarnessId::Copilot, HarnessId::Claude, HarnessId::Opencode],
        symlink_verified: false,
        evidence: Evidence::OfficialDocs,
        plannable: true,
    },
    // ── Coverage-redundant single-harness aliases ───────────────────────────
    // Recorded for auditability (crit 2 of the #33 verification pass); never
    // planned into, because the harness each one serves is already reachable
    // through a shared path above.
    SkillPath {
        dir: ".github/skills",
        covers: &[HarnessId::Copilot],
        symlink_verified: false,
        evidence: Evidence::OfficialDocs,
        plannable: false,
    },
    // Undocumented in OpenAI's public skills table but live in the Codex source
    // (and dogfooded by the openai/codex repo itself), hence `OfficialSource`.
    SkillPath {
        dir: ".codex/skills",
        covers: &[HarnessId::Codex],
        symlink_verified: false,
        evidence: Evidence::OfficialSource,
        plannable: false,
    },
    SkillPath {
        dir: ".gemini/skills",
        covers: &[HarnessId::Gemini],
        symlink_verified: false,
        evidence: Evidence::OfficialDocs,
        plannable: false,
    },
    SkillPath {
        dir: ".opencode/skills",
        covers: &[HarnessId::Opencode],
        symlink_verified: false,
        evidence: Evidence::OfficialDocs,
        plannable: false,
    },
];

// Per-harness skill reload/version facts. `min_version` is the floor at which
// *every akit-plannable* skill destination for that harness works — for the four
// harnesses akit reaches through `.agents/skills`, that is the release which
// added `.agents/skills` support, not the (earlier) release that first shipped
// skills. Gating on the earlier one would claim support for a version that
// cannot read the directory akit actually writes.
const SKILL_SUPPORT: &[SkillSupport] = &[
    // `/skills reload` is an explicit in-session command; Copilot CLI does not
    // watch the skill directories. `.agents/skills` landed in 0.0.401.
    SkillSupport {
        harness: HarnessId::Copilot,
        reload: Reload::Command,
        min_version: Some("0.0.401"),
        evidence: Evidence::OfficialDocs,
    },
    // Claude Code watches the skill directories and hot-reloads within the
    // session (shipped in 2.1.0; skills themselves in 2.0.20 — the gate is the
    // feature floor, not the hot-reload floor).
    SkillSupport {
        harness: HarnessId::Claude,
        reload: Reload::Live,
        min_version: Some("2.0.20"),
        evidence: Evidence::OfficialDocs,
    },
    // Codex watches the skill roots (throttled ~10s) — "Codex detects skill
    // changes automatically". `.agents/skills` landed in 0.94.0.
    SkillSupport {
        harness: HarnessId::Codex,
        reload: Reload::Live,
        min_version: Some("0.94.0"),
        evidence: Evidence::OfficialDocs,
    },
    // Gemini CLI has no watcher; `/skills reload` (alias `/skills refresh`) is
    // the documented refresh. `.agents/skills` alias landed in 0.28.0.
    SkillSupport {
        harness: HarnessId::Gemini,
        reload: Reload::Command,
        min_version: Some("0.28.0"),
        evidence: Evidence::OfficialDocs,
    },
    // OpenCode's docs say nothing about reload, but the source is unambiguous:
    // skills are discovered once into a no-TTL instance cache and no filesystem
    // watcher covers them, so a new skill needs a restart. Skills shipped 1.0.186.
    SkillSupport {
        harness: HarnessId::Opencode,
        reload: Reload::Restart,
        min_version: Some("1.0.186"),
        evidence: Evidence::OfficialSource,
    },
];

// Custom-agent native destinations. Every harness has a distinct proprietary
// directory + format; none are shared. Undetermined reload stays `Unknown`
// (treated as restart), and symlink is only claimed where verified.
const AGENT_TARGETS: &[AgentTarget] = &[
    // "Restart the CLI to load your new custom agent." Custom agents shipped in
    // 0.0.353.
    AgentTarget {
        harness: HarnessId::Copilot,
        dir: ".github/agents",
        ext: "agent.md",
        format: AgentFormat::MarkdownYaml,
        symlink_verified: false,
        reload: Reload::Restart,
        min_version: Some("0.0.353"),
        evidence: Evidence::OfficialDocs,
        needs_probe: false,
    },
    // Claude Code watches `.claude/agents` and picks a new subagent up within a
    // few seconds. No documented version floor for project subagents.
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
    // Codex's subagent docs document the directory and the TOML schema but say
    // nothing about in-session reload, and the only watcher in the source covers
    // *skill* roots — so reload stays honestly `Unknown`. Directory
    // auto-discovery of `.codex/agents/*.toml` landed in 0.115.0.
    AgentTarget {
        harness: HarnessId::Codex,
        dir: ".codex/agents",
        ext: "toml",
        format: AgentFormat::Toml,
        symlink_verified: false,
        reload: Reload::Unknown,
        min_version: Some("0.115.0"),
        evidence: Evidence::OfficialDocs,
        needs_probe: false,
    },
    // `/agents reload` ("Rescans agent directories … and reloads the registry").
    // Gemini CLI has no watcher. Markdown+frontmatter agents replaced the older
    // TOML loader in 0.25.0, which is the floor for the format akit writes.
    AgentTarget {
        harness: HarnessId::Gemini,
        dir: ".gemini/agents",
        ext: "md",
        format: AgentFormat::MarkdownYaml,
        symlink_verified: false,
        reload: Reload::Command,
        min_version: Some("0.25.0"),
        evidence: Evidence::OfficialDocs,
        needs_probe: false,
    },
    // The `agent/` vs `agents/` ambiguity is **resolved, not probed** (#46).
    //
    // OpenCode's source globs `{agent,agents}/**/*.md`, so both spellings work on
    // current versions — but the plural was a *hard error* (`ConfigDirectoryTypoError`)
    // before v1.0.219. The singular `.opencode/agent` is therefore the only form
    // that is correct on every OpenCode release that ever shipped markdown agents,
    // which makes a runtime probe strictly unnecessary: there is no installed
    // version for which probing would pick a different answer than this pin.
    // Hence `needs_probe: false` with `Evidence::OfficialSource` (the public docs
    // list only the plural; OpenCode's own bundled `customize-opencode` skill and
    // the source agree with the singular).
    //
    // Reload: no watcher covers agent discovery and the config/agent list is
    // cached per instance with no TTL, so a new agent file needs a restart.
    // Markdown agents shipped in 0.3.65.
    AgentTarget {
        harness: HarnessId::Opencode,
        dir: ".opencode/agent",
        ext: "md",
        format: AgentFormat::MarkdownYaml,
        symlink_verified: false,
        reload: Reload::Restart,
        min_version: Some("0.3.65"),
        evidence: Evidence::OfficialSource,
        needs_probe: false,
    },
];

/// **Every** registered skill path — the shared ones the planner uses *and* the
/// coverage-redundant single-harness aliases — in stable registry order.
///
/// Use this to *answer questions* about a directory (does harness X read it?).
/// Use [`planner_skill_paths`] to *choose* a destination.
pub fn skill_paths() -> &'static [SkillPath] {
    SKILL_PATHS
}

/// The subset of [`skill_paths`] the install planner may choose from, in the
/// order it should prefer on ties (neutral `.agents/skills` first, then
/// `.claude/skills`). Coverage-redundant aliases are filtered out, so registering
/// one never changes an install plan.
pub fn planner_skill_paths() -> impl Iterator<Item = &'static SkillPath> {
    SKILL_PATHS.iter().filter(|p| p.plannable)
}

/// The set of harnesses that discover skills at `dir`, if akit manages that path.
pub fn skill_path(dir: &str) -> Option<&'static SkillPath> {
    SKILL_PATHS.iter().find(|p| p.dir == dir)
}

/// Per-harness skill reload/version facts, in stable registry order.
pub fn skill_supports() -> &'static [SkillSupport] {
    SKILL_SUPPORT
}

/// The skill capability facts for `harness`.
pub fn skill_support(harness: HarnessId) -> &'static SkillSupport {
    SKILL_SUPPORT
        .iter()
        .find(|s| s.harness == harness)
        .expect("every HarnessId has exactly one skill-support entry")
}

/// How `harness` picks up a newly materialized `primitive` — the single entry
/// point post-install guidance uses, so skills and agents are answered from the
/// same registry rather than from a hardcoded hint.
pub fn reload_for(primitive: Primitive, harness: HarnessId) -> Reload {
    match primitive {
        Primitive::Skill => skill_support(harness).reload,
        Primitive::Agent => agent_target(harness).reload,
    }
}

/// The known minimum harness version for `primitive` on `harness`, if any.
pub fn min_version_for(primitive: Primitive, harness: HarnessId) -> Option<&'static str> {
    match primitive {
        Primitive::Skill => skill_support(harness).min_version,
        Primitive::Agent => agent_target(harness).min_version,
    }
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
    fn opencode_agent_dir_is_pinned_to_the_singular_form() {
        // #46: resolved against the OpenCode source rather than probed. The
        // plural `.opencode/agents` was a hard error before v1.0.219, so the
        // singular is the only spelling correct on every shipped version.
        let t = agent_target(HarnessId::Opencode);
        assert_eq!(t.dir, ".opencode/agent");
        assert!(!t.needs_probe);
        assert!(t.is_enabled());
        assert_eq!(t.evidence, Evidence::OfficialSource);
    }

    #[test]
    fn no_agent_target_currently_needs_a_probe() {
        // The probe path is still wired (PlanIssueReason::NeedsProbe); it just
        // has no subject today. Flipping any target back to `needs_probe: true`
        // must be a deliberate change that trips this test.
        for t in agent_targets() {
            assert!(!t.needs_probe, "{} unexpectedly needs a probe", t.harness);
        }
    }

    #[test]
    fn every_agent_target_is_enabled() {
        for h in HarnessId::ALL {
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

    // ── Per-primitive skill facts (#46) ──────────────────────────────────────

    #[test]
    fn every_harness_has_exactly_one_skill_support_entry() {
        for h in HarnessId::ALL {
            let matches: Vec<_> = skill_supports().iter().filter(|s| s.harness == h).collect();
            assert_eq!(matches.len(), 1, "{h} must have exactly one skill support");
        }
        assert_eq!(skill_supports().len(), HarnessId::ALL.len());
    }

    #[test]
    fn skill_reload_is_recorded_per_harness_not_per_path() {
        // Copilot and Claude share `.claude/skills` yet reload differently — the
        // whole reason skill reload hangs off the harness, not the directory.
        assert!(
            skill_path(".claude/skills")
                .unwrap()
                .covers(HarnessId::Copilot)
        );
        assert!(
            skill_path(".claude/skills")
                .unwrap()
                .covers(HarnessId::Claude)
        );
        assert_eq!(skill_support(HarnessId::Copilot).reload, Reload::Command);
        assert_eq!(skill_support(HarnessId::Claude).reload, Reload::Live);
    }

    #[test]
    fn opencode_skills_need_a_restart_from_source_evidence() {
        let s = skill_support(HarnessId::Opencode);
        assert_eq!(s.reload, Reload::Restart);
        assert_eq!(s.evidence, Evidence::OfficialSource);
    }

    #[test]
    fn reload_for_answers_both_primitives_from_the_registry() {
        for h in HarnessId::ALL {
            assert_eq!(reload_for(Primitive::Skill, h), skill_support(h).reload);
            assert_eq!(reload_for(Primitive::Agent, h), agent_target(h).reload);
        }
        // Skills and agents genuinely differ for at least one harness, so the
        // guidance really is per-primitive and not a relabelled agent hint.
        assert_ne!(
            reload_for(Primitive::Skill, HarnessId::Copilot),
            reload_for(Primitive::Agent, HarnessId::Copilot)
        );
    }

    #[test]
    fn every_harness_has_a_skill_version_gate_and_it_parses() {
        for h in HarnessId::ALL {
            let v = min_version_for(Primitive::Skill, h)
                .unwrap_or_else(|| panic!("{h} skill min_version"));
            // Must be a dotted-numeric string `verify::version_ge` can compare.
            assert!(
                v.split('.').all(|p| p.parse::<u64>().is_ok()),
                "{h} skill min_version '{v}' is not dotted-numeric"
            );
        }
    }

    #[test]
    fn agent_version_gates_are_dotted_numeric_where_present() {
        for h in HarnessId::ALL {
            if let Some(v) = min_version_for(Primitive::Agent, h) {
                assert!(
                    v.split('.').all(|p| p.parse::<u64>().is_ok()),
                    "{h} agent min_version '{v}' is not dotted-numeric"
                );
            }
        }
    }

    // ── Registered vs plannable paths (#46 crit 2) ───────────────────────────

    #[test]
    fn every_verified_alias_path_is_registered() {
        for dir in [
            ".github/skills",
            ".codex/skills",
            ".gemini/skills",
            ".opencode/skills",
        ] {
            let p = skill_path(dir).unwrap_or_else(|| panic!("{dir} should be registered"));
            assert!(!p.plannable, "{dir} is coverage-redundant, not plannable");
        }
    }

    #[test]
    fn only_the_two_shared_paths_are_plannable() {
        let plannable: Vec<&str> = planner_skill_paths().map(|p| p.dir).collect();
        assert_eq!(plannable, vec![".agents/skills", ".claude/skills"]);
    }

    #[test]
    fn redundant_aliases_serve_only_harnesses_a_shared_path_already_reaches() {
        // This is what makes them safe to omit from planning.
        for p in skill_paths().iter().filter(|p| !p.plannable) {
            for &h in p.covers {
                assert!(
                    planner_skill_paths().any(|s| s.covers(h)),
                    "{} covers {h}, which no plannable path reaches",
                    p.dir
                );
            }
        }
    }

    #[test]
    fn evidence_sufficiency_gates_enablement() {
        assert!(Evidence::OfficialDocs.is_sufficient());
        assert!(Evidence::OfficialSource.is_sufficient());
        assert!(Evidence::LiveVerified.is_sufficient());
        assert!(!Evidence::Unverified.is_sufficient());
    }
}
