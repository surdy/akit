# Harness capability registry

`src/harness.rs` is the single source of truth for **how each supported CLI
harness discovers project-level customizations**. Every downstream stage — the
install planner, materialization, and the CLI/embedding surface — reads target
paths, coverage, symlink safety, versions, and reload guidance from here instead
of hardcoding `.github/...`.

This document records the July-2026 official-source audit the registry data is
derived from, and the verification evidence for each cell.

## Supported harnesses

| id         | label                | proprietary skill dir | proprietary agent dir |
|------------|----------------------|-----------------------|-----------------------|
| `copilot`  | GitHub Copilot CLI   | `.github/skills`      | `.github/agents`      |
| `claude`   | Claude Code          | `.claude/skills`      | `.claude/agents`      |
| `codex`    | OpenAI Codex CLI     | `.agents/skills`      | `.codex/agents`       |
| `gemini`   | Gemini CLI           | `.agents/skills`      | `.gemini/agents`      |
| `opencode` | OpenCode             | `.opencode/skills`    | `.opencode/agent(s)`  |

## Skills — path coverage

A `SKILL.md` directory placed under a path is discovered by **every** harness in
the *covers* column. This is what lets one materialization serve several
harnesses.

| path              | covers                                   | evidence      | symlink |
|-------------------|------------------------------------------|---------------|---------|
| `.github/skills`  | copilot                                  | official docs | unverified |
| `.claude/skills`  | copilot, claude, opencode                | official docs | unverified* |
| `.agents/skills`  | copilot, codex, gemini, opencode         | official docs | unverified* |
| `.gemini/skills`  | gemini                                   | official docs | unverified |
| `.opencode/skills`| opencode                                 | official docs | unverified |

\* Symlink-following for skills is officially confirmed only for **Claude** and
**Codex**. Because the two *shared* paths are also read by harnesses whose
symlink behavior is undetermined (Copilot/OpenCode read `.claude/skills`;
Copilot/Gemini/OpenCode read `.agents/skills`), no shared path is symlink-safe
end-to-end. **All shared paths therefore materialize as copies.**

### Registered vs omitted paths

The registry only registers the two paths the planner actually needs:

- **`.agents/skills`** — reaches copilot, codex, gemini, opencode (everyone but
  Claude). Preferred neutral path.
- **`.claude/skills`** — the *only* path that reaches Claude (Claude reads no
  compatibility alias), and also reaches copilot + opencode.

`.github/skills`, `.gemini/skills`, and `.opencode/skills` are single-harness
dead ends already reachable via the two shared paths, so they are intentionally
omitted from the planner's search space.

### Set-cover invariant

No single directory covers all five harnesses (Claude only reads
`.claude/skills`). **Covering all five requires exactly two directories:**
`.agents/skills` + `.claude/skills`. This is asserted by
`harness::tests::claude_is_only_reachable_via_claude_skills` and
`every_harness_is_covered_by_some_skill_path`.

## Custom agents — native destinations

Custom agents are **never shared**: each harness uses a proprietary directory,
extension, and file format. akit copies the catalog's native variant bytes
verbatim — it never transforms one format into another.

| harness  | destination                     | format        | reload   | symlink | evidence |
|----------|---------------------------------|---------------|----------|---------|----------|
| copilot  | `.github/agents/<n>.agent.md`   | markdown+yaml | restart  | copy    | official docs |
| claude   | `.claude/agents/<n>.md`         | markdown+yaml | live     | copy    | official docs |
| codex    | `.codex/agents/<n>.toml`        | toml          | unknown  | copy    | official docs |
| gemini   | `.gemini/agents/<n>.md`         | markdown+yaml | unknown  | copy    | official docs |
| opencode | `.opencode/agent/<n>.md` †      | markdown+yaml | unknown  | copy    | official docs |

† **OpenCode's official docs are internally inconsistent** about
`.opencode/agent` vs `.opencode/agents`. The target is flagged `needs_probe`, so
the caller must resolve the real directory against the installed version rather
than trusting the registry default. `AgentTarget::is_enabled()` returns `false`
for probe-gated targets.

Undetermined reload behavior is recorded as `Unknown` and treated
conservatively as "restart if it does not appear".

## Evidence model

A capability is only *enabled* when its behavior is backed by trusted
[`Evidence`]:

- `official-docs` — vendor documentation.
- `official-source` — proven by reading the harness's OSS implementation.
- `live-verified` — proven by an isolated temporary-project behavioral test.
- `unverified` — **not** enabled; treated as disabled with a reason.

`Evidence::is_sufficient()` gates enablement; only `unverified` fails it.

## Changing the matrix

When a harness ships a new discovery path or a behavior is verified:

1. Update the relevant `SkillPath` / `AgentTarget` entry (and its `evidence`).
2. Only set `symlink_verified: true` when following is confirmed for **every**
   covering harness of that path.
3. Only clear `needs_probe` once the version-sensitive ambiguity is resolved.
4. Update this document's tables with the new citation/evidence.
5. Keep `HarnessId::ALL` in stable registry order — the planner's tie-break
   depends on it.
