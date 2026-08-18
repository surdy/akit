# akit — agent kit

A standalone, harness-agnostic CLI for **on-demand personal agent customizations**.

Keep your skills and custom agents in one central catalog, or pull a remote
`owner/repo/path[#ref]` source through the git-fetch cache, then activate only the ones you
need in a project on demand — one at a time or as named bundles — kept **personal + gitignored**
(`.git/info/exclude`) and tracked by a per-project **lockfile**. Remove them just as easily.

akit has **one engine**, and it is harness-aware:

- **Project commands:** `install`/`uninstall`/`installed`/`reset`/`verify`/`status`/`doctor`/
  `sync`/`repair`/`detach`/`forget`/`adopt` materialize an item into **each** selected harness's
  own discovery paths — across **GitHub Copilot CLI, Claude Code, OpenAI Codex CLI, Gemini CLI,
  and OpenCode** — sharing a directory when several harnesses read the same one, tracked in
  `.akit/kit.lock.json`. `akit verify` checks which of the five are actually usable on the host.
  See [`docs/usage.md`](docs/usage.md#harness-aware-commands-the-akit-engine).
- **Catalog commands:** `ls`/`search`/`show` browse the catalog; `pull`/`update`/`restore`/
  `drop`/`log` populate and maintain it from remote `owner/repo/path[#ref]` sources.

> **Removed in v0.30.0:** the Copilot-only `add`/`rm` commands and the `.copilot/kit.lock.json`
> lockfile they used. Use `install`/`uninstall` over `.akit/kit.lock.json` instead.

> Status/Usage: see [`docs/usage.md`](docs/usage.md). Design + plan in [`docs/design.md`](docs/design.md).
> Embedding akit as a library: see [`docs/embedding.md`](docs/embedding.md).
> GUI integration lives separately in [pterm](https://github.com/surdy/pterm) (Phase 2).

## Why

`~/.copilot/` is **user scope**, so every personal skill/agent is active in **every** project →
noise and context bloat. `akit` moves the canonical catalog out of the auto-discovered dir
and materializes only selected items per project.

## Validated foundation (Copilot CLI 1.0.62)

- Symlinked **skill dirs** under `.github/skills/<name>` are followed (load as `project` scope).
- Symlinked **`.agent.md`** under `.github/agents/` are followed (appear in the agent picker).
- `.git/info/exclude` fully hides pulled items — no repo pollution, no teammate breakage.
- The CLI has **no prompts primitive** → reusable prompts are modeled as **skills**.

See [`docs/design.md`](docs/design.md) for the full design, decisions, and Phase-0 evidence.

## Roadmap

- **Phase 1 — core engine MVP** (shipped, then superseded by Phase 4): single local catalog;
  the original Copilot-only install/remove pair plus `ls`/`search`/`show`/`sync`/`doctor`/`pull`;
  symlink-default/copy-fallback; auto-gitignore; lockfile. Its `add`/`rm` commands and
  `.copilot/kit.lock.json` were removed in v0.30.0 — the harness-aware engine below is the only
  engine now. Scoped into tracer-bullet issues — see the [issues](../../issues).
- **Phase 2 — pterm GUI**: search palette, per-project "active kits" panel, launch-dialog hook.
- **Phase 3 — multiple sources / APM backend**: `owner/repo/path[#ref]` manifests. The current
  stretch implementation proves that source shape with a git-fetch cache, pending APM.
- **Phase 4 — cross-harness (shipped)**: the harness-aware `install`/`uninstall`/`installed`/
  `reset`/`verify` family targets Copilot, Claude, Codex, Gemini, and OpenCode, materializing
  into each harness's own discovery paths and tracking ownership in `.akit/kit.lock.json`.

## Shared contracts (frozen by issue #1, the walking skeleton)

- **Catalog layout:** `$KIT_CATALOG_DIR` (default `~/.akit/catalog`) with
  `skills/<name>/SKILL.md`, `agents/<id>/agent.yml` (a harness-aware **agent package**: a
  descriptor plus one native variant file per harness — the *only* agent shape),
  `bundles/<name>.yml`, and an `akit.yml` manifest of remotely-pulled items. The legacy flat
  `agents/<id>.agent.md` file was removed in v0.32.0 — see
  [migrating a flat agent](docs/usage.md#migrating-a-legacy-flat-agentsidagentmd).
- **Lockfile:** `<project>/.akit/kit.lock.json` (excluded via `.git/info/exclude`):
  `{ "version": 2, "items": [ { "id", "type", "source", "ref"?, "bundle"?, "harnesses",
  "materializations": [ { "path", "mode", "covers", "hash"? } ] } ] }`.
- **fs helpers:** `materialize_one`/`materialize_all` + `check_drift`, and
  `add_line`/`remove_line`/`set_managed_lines` on `.git/info/exclude`.
- **CLI scaffold:** `akit <cmd> [--project <dir>] [--json]`; commands are `install`
  (`[--agent] [-H <id>]... [--dry-run] [--symlink] [--bundle <name>] [--yes] [--force]`, taking a
  catalog id or a remote `owner/repo/path[#ref]` to pull-then-install), `uninstall`
  (`[--agent] [-H <id>]... [--bundle <name>]`), `installed` (list installs + health), `status`
  (project overview, bundle completeness), `doctor` (read-only diagnosis), `sync` (= `repair`),
  `repair`/`detach`/`forget`/`adopt` (ownership maintenance), `reset [--yes]`, `verify` (probe
  harness support on this host), `ls` (list the whole catalog; alias `catalog`), `search`,
  `show`, `pull` (fetch a remote source into the catalog), `drop` (remove an item from the
  catalog + prune its manifest entry if it was pulled), `update` (refresh pulled items to the
  latest upstream commit; `--check` to preview, or `<id> --to <sha>` to roll back / forward-pin
  to an exact commit), `log` (show a pulled item's upstream commit history, marking the
  installed commit), and `restore` (rebootstrap the catalog from `akit.yml`, pinning each item
  to its recorded commit; `--latest` to move to the head of its ref).
