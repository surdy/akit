# ckit — Copilot Kit

A standalone, harness-agnostic CLI for **on-demand personal Copilot customizations**.

Keep your skills and custom agents in one central collection, then pull only the ones you
need into a project on demand — materialized into `.github/{skills,agents}` via **symlink**
(copy fallback), kept **personal + gitignored** (`.git/info/exclude`), and tracked by a
per-project **lockfile**. Remove them just as easily.

> Status/Usage: see [`docs/usage.md`](docs/usage.md). Design + plan in [`docs/design.md`](docs/design.md).
> GUI integration lives separately in [pterm](https://github.com/surdy/pterm) (Phase 2).

## Why

`~/.copilot/` is **user scope**, so every personal skill/agent is active in **every** project →
noise and context bloat. `ckit` moves the canonical collection out of the auto-discovered dir
and materializes only selected items per project.

## Validated foundation (Copilot CLI 1.0.62)

- Symlinked **skill dirs** under `.github/skills/<name>` are followed (load as `project` scope).
- Symlinked **`.agent.md`** under `.github/agents/` are followed (appear in the agent picker).
- `.git/info/exclude` fully hides pulled items — no repo pollution, no teammate breakage.
- The CLI has **no prompts primitive** → reusable prompts are modeled as **skills**.

See [`docs/design.md`](docs/design.md) for the full design, decisions, and Phase-0 evidence.

## Roadmap

- **Phase 1 — core engine MVP** (this repo): single local collection; `add`/`rm`/`ls`/`search`/
  `sync`; symlink-default/copy-fallback; auto-gitignore; lockfile. Scoped into tracer-bullet
  issues — see the [issues](../../issues).
- **Phase 2 — pterm GUI**: search palette, per-project "active kits" panel, launch-dialog hook.
- **Phase 3 — multiple sources / APM backend**: `owner/repo/path[#ref]` manifests.
- **Phase 4 — cross-harness**: Codex / Claude targets.

## Shared contracts (frozen by issue #1, the walking skeleton)

- **Collection layout:** `$KIT_COLLECTION_DIR` (default `~/.copilot-kit/collection`) with
  `skills/<name>/SKILL.md` and `agents/<name>.agent.md`.
- **Lockfile:** `<project>/.copilot/kit.lock.json` (gitignored):
  `{ "version": 1, "items": [ { "id", "type", "source", "ref", "mode", "target", "bundle"? } ] }`.
- **fs helpers:** `materialize(item, mode)`, `addExclude`/`removeExclude` on `.git/info/exclude`.
- **CLI scaffold:** `ckit <cmd> [--project <dir>] [--json]`; commands include `add`, `rm`,
  `ls`/`status`, and `search`.
