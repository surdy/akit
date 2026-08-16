# akit — agent kit

A standalone, harness-agnostic CLI for **on-demand personal agent customizations**.

Keep your skills and custom agents in one central catalog, or pull a remote
`owner/repo/path[#ref]` source through the git-fetch cache, then activate only the ones you
need in a project on demand — one at a time or as named bundles — kept **personal + gitignored**
(`.git/info/exclude`) and tracked by a per-project **lockfile**. Remove them just as easily.

akit ships two command families:

- **Legacy (Copilot-shaped):** `add`/`rm`/`status`/`sync`/`doctor` materialize into
  `.github/{skills,agents}` via **symlink** or `akit add --copy` (with auto copy fallback on
  symlink failure), tracked in `.copilot/kit.lock.json`.
- **Harness-aware (shipped):** `install`/`uninstall`/`installed`/`reset`/`verify` materialize an
  item into **each** selected harness's own discovery paths — across **GitHub Copilot CLI,
  Claude Code, OpenAI Codex CLI, Gemini CLI, and OpenCode** — sharing a directory when several
  harnesses read the same one, tracked in `.akit/kit.lock.json`. `akit verify` checks which of
  the five are actually usable on the host. See [`docs/usage.md`](docs/usage.md#harness-aware-commands-the-akit-engine).

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

- **Phase 1 — core engine MVP** (this repo): single local catalog; `add`/`rm`/`ls`/`search`/
  `show`/`sync`/`doctor`/`pull`; symlink-default/copy-fallback; auto-gitignore; lockfile. Scoped into tracer-bullet
  issues — see the [issues](../../issues).
- **Phase 2 — pterm GUI**: search palette, per-project "active kits" panel, launch-dialog hook.
- **Phase 3 — multiple sources / APM backend**: `owner/repo/path[#ref]` manifests. The current
  stretch implementation proves that source shape with a git-fetch cache, pending APM.
- **Phase 4 — cross-harness (shipped)**: the harness-aware `install`/`uninstall`/`installed`/
  `reset`/`verify` family targets Copilot, Claude, Codex, Gemini, and OpenCode, materializing
  into each harness's own discovery paths and tracking ownership in `.akit/kit.lock.json`.

## Shared contracts (frozen by issue #1, the walking skeleton)

- **Catalog layout:** `$KIT_CATALOG_DIR` (default `~/.akit/catalog`) with
  `skills/<name>/SKILL.md`, `agents/<name>.agent.md`, `bundles/<name>.yml`, and an `akit.yml`
  manifest of remotely-pulled items.
- **Lockfile:** `<project>/.copilot/kit.lock.json` (gitignored):
  `{ "version": 1, "items": [ { "id", "type", "source", "ref", "mode", "target", "bundle"? } ] }`.
- **fs helpers:** `materialize(item, mode)`, `addExclude`/`removeExclude` on `.git/info/exclude`.
- **CLI scaffold:** `akit <cmd> [--project <dir>] [--json]`; commands include `add [--copy]`, `rm`,
  `add --bundle`, `rm --bundle`, `ls` (list the whole catalog), `status` (list items installed in
  the project), `search`, `show`, `sync`, `doctor`, `pull` (fetch a remote source into the
  catalog), `drop` (remove an item from the catalog + prune its manifest entry if it was
  pulled), `update` (refresh pulled items to the latest upstream commit; `--check` to preview,
  or `<id> --to <sha>` to roll back / forward-pin to an exact commit), `log` (show a pulled
  item's upstream commit history, marking the installed commit), and `restore` (rebootstrap
  the catalog from `akit.yml`, pinning each item to its recorded commit; `--latest` to move to
  the head of its ref).
