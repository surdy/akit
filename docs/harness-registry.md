# Harness capability registry

`src/harness.rs` is the single source of truth for **how each supported CLI
harness discovers project-level customizations**. Every downstream stage — the
install planner, materialization, and the CLI/embedding surface — reads target
paths, coverage, symlink safety, versions, and reload guidance from here instead
of hardcoding `.github/...`.

This document is the **audit trail**: every registry cell below carries a
primary-source citation (`[C1]`-style references resolve in
[Sources](#sources)), so any claim can be re-checked against the vendor's own
docs, release notes, or source. Sources were last re-fetched **2026-08-17**
(issue #46, closing crit 12 of the #33 verification pass).

**Rules for this document**

- A citation is only listed if the page was actually fetched and the quoted text
  supports the *specific* claim in that row (path, format, reload, or version).
- Where docs and source disagree, both are cited and the row says which one the
  registry follows.
- Where no primary source establishes a behavior, the cell says `unknown` /
  `unverified` and carries **no** citation. An honest gap beats a fake one.

## Supported harnesses

| id         | label                | project skill dirs                            | project agent dir     |
|------------|----------------------|-----------------------------------------------|-----------------------|
| `copilot`  | GitHub Copilot CLI   | `.github/skills`, `.claude/skills`, `.agents/skills` [C1] | `.github/agents` [C4] |
| `claude`   | Claude Code          | `.claude/skills` [C7]                          | `.claude/agents` [C9] |
| `codex`    | OpenAI Codex CLI     | `.agents/skills`, `.codex/skills` [C11][C12]   | `.codex/agents` [C16] |
| `gemini`   | Gemini CLI           | `.gemini/skills`, `.agents/skills` [C19][C20]  | `.gemini/agents` [C22] |
| `opencode` | OpenCode             | `.opencode/skills`, `.claude/skills`, `.agents/skills` [C25][C26] | `.opencode/agent` [C28][C29] |

## Skills — path coverage

A `SKILL.md` directory placed under a path is discovered by **every** harness in
the *covers* column. This is what lets one materialization serve several
harnesses.

| path               | covers                            | plannable | evidence        | symlink     | citation   |
|--------------------|-----------------------------------|-----------|-----------------|-------------|------------|
| `.agents/skills`   | copilot, codex, gemini, opencode  | **yes**   | official docs   | unverified* | [C1][C11][C19][C25] |
| `.claude/skills`   | copilot, claude, opencode         | **yes**   | official docs   | unverified* | [C1][C7][C25] |
| `.github/skills`   | copilot                           | no        | official docs   | unverified  | [C1]       |
| `.codex/skills`    | codex                             | no        | official source | unverified  | [C12]      |
| `.gemini/skills`   | gemini                            | no        | official docs   | unverified  | [C19]      |
| `.opencode/skills` | opencode                          | no        | official docs   | unverified  | [C25]      |

\* Symlink-following for skills is **confirmed** for Claude [C7], Codex
([C13] docs + [C14] source) and OpenCode ([C27] source: every agent/skill glob
passes `symlink: true`, which maps to node-glob `follow`). It is **undetermined**
for Copilot and Gemini — no vendor statement, no source glob we could read. Since
both shared paths are read by Copilot, no shared path is symlink-safe end to end,
so **all shared paths materialize as copies**.

`HarnessId::follows_skill_symlink` deliberately stays the narrower
Claude-and-Codex gate: promoting OpenCode would change `install --symlink`
behavior, which is out of scope for the registry-data slice that added this
citation. The evidence is recorded here so that promotion is a one-line change
when someone wants it.

### Registered vs plannable paths

All six verified paths are **registered**, so the registry can answer "does
harness X read directory Y?" for any of them (crit 2). Only the two *shared*
paths are **plannable** — eligible as install destinations:

- **`.agents/skills`** — reaches copilot, codex, gemini, opencode (everyone but
  Claude). Preferred neutral path.
- **`.claude/skills`** — the *only* path that reaches Claude (Claude reads no
  compatibility alias [C7]), and also reaches copilot + opencode.

The other four are **coverage-redundant**: every harness that reads one also
reads a shared path, so planning into them would add a duplicate copy of the same
skill and nothing else. `SkillPath::plannable = false` keeps them out of the
planner's search space; `harness::planner_skill_paths()` is what the planner
iterates. This is asserted by
`harness::tests::redundant_aliases_serve_only_harnesses_a_shared_path_already_reaches`.

Two further real paths are **deliberately not registered**, because akit only
models *project*-level discovery: personal locations (`~/.copilot/skills`,
`~/.agents/skills`, `~/.claude/skills`, `~/.gemini/skills`,
`~/.config/opencode/skills`) and admin/system locations (`/etc/codex/skills`).
akit never writes outside the project.

### Set-cover invariant

No single directory covers all five harnesses (Claude only reads
`.claude/skills`). **Covering all five requires exactly two directories:**
`.agents/skills` + `.claude/skills`. This is asserted by
`harness::tests::claude_is_only_reachable_via_claude_skills` and
`every_harness_is_covered_by_some_skill_path`.

## Skills — reload and version, per harness

Reload behavior is a property of the **harness**, not of the shared directory:
Copilot CLI and Claude Code both read `.claude/skills` yet pick a new skill up
completely differently. So these facts live on `SkillSupport` (one entry per
harness), not on `SkillPath`, and post-install guidance reports them per served
harness.

| harness  | reload    | how                                                | min version | citation        |
|----------|-----------|----------------------------------------------------|-------------|-----------------|
| copilot  | `command` | `/skills reload` — "to avoid having to restart the CLI" | `0.0.401`   | [C2] / [C3]     |
| claude   | `live`    | watches the skill dirs, "picks up the change within the current session, without a restart" | `2.0.20` | [C8] / [C10] |
| codex    | `live`    | "Codex detects skill changes automatically" (watcher, ~10s throttle) | `0.94.0` | [C13][C15] / [C18] |
| gemini   | `command` | `/skills reload` (alias `/skills refresh`); no watcher | `0.28.0`  | [C19] / [C21]   |
| opencode | `restart` | no watcher; skills load once into a no-TTL instance cache | `1.0.186` | [C30] / [C31]   |

**What `min_version` means here:** the floor at which *every akit-plannable*
skill destination for that harness works. For the four harnesses akit reaches via
`.agents/skills`, that is the release which added `.agents/skills` support — not
the earlier release that first shipped skills. Gating on the earlier one would
claim support for a version that cannot read the directory akit actually writes.

Known-but-not-used earlier floors, for the record: Copilot CLI shipped skills
around `0.0.371` (2025-12-18) with `.github`/`.claude` paths [C3][C6]; Codex
shipped skills at `0.65.0` and repo-root `.codex/skills` at `0.66.0` [C18];
Gemini shipped skills at `0.24.0` [C21]; Claude Code's skill hot-reload arrived
in `2.1.0`, after skills themselves in `2.0.20` [C10].

**Caveats not modelled by the registry** (documented here so the `live` cells are
not read as stronger than they are):

- Claude Code's watcher only covers directories that **existed when the session
  started**, so akit's *first* skill install into a repo with no `.claude/skills`
  still needs a restart [C8].
- Gemini CLI skips workspace skills entirely in an **untrusted folder** [C20].
- OpenCode has an undocumented `SIGUSR2` reload path; restart is the only
  supported answer, so the registry records `restart` [C30].

**Gemini version numbers are source-inferred.** google-gemini/gemini-cli ships no
CHANGELOG and its release notes do not state "introduced in vX", so `0.28.0` and
`0.25.0` come from tag-content bisection — the defining symbol is absent at the
previous tag and present at this one [C21][C24]. Primary-source, but inferential;
flagged rather than presented as a vendor statement.

## Custom agents — native destinations

Custom agents are **never shared**: each harness uses a proprietary directory,
extension, and file format. akit copies the catalog's native variant bytes
verbatim — it never transforms one format into another.

| harness  | destination                   | format        | reload    | min version | symlink | evidence        | citation        |
|----------|-------------------------------|---------------|-----------|-------------|---------|-----------------|-----------------|
| copilot  | `.github/agents/<n>.agent.md` | markdown+yaml | restart   | `0.0.353`   | copy    | official docs   | [C4][C5] / [C3][C6] |
| claude   | `.claude/agents/<n>.md`       | markdown+yaml | live      | —           | copy    | official docs   | [C9]            |
| codex    | `.codex/agents/<n>.toml`      | toml          | **unknown** | `0.115.0` | copy    | official docs   | [C16][C17] / [C18] |
| gemini   | `.gemini/agents/<n>.md`       | markdown+yaml | command   | `0.25.0`    | copy    | official docs   | [C22][C23] / [C24] |
| opencode | `.opencode/agent/<n>.md`      | markdown+yaml | restart   | `0.3.65`    | copy    | official source | [C28][C29] / [C32] |

- **copilot** — "Restart the CLI to load your new custom agent." [C4]
- **claude** — "Claude Code watches `~/.claude/agents/` and `.claude/agents/` …
  the next delegation uses the updated definition, with no restart needed" [C9].
  Same first-write caveat as skills: a scope's first agent file in a brand-new
  `agents` directory needs a restart [C9]. No documented version floor.
- **codex** — reload is genuinely `unknown`: the subagent docs [C16] specify the
  directory and TOML schema but say nothing about in-session reload, and the only
  watcher in the source covers *skill* roots [C15]. Recorded as `Unknown` rather
  than guessed; the UI degrades to "restart the harness if it does not appear".
- **gemini** — `/agents reload` "Rescans agent directories … and reloads the
  registry" [C23]. `0.25.0` is the floor for the markdown+frontmatter format akit
  writes (it replaced an older TOML loader) [C24].
- **opencode** — see below.

### OpenCode `agent/` vs `agents/` — resolved, not probed

The target used to be flagged `needs_probe: true`, and nothing ever resolved it,
so OpenCode agents were unplannable (crit 5). Resolved in #46 by **pinning the
singular** `.opencode/agent`, on this evidence:

1. Current OpenCode globs **`{agent,agents}/**/*.md`** — both spellings work
   [C28][C29].
2. But the plural was a **hard error** (`ConfigDirectoryTypoError`) until
   **v1.0.219**, whose release note reads "Read plural resource types and stop
   erroring on them" [C33].
3. Therefore the singular `.opencode/agent` is the **only** spelling that is
   correct on every OpenCode release that ever shipped markdown agents (`0.3.65`
   onward [C32]).

Since no installed version exists for which a probe would return a different
answer than this pin, a runtime probe cannot improve on it — implementing one
would be code that can never change an outcome. `needs_probe` is now `false` and
the evidence is `official-source`: OpenCode's public docs list only the plural
[C26], while the source [C28][C29] and OpenCode's own bundled
`customize-opencode` skill [C34] both document the singular. **The registry
follows the source.**

The probe mechanism itself is *retained*: `AgentTarget::needs_probe`,
`AgentTarget::is_enabled()`, and `PlanIssueReason::NeedsProbe` all remain wired,
so the next version-sensitive ambiguity has a place to live. It simply has no
subject today, which
`harness::tests::no_agent_target_currently_needs_a_probe` asserts.

## Evidence model

A capability is only *enabled* when its behavior is backed by trusted
`Evidence`:

- `official-docs` — vendor documentation or vendor release notes.
- `official-source` — proven by reading the harness's OSS implementation.
- `live-verified` — proven by an isolated temporary-project behavioral test.
- `unverified` — **not** enabled; treated as disabled with a reason.

`Evidence::is_sufficient()` gates enablement; only `unverified` fails it.

Reload is modelled separately by `Reload`: `live` / `command` / `restart` /
`unknown`. `unknown` is a first-class honest answer — it renders as "restart the
harness if it does not appear" rather than inventing a behavior.

## Changing the matrix

When a harness ships a new discovery path or a behavior is verified:

1. Update the relevant `SkillPath` / `SkillSupport` / `AgentTarget` entry (and
   its `evidence`).
2. Only set `symlink_verified: true` when following is confirmed for **every**
   covering harness of that path.
3. Only clear `needs_probe` once the version-sensitive ambiguity is resolved —
   either by a real probe or, as with OpenCode, by proving one spelling is
   correct for every supported version.
4. Register a newly verified path even if it is coverage-redundant; mark it
   `plannable: false` so the planner is unaffected.
5. Add the citation to [Sources](#sources) below — a URL you fetched, plus the
   sentence that supports the claim. No citation, no `evidence` upgrade.
6. Keep `HarnessId::ALL` in stable registry order — the planner's tie-break
   depends on it.

## Sources

Fetched and confirmed 2026-08-17. OpenCode source links are permalinked to
commit `4e81a0b`; other source links track the default branch.

**GitHub Copilot CLI**

- `[C1]` <https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-skills>
  — "create a `.github/skills`, `.claude/skills`, or `.agents/skills` directory
  in your repository".
- `[C2]` same page — "if you have added a skill during a CLI session, you can add
  it using the command `/skills reload` to avoid having to restart the CLI".
- `[C3]` <https://raw.githubusercontent.com/github/copilot-cli/main/changelog.md>
  — `0.0.401 - 2026-02-03`: "Support `.agents/skills` directory for auto-loading
  skills"; `0.0.353 - 2025-10-28`: "Added support for custom agents … `.github/agents`".
- `[C4]` <https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/create-custom-agents-for-cli>
  — "Each custom agent is defined by a Markdown file with an `.agent.md`
  extension"; "Project (`.github/agents/`)"; "Restart the CLI to load your new
  custom agent."
- `[C5]` <https://docs.github.com/en/copilot/reference/custom-agents-configuration>
  — "Define the agent's behavior … below the YAML frontmatter."
- `[C6]` <https://github.blog/changelog/2025-12-18-github-copilot-now-supports-agent-skills/>
  — skills announcement; "If you've already set up skills for Claude Code in the
  `.claude/skills` directory … Copilot will pick them up automatically." States
  no version number, which is why the registry uses the changelog floor instead.

**Claude Code**

- `[C7]` <https://code.claude.com/docs/en/skills> — "Where skills live" table:
  Project = `.claude/skills/<skill-name>/SKILL.md`. Same page: "A `<skill-name>`
  entry … can be a symlink to a directory elsewhere on disk. Claude Code follows
  the symlink". The table enumerates every location and `.agents` appears
  nowhere — the basis for "Claude reads no compatibility alias".
- `[C8]` same page, "Live change detection" — "Claude Code watches skill
  directories for file changes … picks up the change within the current session,
  without a restart. If you create a top-level skills directory that didn't exist
  when the session started, restart Claude Code".
- `[C9]` <https://code.claude.com/docs/en/sub-agents> — "**Project subagents**
  (`.claude/agents/`)"; markdown + YAML frontmatter example; "Claude Code watches
  … the next delegation uses the updated definition, with no restart needed",
  plus the three restart cases.
- `[C10]` <https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md>
  — `2.0.20`: "Added support for Claude Skills"; `2.1.0`: "Added automatic skill
  hot-reload".

**OpenAI Codex CLI**

- `[C11]` <https://developers.openai.com/codex/skills> (redirects to
  <https://learn.chatgpt.com/docs/build-skills>) — "For repositories, Codex scans
  `.agents/skills` in every directory from your current working directory up to
  the repository root."
- `[C12]` <https://github.com/openai/codex/blob/main/codex-rs/ext/skills/src/host_roots.rs>
  — `AGENTS_DIR_NAME = ".agents"`; the Project config layer pushes
  `config_folder.join(SKILLS_DIR_NAME)`, i.e. `.codex/skills`. Undocumented in
  the public table, hence `official-source` for that row.
- `[C13]` <https://learn.chatgpt.com/docs/build-skills> — "Codex detects skill
  changes automatically. If an update doesn't appear, restart Codex." Also:
  "Codex supports symlinked skill folders and follows the symlink target".
- `[C14]` <https://github.com/openai/codex/blob/main/codex-rs/ext/skills/src/loader/host.rs>
  — `SkillScope::User | SkillScope::Repo | SkillScope::Admin => DirectorySymlinkPolicy::Follow`.
- `[C15]` <https://github.com/openai/codex/blob/main/codex-rs/app-server/src/skills_watcher.rs>
  — a real `FileWatcher` over skill roots, `WATCHER_THROTTLE_INTERVAL = 10s`.
  Scope note: this watcher covers **skills only**, which is why the Codex *agent*
  reload cell stays `unknown`.
- `[C16]` <https://developers.openai.com/codex/subagents.md> — "add standalone
  TOML files under `~/.codex/agents/` for personal agents or `.codex/agents/` for
  project-scoped agents"; required keys `name`, `description`,
  `developer_instructions`.
- `[C17]` <https://github.com/openai/codex/blob/main/codex-rs/core/src/config/agent_roles.rs>
  — `config_folder.join("agents")` and `extension == "toml"`.
- `[C18]` <https://github.com/openai/codex/releases/tag/rust-v0.94.0> — "Skills
  can be loaded from `.agents/skills`"; and
  <https://github.com/openai/codex/releases/tag/rust-v0.115.0> for `.codex/agents`
  directory auto-discovery.

**Gemini CLI**

- `[C19]` <https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/skills.md>
  — "**Workspace skills**: Located in `.gemini/skills/` or the `.agents/skills/`
  alias"; "`/skills reload` (or `/skills refresh`): Refreshes the list of
  discovered skills from all tiers."
- `[C20]` <https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/skills/skillManager.ts>
  — loads user/project skills plus the `.agents/skills` aliases; contains no
  watcher, and gates workspace skills on `isTrusted`. Path constants in
  <https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/config/storage.ts>
  (`AGENTS_DIR_NAME = '.agents'`).
- `[C21]` <https://github.com/google-gemini/gemini-cli/blob/v0.28.0/packages/core/src/config/storage.ts>
  — `getUserAgentSkillsDir`/`getProjectAgentSkillsDir` present at `v0.28.0` and
  absent at `v0.27.0`; `packages/core/src/skills/` first exists at `v0.24.0`.
  Tag-bisection, not a vendor statement — see the caveat above.
- `[C22]` <https://github.com/google-gemini/gemini-cli/blob/main/docs/core/subagents.md>
  — "Custom agents are defined as Markdown files (`.md`) with YAML frontmatter …
  **Project-level:** `.gemini/agents/*.md`". No `.agents/agents` alias exists.
- `[C23]` <https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/commands.md>
  — "**`reload`** (alias: `refresh`): Rescans agent directories (`~/.gemini/agents`
  and `.gemini/agents`) and reloads the registry."
- `[C24]` <https://github.com/google-gemini/gemini-cli/blob/v0.25.0/packages/core/src/agents/agentLoader.ts>
  — the markdown loader ("Supported extensions: .md") exists at `v0.25.0` and not
  at `v0.24.0`, where the loader was `toml-loader.ts`. Tag-bisection.

**OpenCode** (source permalinks pinned to `4e81a0b`)

- `[C25]` <https://opencode.ai/docs/skills/> — lists "Project config:
  `.opencode/skills/<name>/SKILL.md`", "Project Claude-compatible:
  `.claude/skills/<name>/SKILL.md`", "Project agent-compatible:
  `.agents/skills/<name>/SKILL.md`".
- `[C26]` <https://opencode.ai/docs/agents/> — documents only the **plural**
  "Per-project: `.opencode/agents/`". Cited as the docs side of the
  docs-vs-source disagreement.
- `[C27]` <https://github.com/sst/opencode/blob/4e81a0b73f6e614afebf9c7ff8862904a3674455/packages/core/src/util/glob.ts>
  — `follow: options.symlink ?? false`; the skill and agent scans in
  <https://github.com/sst/opencode/blob/4e81a0b73f6e614afebf9c7ff8862904a3674455/packages/opencode/src/skill/index.ts>
  pass `symlink: true`.
- `[C28]` <https://github.com/sst/opencode/blob/4e81a0b73f6e614afebf9c7ff8862904a3674455/packages/opencode/src/config/agent.ts>
  — `Glob.scan("{agent,agents}/**/*.md", { cwd: dir, absolute: true, dot: true, symlink: true })`.
- `[C29]` <https://github.com/sst/opencode/blob/4e81a0b73f6e614afebf9c7ff8862904a3674455/packages/core/src/config/plugin/agent.ts>
  — the same `{agent,agents}/**/*.md` glob, plus name derivation stripping
  `^(agent|agents|mode|modes)/`; the markdown **body** becomes the system prompt.
- `[C30]` <https://github.com/sst/opencode/blob/4e81a0b73f6e614afebf9c7ff8862904a3674455/packages/opencode/src/effect/instance-state.ts>
  — skills/config are memoised in a `ScopedCache` with
  `capacity: Number.POSITIVE_INFINITY` and no TTL; no filesystem watcher covers
  skill or agent discovery, and the only invalidation is an explicit teardown
  (`SIGUSR2` / API `configUpdate`). Basis for the `restart` cells; note the docs
  say nothing about reload at all.
- `[C31]` <https://github.com/sst/opencode/releases/tag/v1.0.186> — "Added Agent
  Skills support".
- `[C32]` <https://github.com/sst/opencode/releases/tag/v0.3.65> — first release
  containing markdown agents in `.opencode/agent/**/*.md`.
- `[C33]` <https://github.com/sst/opencode/releases/tag/v1.0.219> — "Read plural
  resource types and stop erroring on them" (before this, an `agents/` directory
  threw `ConfigDirectoryTypoError`).
- `[C34]` <https://github.com/sst/opencode/blob/4e81a0b73f6e614afebf9c7ff8862904a3674455/packages/core/src/plugin/skill/customize-opencode.md>
  — OpenCode's own bundled skill: "Project agents | `.opencode/agent/<name>.md`
  or `.opencode/agents/<name>.md`".
