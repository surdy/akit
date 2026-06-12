# On-demand personal Copilot customizations ("Copilot Kit") — design & plan

> Status: proposal only. No code written. Focus: GitHub Copilot CLI first; extensible to Codex/Claude later.

## Problem
User keeps a personal collection of skills / custom agents / "prompts" in `~/.copilot/`.
Because `~/.copilot/` is **user scope**, every item is active in **every** project → noise,
context bloat, irrelevant skills. Want: pull selected items into a project **on demand**,
remove them just as easily. Future: multiple sources + an APM-style manifest for selecting
which files form the searchable collection.

## Verified facts about Copilot CLI customization (authoritative — GitHub Docs + CLI help)
Customization primitives are: **custom instructions, skills, custom agents, hooks, MCP, plugins**.
There is **no `prompts/` primitive** in the CLI — `.github/prompts/*.prompt.md` is a VS Code feature.
Reusable task-prompts in the CLI = **skills** (slash-invocable, just-in-time, description-matched).

| Primitive | Personal (all projects) | Project (per-repo) | Format | Override |
|---|---|---|---|---|
| Custom agents | `~/.copilot/agents/*.agent.md` | `.github/agents/*.agent.md` | `.agent.md` | project > personal |
| Skills | `~/.copilot/skills/<name>/SKILL.md` (or `~/.agents/skills`) | `.github/skills/<name>/SKILL.md` (or `.claude/skills`, `.agents/skills`) | folder + `SKILL.md` | project > personal |
| Instructions | `~/.copilot/copilot-instructions.md`, `~/.copilot/instructions/*.instructions.md` | `.github/copilot-instructions.md`, `.github/instructions/**/*.instructions.md`, `AGENTS.md` | Markdown | system > repo > org |
| Hooks | `~/.copilot/hooks/` | `.github/hooks/` | scripts | loaded together |
| MCP | `~/.copilot/mcp-config.json` | `.mcp.json` / `.github/mcp.json` | JSON | project > user |

Relevant native levers:
- **Per-project dirs** (`.github/skills`, `.github/agents`) are exactly the on-demand target. Project > personal on name conflict.
- **`/skills`** toggles skills on/off per session (space bar); **`/skills add <dir>`** registers an *extra* skills location (can be outside the repo); **`/skills remove`**, **`/skills reload`**, **`/skills info`** (shows location).
  - NOTE: there is **no** equivalent "add external location" for custom agents — agents only load from `~/.copilot/agents` or `.github/agents`. This asymmetry constrains "keep everything outside the repo" designs.
- **`COPILOT_HOME`** env var relocates the *entire* `~/.copilot` (sessions, auth, plugins, settings) — too heavy for per-project scoping unless combined with symlinks.
- **`COPILOT_CUSTOM_INSTRUCTIONS_DIRS`** env var adds extra instruction dirs (instructions only).
- **Plugins + marketplaces** (`/plugin`, `installed-plugins/`) bundle skills/agents/hooks/MCP; **user-scope** though, so still "all projects" unless toggled. `gh skill` (GitHub CLI) can search/install/update/publish skills.
- **APM (microsoft/apm)**: `apm.yml` manifest + `apm install` resolves agentic deps (skills/agents/instructions/plugins/MCP) from any git source with lockfile, integrity hashes, transitive deps, marketplaces, security scanning; `apm compile -t copilot` writes into `.github/...`. Source spec: `owner/repo/path[#ref]`, e.g. `apm install vercel-labs/agent-skills --skill deploy-to-vercel`. Cross-harness. **This already implements the "future multi-source manifest" goal.**

## Core insight
The "all files in all projects" symptom is caused purely by living in `~/.copilot/` (user scope).
The fix is structural: **move the canonical collection OUT of any auto-discovered Copilot dir** into a
neutral store, then **materialize only selected items into each project's `.github/{skills,agents}`** on demand.

## Evaluation of the symlink idea (user's proposal)
Verdict: **viable and the right primitive for the "pull/unlink" action**, with caveats.

Pros: project dirs are exactly where Copilot looks; symlink = instant, reversible (unlink), single
source of truth (edits propagate); no duplication.

Caveats / must-handle:
1. **Repo pollution / leaking personal absolute paths.** `.github/` is committed. A symlink there
   would be committed as a (likely broken-for-teammates) symlink. → These personal pulls must be
   **git-ignored** via `.git/info/exclude` (local, doesn't touch the tracked `.gitignore`).
2. **Symlink-follow is unverified.** Must test that Copilot follows a **symlinked skill *directory***
   and a symlinked `*.agent.md` (`/skills list`, `/skills info`, `/agent`). If not → fall back to copy
   or to `/skills add <external-dir>` for skills.
3. **Windows** symlink support is poor (needs Dev Mode/admin) → copy fallback for portability.
4. **No record of what's pulled** → need a per-project **lockfile** (gitignored) to power list / remove /
   remove-all / sync / status and to avoid orphans.
5. Symlinking is only the *action*; still need a **search/browse** layer over the collection.

## Alternative solutions
- **A. Symlink (default) + copy (fallback)** into `.github/{skills,agents}`, gitignored, with lockfile. ✅ recommended primitive.
- **B. Copy-only mode.** Portable, Windows-safe, can optionally be committed to share with a team. No propagation; "remove" = delete. Good as a mode/flag.
- **C. Out-of-repo registration.** `/skills add ~/.copilot-kits/<proj>/skills` + `COPILOT_CUSTOM_INSTRUCTIONS_DIRS`. Avoids repo pollution but **agents can't** be registered this way → non-uniform. Weaker.
- **D. Native plugins + personal marketplace.** Author the collection as plugin(s), `/plugin install` per project, toggle. Good for *distribution*, but install is user-scope (not per-project) and authoring is heavier. Complementary, not the core.
- **E. Adopt APM as the backend.** Strongest for the *future* multi-source + manifest goal; already does selection (`--skill`), lockfile, security, cross-harness. But it's oriented to *project-shared, committed, reproducible* config; for *personal ephemeral* use you'd gitignore its outputs. Heavier than needed for the seamless add/remove now, but ideal as the eventual engine.
- **F. pterm-integrated manager** (UX layer over A). Searchable palette + "active kits in this dir" panel + integrate with the new launch-in-directory flow.

## Recommended architecture — two layers
**1. Headless core engine** (standalone lib + tiny CLI, e.g. `ckit`), harness-agnostic:
- Index a **collection**: v1 = one git repo with `skills/`, `agents/` (+ any "prompts" treated as skills);
  parse YAML frontmatter (`name`, `description`, category) for search.
- `add <item>`: materialize into `.github/skills/<name>/` or `.github/agents/<name>.agent.md`
  via **symlink (default) / copy (`--copy`, auto on Windows)**.
- Auto-append the materialized paths to **`.git/info/exclude`** (never commit personal pulls).
- Maintain a **project lockfile** `.copilot/kit.lock.json` (gitignored): item id, source repo+ref, mode, target path.
- Commands: `search`, `add`, `rm`, `ls`, `status`, `sync`, `update`.

**2. pterm UX layer** (where a GUI genuinely wins):
- Fuzzy **search/browse/preview** of the collection (reuse command-palette infra; show frontmatter descriptions).
- **"Kits in this project"** panel (per active directory) with add/remove toggles → calls the core engine.
- Hook into the **agent-launch dialog** just built: "Launch Copilot in <dir> with kits [a, b, c]".

Why split: the file logic (search, link/copy, gitignore, lockfile, sync) is tool-agnostic and belongs in a
reusable, unit-testable core usable from CI/headless/other editors; pterm provides discovery/one-click UX
without owning fragile filesystem logic. Keeps the door open to delegate the heavy lifting to **APM** later.

## Multiple sources (future)
Distinguish two manifests:
- **Collection manifest** (user-global, e.g. `~/.copilot-kit/sources.yml`): declares WHERE the searchable
  pool comes from — multiple repos + path/glob selectors. APM-style source spec `owner/repo/path[#ref]`.
- **Project lockfile** (per-repo, gitignored): declares WHAT was pulled into this project.
Either implement this thin selection layer, or adopt **APM** as the backend and let pterm be the GUI over it.

## Should it live in pterm?
Yes for the **UX** (pterm already owns multi-agent + the new per-directory launch). But put the **core engine
in a standalone lib/CLI** so it's reusable and testable; pterm wraps it. Avoid coupling filesystem provisioning
to the terminal app, and avoid reinventing APM's manifest/resolution.

## Phase 0 validation — RESULTS (2026-06-12, Copilot CLI 1.0.62)
All unknowns validated **positive** with a throwaway harness (canonical collection outside the repo, symlinked into a temp git project's `.github/`). Method: ran `copilot -C <proj> --log-level all --log-dir <tmp> --no-remote -p "..."` and inspected the system prompt captured in the debug log.

1. **Symlinked skill directory IS followed.** A `.github/skills/<name>` symlink pointing at an out-of-repo `SKILL.md` dir showed up in the model's `<available_skills>` block as `<name>zzz-phase0-skill</name>` with **`<location>project</location>`**. → symlink-default is safe for skills.
2. **Symlinked `.agent.md` IS followed.** A `.github/agents/<name>.agent.md` symlink appeared in the Task tool's `agent_type` **enum** (`"zzz-phase0-agent"`). → symlink-default is safe for agents.
3. **`.git/info/exclude` fully hides pulls.** After adding the two item paths, `git status --porcelain` was empty and `git check-ignore -v` attributed both to `.git/info/exclude`. Per-item patterns, so real `.github/workflows` etc. stay tracked. → no repo pollution, no teammate breakage.
4. **No prompts primitive confirmed.** `copilot help commands/config/environment` expose only **instructions, skills, agents, hooks, MCP, plugins** — there is **no `/prompt` command and no prompt-file directory**. Reusable prompts must be modeled as **skills** (open question #5 → resolved: use skills). Also confirmed relocation levers `COPILOT_HOME` and `COPILOT_CUSTOM_INSTRUCTIONS_DIRS`, and config `customAgents.defaultLocalOnly`.

**Implications:** the symlink-default / copy-fallback decision is validated for both skills and agents; copy fallback is now only needed for Windows (no-admin symlinks), not for discovery reasons. Reversibility = a simple `unlink` (no Copilot re-run needed to "unregister"). Harness was deleted after the run.

## Phased plan
- **Phase 0 — validate unknowns:** ✅ **DONE — all positive (see results above).**
- **Phase 1 — core engine MVP (single repo collection):** index + frontmatter search; `add/rm/ls/status`
  with symlink-default/copy-fallback; auto-gitignore; lockfile. Manual CLI usable standalone.
- **Phase 2 — pterm integration:** search palette + per-project kits panel + launch-dialog "attach kits".
- **Phase 3 — multiple sources + manifest:** collection `sources.yml` (multi-repo/glob) OR adopt APM backend.
- **Phase 4 — cross-harness:** map the same items to Codex/Claude target dirs (or via APM `compile -t`).

## Open questions for the user
_Questions 1–4 are now resolved — see "Resolved decisions" below._
1. ~~Pull mode default — symlink vs copy?~~ → **Resolved: symlink default, copy fallback.**
2. ~~Committable/shareable vs always personal?~~ → **Resolved: always personal + gitignored.**
3. ~~Granularity — individual items vs bundles?~~ → **Resolved: both.**
4. ~~Thin custom engine vs adopt APM?~~ → **Resolved: adopt APM as backend, pterm as GUI.**
5. ~~Are "prompts" acceptable to model as **skills** in the CLI?~~ → **Resolved (Phase 0): the CLI has no prompts primitive, so reusable prompts are modeled as skills.**

## Resolved decisions (2026-06-12)
1. **Materialization:** symlink by default, **copy as fallback** (Windows / when symlink-follow fails).
2. **Scope:** always **personal + gitignored** via `.git/info/exclude` — never committed to the project's tracked `.github/`.
3. **Backend:** **adopt APM (microsoft/apm) as the engine**, pterm is the GUI on top; gitignore APM's materialized outputs since the use is personal/ephemeral.
4. **Granularity:** support **both** — searchable individual items **plus** optional named bundles/categories that expand to a set.
5. **Prompts → skills:** (still open) confirm in Phase 0 that modeling reusable prompts as **skills** is acceptable for the CLI.

### Implications of these decisions
- The core engine the plan describes is now mostly **APM configuration + a thin wrapper**, not a from-scratch resolver: lean on APM's `owner/repo/path#ref` sources, lockfile, integrity, and `compile -t`.
- pterm work narrows to **(a)** a search/preview palette over the collection, **(b)** a per-project "active kits" panel, **(c)** wiring `apm install/uninstall` (symlink/copy + `.git/info/exclude`) behind those, and **(d)** the launch-dialog "attach kits" hook.
- "Bundles" map naturally to **APM manifest groups** (a named manifest that pulls a set), so bundle support mostly comes for free from the backend choice.
- Phase 0 still gates everything: verify symlink-follow for skill dirs **and** `.agent.md`, the prompts→skills assumption, and that APM outputs can be fully hidden via `.git/info/exclude`.

---

## Phase 1 — tracer-bullet issues (vertical slices)

> **Filed:** these are live at **`surdy/ckit`** (private) — issues **#1–#8** (slices) + **#9** (epic). Repo: https://github.com/surdy/ckit · Epic: https://github.com/surdy/ckit/issues/9. GitHub issue numbers intentionally match the slice numbers below.

Working name for the standalone core engine: **`ckit`** (Copilot Kit CLI). Phase 1 = a manually-usable CLI that pulls/removes items from a **single local collection** into a project, gitignored, tracked by a lockfile.

**Slicing principle:** each issue is a *vertical* slice that runs the whole pipeline end-to-end —
`resolve → materialize → gitignore → record in lockfile → reflect back to the user` — rather than a
horizontal layer. Issue **#1 is the walking skeleton** (thinnest complete path) and establishes the
shared contracts below; **#2–#8 fan out from it and are mostly grabbable in parallel.**

### Shared contracts (defined and frozen by #1, so others can be picked up independently)
- **Collection layout:** local dir `KIT_COLLECTION_DIR` (default `~/.copilot-kit/collection`) containing
  `skills/<name>/SKILL.md` and `agents/<name>.agent.md`.
- **Lockfile:** `<project>/.copilot/kit.lock.json` (itself gitignored). Schema:
  `{ "version": 1, "items": [ { "id", "type": "skill|agent", "source": "local|<owner/repo/path>", "ref", "mode": "symlink|copy", "target": ".github/skills/<name>", "bundle"? } ] }`.
- **fs helpers:** `materialize(item, mode)`, `addExclude(path)` / `removeExclude(path)` operating on
  `.git/info/exclude` (idempotent, line-scoped).
- **CLI scaffold:** `ckit <cmd> [--project <dir>] [--json]`; `--project` defaults to git-root/cwd; stable exit codes.

### Dependency / parallelism map
| # | Issue | Depends on | Parallel-safe after #1 |
|---|---|---|---|
| 1 | Walking skeleton: `add` one skill | — | (foundation) |
| 2 | `rm` + `ls` (skills) | #1 | ✅ |
| 3 | Agents: `add`/`rm`/`ls` for `.agent.md` | #1 | ✅ |
| 4 | `search` the collection | #1 (config only) | ✅✅ (read-only) |
| 5 | `--copy` mode + Windows fallback | #1 | ✅ |
| 6 | Bundles: `add --bundle` | #1 (+#3 for mixed) | ✅ |
| 7 | `sync` / `doctor` reconcile | #1, #2 | ✅ |
| 8 | (stretch) APM-backed remote source | #1 | ✅ |

### Issue specs

**#1 — Walking skeleton: `ckit add <skill>` pulls one skill end-to-end**  _(foundation, no deps)_
The thinnest complete path from local collection to Copilot actually loading the skill.
- Resolve `<skill>` → `$KIT_COLLECTION_DIR/skills/<name>/`.
- Symlink it into `<project>/.github/skills/<name>`.
- Append the target to `.git/info/exclude`; also exclude `.copilot/kit.lock.json`.
- Create/update the lockfile with one entry.
- Define & document the shared contracts above.
Acceptance:
- After `ckit add <skill>`, launching Copilot in the project lists the skill in `<available_skills>` as
  `<location>project</location>` (the Phase-0-validated signal).
- `git status --porcelain` is empty afterwards.
- Re-running `add` is idempotent (no dup links / dup exclude lines / dup lockfile entries).
_Tracer rationale:_ exercises every layer once on the simplest item.

**#2 — Close the loop: `ckit rm <skill>` + `ckit ls`**  _(dep #1)_
- `rm`: unlink target, remove its `.git/info/exclude` line, drop the lockfile entry.
- `ls` / `status`: read lockfile → list installed items with health (target present? orphaned source?).
Acceptance: add→`ls` shows it; `rm`→`ls` empty; exclude + lockfile both clean; Copilot no longer sees it.

**#3 — Second primitive: agents `add`/`rm`/`ls` (`.agent.md`)**  _(dep #1)_
- `add <agent>` symlinks `agents/<name>.agent.md` into `.github/agents/`; gitignore; lockfile `type:agent`.
- `rm`/`ls` handle agents; `ls` shows the type column.
Acceptance: after add, the agent appears in Copilot's Task `agent_type` enum (Phase-0 signal); rm reverses cleanly.

**#4 — Discovery: `ckit search <query>`**  _(dep #1 config only; read-only — most independent)_
- Scan `skills/` + `agents/`, parse YAML frontmatter (`name`, `description`, `category`), fuzzy-rank.
- Human output `type  name  — description (category)`; `--json` for the future pterm palette.
Acceptance: partial term returns the right items ranked; empty query lists all; malformed/missing frontmatter degrades gracefully (warn, skip).

**#5 — Materialize modes: `--copy` + Windows auto-fallback**  _(dep #1)_
- `add --copy` copies instead of symlinking; auto-copy on Windows or when symlink creation fails (warn).
- Lockfile records `mode`; `rm` deletes copied dir/file; `status` flags copy drift (copy ≠ source).
Acceptance: `--copy` yields real files (not links) recorded `mode:copy`; rm cleans; simulated symlink failure falls back with a warning.

**#6 — Bundles: `ckit add --bundle <name>`**  _(dep #1, +#3 for mixed skill+agent bundles)_
- Bundle manifest `bundles/<name>.yml` in the collection lists item ids (skills and/or agents).
- `add --bundle frontend` materializes the whole set via the #1 add path; each lockfile entry tagged `bundle:frontend`.
- `rm --bundle frontend` removes exactly that set.
Acceptance: a 3-item mixed bundle installs all 3; `ls` groups by bundle; `rm --bundle` removes precisely those.

**#7 — Reconcile: `ckit sync` / `ckit doctor`**  _(dep #1, #2)_
- Detect: orphaned links (source gone), missing links (locked but absent), stale/missing exclude lines, copy drift.
- `sync` re-materializes from the lockfile; `doctor` / `status --verbose` reports health.
Acceptance: deleting a link then `sync` restores it; deleting a source then `doctor` flags the orphan; exclude lines reconciled to match the lockfile.

**#8 — (Stretch / Phase-3 bridge) APM-backed remote source behind `add`**  _(dep #1)_
Proves the "adopt APM as backend" decision on one real remote skill, reusing the spine.
- Accept item `source` as `owner/repo/path[#ref]`; `ckit add` resolves via APM (`apm install … --skill …`) or a
  cache fetch, then materializes through the SAME symlink/copy + gitignore + lockfile path (record `source`,`ref`).
- Outputs gitignored (personal/ephemeral).
Acceptance: `ckit add vercel-labs/agent-skills#<ref> --skill deploy-to-vercel` lands the skill in
`.github/skills`, gitignored, lockfile records source+ref, Copilot sees it; `rm` reverses. De-risks the
APM backend without rewriting the engine.

**#0 — (optional) Epic / tracker** linking #1–#8 with the dependency map above and the Phase-0 results.
