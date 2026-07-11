//! The logical install planner (issue #32).
//!
//! Given a set of *selected* harnesses and a catalog item, this module computes
//! the minimal set of **physical materializations** that make the item
//! discoverable by every selected harness. It is pure (no I/O): it turns a
//! logical intent ("install `reviewer` for copilot + claude") into a concrete,
//! deterministic file plan the materializer (#31) executes and records.
//!
//! ## Skills — shared-path set cover
//!
//! Several harnesses read the *same* skill directory, so one materialization can
//! serve many. The planner runs a greedy set cover over the registry's
//! [`crate::harness::skill_paths`] to minimize the number of physical copies,
//! breaking ties toward the neutral `.agents/skills` path (registry order).
//!
//! ## Agents — one native file per harness
//!
//! Custom agents share nothing: each selected harness gets its own copy of that
//! harness's native variant at its proprietary destination.
//!
//! A selected harness that cannot be served (a skill incompatible with it, an
//! agent lacking its variant, or a probe-gated/unverified target) is not
//! silently dropped — it surfaces as a [`PlanIssue`] the caller reports.

use crate::agentpkg::{AgentPackage, SkillCompat};
use crate::harness::{self, HarnessId};
use crate::lockfile::Mode;

/// What a materialization installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatKind {
    /// A skill directory (`<skill-path>/<id>/`).
    SkillDir,
    /// A single native agent file.
    AgentFile,
}

/// One physical file/directory the plan will create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMaterialization {
    /// Project-relative destination path.
    pub path: String,
    /// How it is materialized (copy vs verified symlink).
    pub mode: Mode,
    /// The selected harnesses this single materialization makes the item
    /// discoverable by. Always non-empty and sorted.
    pub covers: Vec<HarnessId>,
    /// Whether this is a skill directory or an agent file.
    pub kind: MatKind,
    /// For agents: the package-relative source variant file to copy. `None` for
    /// skills (the whole skill directory is the source).
    pub source_file: Option<String>,
}

/// A selected harness that could not be served, with a machine-usable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanIssue {
    pub harness: HarnessId,
    pub reason: PlanIssueReason,
}

/// Why a selected harness was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanIssueReason {
    /// A skill declared incompatible with this harness via `skill.yml`.
    SkillIncompatible,
    /// An agent package provides no native variant for this harness.
    NoAgentVariant,
    /// The harness's target is disabled pending a runtime capability probe
    /// (e.g. OpenCode `agent/` vs `agents/`).
    NeedsProbe,
}

impl PlanIssueReason {
    /// Short human-facing explanation.
    pub const fn message(self) -> &'static str {
        match self {
            PlanIssueReason::SkillIncompatible => "skill declares it is not compatible",
            PlanIssueReason::NoAgentVariant => "agent has no variant for this harness",
            PlanIssueReason::NeedsProbe => {
                "destination is version-dependent and must be probed before install"
            }
        }
    }
}

/// The result of planning an install: what to materialize + what was skipped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Physical materializations, sorted by path for deterministic output.
    pub materializations: Vec<PlannedMaterialization>,
    /// Selected harnesses that could not be served.
    pub issues: Vec<PlanIssue>,
}

impl Plan {
    /// The harnesses actually served by at least one materialization.
    pub fn served(&self) -> Vec<HarnessId> {
        let mut set: Vec<HarnessId> = self
            .materializations
            .iter()
            .flat_map(|m| m.covers.iter().copied())
            .collect();
        set.sort();
        set.dedup();
        set
    }
}

/// Plan a skill install for `id` across `selected`, honoring `compat`.
///
/// Runs a greedy set cover over the registry skill paths, so shared harnesses
/// collapse onto a single materialization wherever possible.
pub fn plan_skill(id: &str, selected: &[HarnessId], compat: &SkillCompat) -> Plan {
    let mut plan = Plan::default();

    // Split selected into servable (compatible) and incompatible.
    let mut remaining: Vec<HarnessId> = Vec::new();
    for &h in dedup_sorted(selected).iter() {
        if compat.allows(h) {
            remaining.push(h);
        } else {
            plan.issues.push(PlanIssue {
                harness: h,
                reason: PlanIssueReason::SkillIncompatible,
            });
        }
    }

    // Greedy set cover: repeatedly pick the path covering the most still-
    // uncovered harnesses; ties break toward earlier registry order (which puts
    // the neutral `.agents/skills` first).
    let paths = harness::skill_paths();
    while !remaining.is_empty() {
        let mut best: Option<(usize, Vec<HarnessId>)> = None;
        for (idx, path) in paths.iter().enumerate() {
            let covered: Vec<HarnessId> = remaining
                .iter()
                .copied()
                .filter(|h| path.covers(*h))
                .collect();
            if covered.is_empty() {
                continue;
            }
            let better = match &best {
                None => true,
                Some((best_idx, best_cov)) => {
                    covered.len() > best_cov.len()
                        || (covered.len() == best_cov.len() && idx < *best_idx)
                }
            };
            if better {
                best = Some((idx, covered));
            }
        }

        let Some((idx, covered)) = best else {
            // No registered path can serve the remaining harnesses. This should
            // not happen (every harness is covered by some path), but guard
            // rather than loop forever.
            break;
        };
        let path = &paths[idx];
        remaining.retain(|h| !covered.contains(h));

        let mode = if path.symlink_verified {
            Mode::Symlink
        } else {
            Mode::Copy
        };
        plan.materializations.push(PlannedMaterialization {
            path: format!("{}/{id}", path.dir),
            mode,
            covers: covered,
            kind: MatKind::SkillDir,
            source_file: None,
        });
    }

    plan.materializations.sort_by(|a, b| a.path.cmp(&b.path));
    plan
}

/// Plan an agent install for `pkg` across `selected`: one native file per
/// harness that has a variant and an enabled (non-probe) target.
pub fn plan_agent(pkg: &AgentPackage, selected: &[HarnessId]) -> Plan {
    let mut plan = Plan::default();

    for &h in dedup_sorted(selected).iter() {
        if !pkg.supports(h) {
            plan.issues.push(PlanIssue {
                harness: h,
                reason: PlanIssueReason::NoAgentVariant,
            });
            continue;
        }
        let target = harness::agent_target(h);
        if target.needs_probe || !target.evidence.is_sufficient() {
            plan.issues.push(PlanIssue {
                harness: h,
                reason: PlanIssueReason::NeedsProbe,
            });
            continue;
        }
        let source_file = pkg.variants[&h].source_file.clone();
        let mode = if target.symlink_verified {
            Mode::Symlink
        } else {
            Mode::Copy
        };
        plan.materializations.push(PlannedMaterialization {
            path: target.destination(&pkg.basename),
            mode,
            covers: vec![h],
            kind: MatKind::AgentFile,
            source_file: Some(source_file),
        });
    }

    plan.materializations.sort_by(|a, b| a.path.cmp(&b.path));
    plan
}

fn dedup_sorted(harnesses: &[HarnessId]) -> Vec<HarnessId> {
    let mut v: Vec<HarnessId> = harnesses.to_vec();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crate::agentpkg::AgentVariant;
    use crate::harness::AgentFormat;

    fn pkg(id: &str, harnesses: &[HarnessId]) -> AgentPackage {
        let mut variants = BTreeMap::new();
        for &h in harnesses {
            variants.insert(
                h,
                AgentVariant {
                    harness: h,
                    source_file: format!("{}.file", h.as_str()),
                    format: AgentFormat::MarkdownYaml,
                },
            );
        }
        AgentPackage {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            category: String::new(),
            basename: id.to_string(),
            dir: PathBuf::from("/tmp/pkg"),
            variants,
        }
    }

    #[test]
    fn all_five_skill_harnesses_need_exactly_two_paths() {
        let plan = plan_skill("deploy", &HarnessId::ALL, &SkillCompat::Portable);
        assert_eq!(plan.issues, vec![]);
        assert_eq!(plan.materializations.len(), 2);
        // Every selected harness is served.
        assert_eq!(plan.served(), HarnessId::ALL.to_vec());
        // The two paths are the neutral one plus the Claude one.
        let paths: Vec<&str> = plan
            .materializations
            .iter()
            .map(|m| m.path.as_str())
            .collect();
        assert!(paths.contains(&".agents/skills/deploy"));
        assert!(paths.contains(&".claude/skills/deploy"));
    }

    #[test]
    fn single_non_claude_harness_uses_one_neutral_path() {
        let plan = plan_skill("deploy", &[HarnessId::Codex], &SkillCompat::Portable);
        assert_eq!(plan.materializations.len(), 1);
        assert_eq!(plan.materializations[0].path, ".agents/skills/deploy");
        assert_eq!(plan.materializations[0].covers, vec![HarnessId::Codex]);
        assert_eq!(plan.materializations[0].kind, MatKind::SkillDir);
    }

    #[test]
    fn claude_only_uses_claude_path() {
        let plan = plan_skill("deploy", &[HarnessId::Claude], &SkillCompat::Portable);
        assert_eq!(plan.materializations.len(), 1);
        assert_eq!(plan.materializations[0].path, ".claude/skills/deploy");
    }

    #[test]
    fn copilot_and_claude_collapse_onto_one_shared_path() {
        // Copilot is covered by BOTH paths; the greedy solver should pick the
        // single `.claude/skills` path that also covers Claude, not two paths.
        let plan = plan_skill(
            "deploy",
            &[HarnessId::Copilot, HarnessId::Claude],
            &SkillCompat::Portable,
        );
        assert_eq!(plan.materializations.len(), 1);
        assert_eq!(plan.materializations[0].path, ".claude/skills/deploy");
        assert_eq!(
            plan.materializations[0].covers,
            vec![HarnessId::Copilot, HarnessId::Claude]
        );
    }

    #[test]
    fn skills_default_to_copy_mode() {
        let plan = plan_skill("deploy", &HarnessId::ALL, &SkillCompat::Portable);
        for m in &plan.materializations {
            assert_eq!(m.mode, Mode::Copy);
        }
    }

    #[test]
    fn incompatible_skill_harness_becomes_issue_not_materialization() {
        let compat = SkillCompat::Only(vec![HarnessId::Claude]);
        let plan = plan_skill("deploy", &[HarnessId::Claude, HarnessId::Codex], &compat);
        assert_eq!(plan.materializations.len(), 1);
        assert_eq!(plan.materializations[0].path, ".claude/skills/deploy");
        assert_eq!(
            plan.issues,
            vec![PlanIssue {
                harness: HarnessId::Codex,
                reason: PlanIssueReason::SkillIncompatible,
            }]
        );
    }

    #[test]
    fn agent_plans_one_native_file_per_supported_harness() {
        let p = pkg("reviewer", &[HarnessId::Copilot, HarnessId::Claude]);
        let plan = plan_agent(&p, &[HarnessId::Copilot, HarnessId::Claude]);
        assert_eq!(plan.issues, vec![]);
        assert_eq!(plan.materializations.len(), 2);
        let paths: Vec<&str> = plan
            .materializations
            .iter()
            .map(|m| m.path.as_str())
            .collect();
        assert!(paths.contains(&".github/agents/reviewer.agent.md"));
        assert!(paths.contains(&".claude/agents/reviewer.md"));
        // Agents never share coverage.
        for m in &plan.materializations {
            assert_eq!(m.covers.len(), 1);
            assert_eq!(m.kind, MatKind::AgentFile);
            assert!(m.source_file.is_some());
        }
    }

    #[test]
    fn agent_missing_variant_becomes_issue() {
        let p = pkg("reviewer", &[HarnessId::Copilot]);
        let plan = plan_agent(&p, &[HarnessId::Copilot, HarnessId::Gemini]);
        assert_eq!(plan.materializations.len(), 1);
        assert_eq!(
            plan.issues,
            vec![PlanIssue {
                harness: HarnessId::Gemini,
                reason: PlanIssueReason::NoAgentVariant,
            }]
        );
    }

    #[test]
    fn agent_probe_gated_target_becomes_issue() {
        // OpenCode is probe-gated in the registry; even with a variant present it
        // must not be blindly materialized.
        let p = pkg("reviewer", &[HarnessId::Opencode]);
        let plan = plan_agent(&p, &[HarnessId::Opencode]);
        assert_eq!(plan.materializations, vec![]);
        assert_eq!(
            plan.issues,
            vec![PlanIssue {
                harness: HarnessId::Opencode,
                reason: PlanIssueReason::NeedsProbe,
            }]
        );
    }

    #[test]
    fn plans_are_deterministic_regardless_of_selection_order() {
        let a = plan_skill(
            "deploy",
            &[HarnessId::Opencode, HarnessId::Claude, HarnessId::Copilot],
            &SkillCompat::Portable,
        );
        let b = plan_skill(
            "deploy",
            &[HarnessId::Copilot, HarnessId::Opencode, HarnessId::Claude],
            &SkillCompat::Portable,
        );
        assert_eq!(a, b);
    }
}
