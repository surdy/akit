# akit usage

`akit` pulls personal agent customizations (skills and custom agents) from a central
**catalog** into a project on demand, kept personal and gitignored, tracked by a lockfile.

The **harness-aware** commands
([`install`](#install--install-a-skill-or-agent-for-one-or-more-harnesses) /
[`uninstall`](#uninstall--remove-a-harness-aware-install) /
[`installed`](#installed--list-harness-aware-installs-and-their-health) /
[`status`](#status--harness-aware-project-overview) /
[`doctor`](#doctor--read-only-harness-aware-diagnosis) /
[`sync`](#sync--repair-safe-lockfilefilesystemexclude-drift) /
[`reset`](#reset--remove-every-harness-aware-install) /
[`verify`](#verify--check-harness-support-on-this-host) /
[`repair`/`detach`/`forget`/`adopt`](#repair--detach--forget--adopt--maintain-akit-ownership))
materialize into **each** selected harness's own discovery paths across Copilot, Claude Code,
Codex, Gemini, and OpenCode, tracked in `.akit/kit.lock.json` — see
[Harness-aware commands](#harness-aware-commands-the-akit-engine). The catalog commands
([`pull`](#pull--fetch-a-remote-source-into-the-catalog) /
[`restore`](#restore--rebootstrap-the-catalog-from-the-manifest) /
[`update`](#update--refresh-pulled-items-to-the-latest-upstream-commit) /
[`log`](#log--show-a-pulled-items-commit-history) / [`drop`](#drop--remove-an-item-from-the-catalog) /
[`ls`](#ls--list-everything-in-the-catalog) / [`search`](#search--search-the-catalog) /
[`show`](#show--preview-a-catalog-item)) populate and inspect the catalog itself.

> **Removed in v0.30.0:** the legacy `add`/`rm` commands and the `.copilot/kit.lock.json`
> lockfile they used. Use [`install`](#install--install-a-skill-or-agent-for-one-or-more-harnesses) /
> [`uninstall`](#uninstall--remove-a-harness-aware-install) instead — `install` also installs
> straight from a remote `owner/repo/path[#ref]` and supports `--bundle` and `--symlink`.

## Install / build

```bash
git clone https://github.com/surdy/akit.git
cd akit
cargo build --release
# binary at target/release/akit
```

## Your catalog

`akit` reads from a single local catalog directory:

- Location: `$KIT_CATALOG_DIR`, or `~/.akit/catalog` by default.
- Layout:

  ```text
  <catalog>/
    akit.yml                 # manifest of remotely-pulled items (for `akit restore`)
    skills/<name>/SKILL.md
    agents/<name>/agent.yml   # harness-aware agent package (+ one native file per harness)
    bundles/<name>.yml
  ```

Move your personal skills/agents here (out of `~/.copilot/`, which is auto-loaded in *every*
project). Skills are directories containing `SKILL.md`. An agent is always an **agent package**
`agents/<name>/` — an `agent.yml` plus one native variant file per harness — see
[Authoring an agent package](#authoring-an-agent-package) for the descriptor format. The
read/browse commands
([`ls`](#ls--list-everything-in-the-catalog) / [`search`](#search--search-the-catalog) /
[`show`](#show--preview-a-catalog-item)) surface packages; `akit` then materializes only the
ones you select into a given project with
[`install`](#install--install-a-skill-or-agent-for-one-or-more-harnesses).

### Migrating a legacy flat `agents/<id>.agent.md`

> **Removed in v0.32.0.** The flat single-file agent (`agents/<id>.agent.md`, Copilot-shaped)
> is no longer a catalog shape. It became uninstallable in v0.30.0 when the legacy `add`
> command was removed; it is now also invisible to
> [`ls`](#ls--list-everything-in-the-catalog) / [`search`](#search--search-the-catalog) /
> [`show`](#show--preview-a-catalog-item) and rejected by
> [`pull`](#pull--fetch-a-remote-source-into-the-catalog). The agent-package contract is the
> only agent contract.

A leftover flat file in your catalog is **ignored**, not deleted: nothing lists it, and `ls`
prints a one-line note to stderr naming the files it skipped. Convert each one:

```bash
cd ~/.akit/catalog/agents
mkdir reviewer
git mv reviewer.agent.md reviewer/copilot.agent.md   # the Copilot-native variant
$EDITOR reviewer/agent.yml
```

```yaml
# reviewer/agent.yml
name: Code Reviewer
description: Reviews a diff for correctness and style
variants:
  copilot: copilot.agent.md
```

The old file's body is already valid Copilot Markdown, so it becomes the `copilot:` variant
verbatim. Add a native file per additional harness you want to target (`claude.md`,
`codex.toml`, …) and list it under `variants:` — see
[Authoring an agent package](#authoring-an-agent-package) for the full schema.

If the agent had been **pulled**, its `akit.yml` entry still records the old `.agent.md` path.
[`restore`](#restore--rebootstrap-the-catalog-from-the-manifest) and
[`update`](#update--refresh-pulled-items-to-the-latest-upstream-commit) report such an entry as
a per-item **error** carrying this migration hint and carry on with the rest of the run; the
entry is left in the manifest so it can be fixed and retried. Either re-point it at a package
upstream (`akit pull --agent owner/repo/agents/<id>`) or forget it with
`akit drop --agent <id>`, which prunes the stale entry even though there is nothing on disk to
remove.

You can populate the catalog by hand (move/copy files into the layout above) or fetch a
remote source straight into it with [`akit pull`](#pull--fetch-a-remote-source-into-the-catalog).
Each `pull` records its source in a catalog manifest (`akit.yml`) so a new machine can be
rebootstrapped with [`akit restore`](#restore--rebootstrap-the-catalog-from-the-manifest).

Bundles are named YAML manifests that install a set of skills and agents together:

```yaml
skills: [deploy-to-vercel, lint-fix]
agents: [code-reviewer]
```

Either key may be omitted and is treated as an empty list. `install --bundle` validates every
referenced skill and agent before materializing anything; if an id is missing, the whole bundle
install fails.

## Authoring an agent package

An **agent package** is a directory `agents/<id>/` in the catalog holding an `agent.yml`
descriptor plus one *native* file per harness the agent supports. akit copies a variant's bytes
**verbatim** into that harness's destination — it never converts one harness's format into
another's — so each variant is authored in the harness's own format.

```text
<catalog>/agents/code-reviewer/
  agent.yml            # the descriptor (required)
  copilot.agent.md     # native variant files, named however you like
  claude.md
  codex.toml
```

### `agent.yml`

```yaml
# Display name shown by `show`. Optional — defaults to the package id
# (the directory name).
name: Code Reviewer

# One-line summary shown by `ls`, `search` and `show`. REQUIRED: a package
# with no `description` (or a blank one) fails to load.
description: Reviews a diff for correctness and style

# Free-text grouping used by search/preview. Optional, defaults to empty.
category: quality

# Destination filename *stem* for every harness. Optional — defaults to the
# package id. akit owns the directory and extension (see the table below), so
# this is a bare name: no `/`, no `\`, no `..`.
basename: code-reviewer

# REQUIRED: at least one entry. Maps a harness id to the variant file to copy,
# as a path relative to this package directory.
variants:
  copilot: copilot.agent.md
  claude: claude.md
  codex: codex.toml
```

The package **id** is the directory name — it is not read from `agent.yml` (an `id:` key there is
ignored). Variant keys must be one of the five supported harness ids: `copilot`, `claude`,
`codex`, `gemini`, `opencode`.

### Where each variant lands, and in what format

Only the *basename* comes from the package; the directory and extension come from the harness
registry:

| Harness | Agent destination | Format |
|---|---|---|
| copilot | `.github/agents/<basename>.agent.md` | Markdown + YAML |
| claude | `.claude/agents/<basename>.md` | Markdown + YAML |
| codex | `.codex/agents/<basename>.toml` | TOML |
| gemini | `.gemini/agents/<basename>.md` | Markdown + YAML |
| opencode | `.opencode/agent/<basename>.md` | Markdown + YAML |

Write each variant file in the destination format above — a `codex:` variant is TOML, the rest
are Markdown with YAML frontmatter. akit does not validate a variant's *contents*; it validates
the package's structure and copies bytes.

A package need not cover every harness. Installing for a harness the package has no variant for
is reported as a **skipped** issue rather than an error, and `ls` / `search` / `show` list the
harnesses each package actually supports.

### What makes a package invalid

`agent.yml` fails to load — and the agent becomes uninstallable — when:

- `agent.yml` is missing from the package directory, or is not parseable YAML.
- `variants` is absent or empty (an agent must provide at least one harness variant).
- `description` is missing, empty, or only whitespace.
- `basename` is empty or contains `/`, `\`, or `..`.
- A variant key is not a supported harness id.
- A variant path is absolute, empty, or escapes the package directory (`../…`).
- A variant points at a file that does not exist.

Invalid packages are **not** hidden. One bad package never breaks the rest of the catalog:
[`ls`](#ls--list-everything-in-the-catalog) still lists it with `disabled` in the HARNESSES
column (`"disabled": true` and an empty `harnesses` array under `--json`) and the load error as
its description:

```text
TYPE   ID             ORIGIN  HARNESSES  DESCRIPTION
agent  code-reviewer  local   disabled   invalid package: agent package 'code-reviewer' declares no description in …
```

Commands that act on the item — [`show`](#show--preview-a-catalog-item) and
[`install`](#install--install-a-skill-or-agent-for-one-or-more-harnesses) — fail with that same
message, so the defect is always reported rather than silently skipped.

## Global flags

| Flag | Meaning |
|---|---|
| `--project <dir>` | Target project (defaults to the enclosing git repo root, else the current dir). |
| `--json` | Emit machine-readable JSON instead of human text. |

## Commands

### `pull` — fetch a remote source into the catalog

```bash
akit pull [--agent] [--as <id>] [--force] owner/repo/path[#ref]
```

Where `install` materializes items *into a project*, `pull` copies a remote source *into your
local catalog* so it becomes a reusable item you can later `install`, `search`, and `show` like
any hand-authored kit. This is how you populate the catalog from shared repositories without
cloning and copying by hand. (To fetch and install in one step, pass the remote spec straight to
[`install`](#install--install-a-skill-or-agent-for-one-or-more-harnesses).)

- Fetches `owner/repo/path[#ref]` through the same git-fetch cache `install` uses for a remote
  source (honoring `$KIT_CACHE_DIR` and `$KIT_REMOTE_BASE_URL`), then **copies** the resolved item
  into the catalog — a standalone copy, independent of the cache.
- By default the source is a **skill** (`<catalog>/skills/<id>/`); with `--agent` it is an
  agent **package** — a directory `agents/<id>/` holding an `agent.yml`, stored at
  `<catalog>/agents/<id>/`. A source that resolves to a legacy flat `.agent.md` file is
  **rejected** with a migration hint (see
  [Migrating a legacy flat agent](#migrating-a-legacy-flat-agentsidagentmd)). The same path
  resolution as a remote `install` applies, so a single-segment `path` like `deploy-to-vercel`
  resolves to `skills/deploy-to-vercel` (or, with `--agent`, the
  `agents/deploy-to-vercel/` package) in the source repo.
- The catalog **id** defaults to the source's last path segment; `--as <id>` stores it under
  a different name. Ids must be a single path segment (no `/`).
- Validates the fetched source before writing: a skill must be a directory containing `SKILL.md`;
  an agent must be a valid package directory (`agent.yml` + declared variant files).
- Records an agent with its real package directory path and an explicit `type: agent` in the
  manifest, so [`restore`](#restore--rebootstrap-the-catalog-from-the-manifest) rebuilds the
  whole package.
- Creates the `skills/` / `agents/` directories if the catalog does not exist yet.
- **Idempotent and safe:** an identical existing item is a no-op (`"created": false`); an item
  that already exists and *differs* from the source is left untouched and the command errors
  unless you pass `--force` to overwrite it.
- Records the **resolved commit** (the SHA the ref pointed at) in the catalog manifest alongside
  the symbolic ref, so [`restore`](#restore--rebootstrap-the-catalog-from-the-manifest) is
  reproducible and [`update`](#update--refresh-pulled-items-to-the-latest-upstream-commit) can
  report precise `old → new` diffs.
- The global `--project` flag is accepted but unused — `pull` only touches the catalog.

With `--json`, `pull` emits a stable object:

```json
{
  "id": "deploy-to-vercel",
  "type": "skill",
  "source": "vercel-labs/agent-skills/deploy-to-vercel",
  "ref": "main",
  "path": "/home/you/.akit/catalog/skills/deploy-to-vercel",
  "created": true,
  "overwritten": false,
  "commit": "9f3c1a2e…"
}
```

`type` is `"skill"` or `"agent"`; `ref` is omitted when no `#ref` was supplied. `created` is
`false` when an identical copy was already present; `overwritten` is `true` only when `--force`
replaced a differing item. `commit` is the resolved SHA when it could be determined.

Example:

```bash
$ akit pull vercel-labs/agent-skills/deploy-to-vercel#main
Pulled skill 'deploy-to-vercel' from vercel-labs/agent-skills/deploy-to-vercel#main -> /home/you/.akit/catalog/skills/deploy-to-vercel (copied)

$ akit pull --agent acme/kits/agents/reviewer#main
Pulled agent 'reviewer' from acme/kits/agents/reviewer -> /home/you/.akit/catalog/agents/reviewer (copied)

$ akit pull --as vercel vercel-labs/agent-skills/deploy-to-vercel#main
Pulled skill 'vercel' from vercel-labs/agent-skills/deploy-to-vercel#main -> /home/you/.akit/catalog/skills/vercel (copied)
```

Once pulled, the item is just another catalog entry:

```bash
$ akit search deploy
skill  Deploy to Vercel  — Ship apps to Vercel (ops)
$ akit install deploy-to-vercel -H claude   # materialize it into a project
```

### `restore` — rebootstrap the catalog from the manifest

```text
akit restore [--force] [--latest]
```

Re-fetches every remotely-pulled item recorded in the catalog manifest (`akit.yml`), so you
can recreate your catalog on a new machine. Run it after copying just `akit.yml` to a fresh
`~/.akit/catalog/`:

```bash
$ akit restore
  pulled skill 'deploy-to-vercel' from vercel-labs/agent-skills/deploy-to-vercel#main
  pulled agent 'reviewer' from acme/kits/agents/reviewer#main
Restored 2 item(s): 2 pulled, 0 already present, 0 overwritten, 0 error(s).
```

- Each entry is re-pulled under its recorded id, so `--as` aliases are reproduced exactly.
- **Reproducible by default.** When an entry records a resolved `commit` (see below), `restore`
  checks out that exact commit rather than wherever the branch points now — two machines
  restored a week apart get the same content. Pass `--latest` to instead follow each item's
  symbolic ref to its newest commit and rewrite the recorded commit. Legacy entries without a
  recorded commit always follow the ref (and gain a recorded commit on the next restore).
- Items already present and identical are left untouched (idempotent). `--force` overwrites a
  catalog item that has drifted from its recorded source.
- A failed item does not abort the run; remaining items are still restored. `restore` exits
  non-zero if **any** item failed.
- The manifest only tracks remote pulls. Hand-authored skills/agents are your own content —
  keep those under version control yourself.

#### The manifest (`akit.yml`)

`pull` records each remote item in `<catalog>/akit.yml`, using the
[APM](https://github.com/microsoft/apm) manifest shape:

```yaml
name: akit-catalog
version: 0.0.0
dependencies:
  apm:
    - vercel-labs/agent-skills/lint-fix#main           # skill, no recorded commit (legacy form)
    - git: acme/kits                                   # agent package (explicit type)
      path: agents/reviewer
      type: agent
      ref: main
    - git: vercel-labs/agent-skills                    # skill with a resolved commit
      path: deploy-to-vercel
      ref: main
      commit: 9f3c1a2e…                                # exact commit `main` resolved to
    - git: acme/kits                                   # custom id via object form
      path: skills/deploy-to-vercel
      ref: main
      commit: 1b8d4c0f…
      alias: vercel
```

An entry is stored as the APM **string shorthand** `owner/repo/path[#ref]` only when it is a
skill with no recorded commit and the default id. As soon as a resolved **`commit`** is
recorded — which every `pull`/`update` does now — the entry switches to the **object form**
(`git` + `path` + `ref` + `commit`, plus `alias` for a `--as <id>` pull), because a single
string can't carry both the symbolic ref and the commit. An **agent** always uses the object
form: its package path carries no suffix to classify it by, so it records an explicit
`type: agent`. The loader still accepts the legacy string form, so older `akit.yml` files keep
working. Entries are upserted by `(type, id)`, and unknown keys (`name`, `author`, …) are
preserved across rewrites.

A path ending in `.agent.md` is still *read* as an agent — that is how a pre-v0.32.0 manifest
recorded a flat agent — but that shape no longer exists, so `restore`/`update` report the entry
as an error with a migration hint rather than misreading it as a skill. See
[Migrating a legacy flat agent](#migrating-a-legacy-flat-agentsidagentmd).

The recorded `commit` is what makes `restore` reproducible and `update` diffs precise; see those
commands for how it is consumed and refreshed.

With `--json`, `restore` emits a stable object:

```json
{
  "items": [
    {
      "id": "deploy-to-vercel",
      "type": "skill",
      "source": "vercel-labs/agent-skills/deploy-to-vercel",
      "ref": "main",
      "status": "pulled"
    }
  ],
  "summary": { "pulled": 1, "already_present": 0, "overwritten": 0, "errors": 0 }
}
```

`status` is one of `pulled`, `already-present`, `overwritten`, or `error`; failed items add an
`error` string.

### `update` — refresh pulled items to the latest upstream commit

```text
akit update [--check] [<id> [--agent]]
akit update <id> [--agent] --to <sha>
```

Re-fetches remotely-pulled catalog items and rewrites them to the **latest commit** of their
recorded ref (or the repository's default branch when the manifest records no ref). Where
[`restore`](#restore--rebootstrap-the-catalog-from-the-manifest) reuses the cached checkout to
recreate missing items, `update` always contacts the remote so it picks up upstream changes:

```bash
$ akit update
  updated skill 'deploy-to-vercel' from vercel-labs/agent-skills/deploy-to-vercel#main (9f3c1a2 → 4b7e0d1)
  up to date agent 'reviewer' from acme/kits/agents/reviewer#main
Updated 2 item(s): 1 updated, 1 up to date, 0 pinned, 0 error(s).
```

- With no `id`, every pulled item is considered; pass an `id` (add `--agent` for an agent) to
  update just one. An `id` that was never pulled is an error.
- `--check` reports what would change **without writing anything** — items show as `outdated`
  or `up to date`. Use it in scripts or before a bulk update.
- Items pinned to an immutable full commit **SHA** are reported as `pinned` and never refetched
  (a SHA can't move). Branch and tag refs are always re-checked.
- When an item advances, `update` rewrites the recorded `commit` in the manifest and shows the
  short `old → new` SHA. Legacy entries without a recorded commit gain one on the first update.
- Items sharing the same `owner/repo/ref` are fetched from the network only once.
- A failed item does not abort the run; `update` exits non-zero if **any** item failed.

With `--json`, `update` emits a stable object:

```json
{
  "items": [
    {
      "id": "deploy-to-vercel",
      "type": "skill",
      "source": "vercel-labs/agent-skills/deploy-to-vercel",
      "ref": "main",
      "status": "updated",
      "previous_commit": "9f3c1a2e…",
      "commit": "4b7e0d1a…"
    }
  ],
  "summary": { "updated": 1, "outdated": 0, "up_to_date": 0, "pinned": 0, "errors": 0 }
}
```

`status` is one of `updated`, `outdated`, `up-to-date`, `pinned`, or `error`; failed items add
an `error` string. `previous_commit`/`commit` are included when known.

#### Roll back (or forward-pin) with `--to <sha>`

Pass `--to <sha>` with an `<id>` to move a single item to an **exact commit** of its recorded ref
instead of following the ref forward. The `<sha>` may be a full SHA or an unambiguous prefix, and it
must be **reachable from the item's recorded ref** — an unknown or unreachable commit is rejected
(without touching the manifest) with a pointer to [`log`](#log--show-a-pulled-items-commit-history):

```bash
$ akit update deploy-to-vercel --to 9f3c1a2
  updated skill 'deploy-to-vercel' from vercel-labs/agent-skills/deploy-to-vercel#main (4b7e0d1 → 9f3c1a2)
Updated 1 item(s): 1 updated, 0 up to date, 0 pinned, 0 error(s).
```

The catalog copy is re-materialized at that commit and the manifest is **pinned to the resolved
full SHA** (its `ref` becomes the SHA). Because a full SHA is immutable, `update --check` then
reports the item as `pinned`; run `akit update <id>` against the branch again — or edit the manifest
`ref` — to resume tracking. `--to` cannot be combined with `--check`, and requires an `<id>`. The
`--json` output reuses the `update` object shape (`status`, `previous_commit`, `commit`).

### `log` — show a pulled item's commit history

```text
akit log <id> [--agent]
```

Lists the upstream commit history of a pulled catalog item for its recorded ref, newest first, and
marks (`*`) the commit currently recorded in the manifest (the installed one). History is read from
the git-fetch **cache's clone** — the manifest itself only ever records the current commit — so an
already-cached source lists offline; a cache miss fetches once (like `update`).

```bash
$ akit log deploy-to-vercel
* 4b7e0d1  2026-02-14  tighten deploy defaults
  9f3c1a2  2026-01-30  initial deploy-to-vercel skill
```

- `<id>` must be an item that was pulled and recorded in the manifest (add `--agent` for an agent);
  an id that was never pulled is an error.
- Pair it with [`update <id> --to <sha>`](#roll-back-or-forward-pin-with---to-sha) to roll back to
  any listed commit.

With `--json`, `log` emits an array, newest first:

```json
[
  {
    "commit": "4b7e0d1a…",
    "ref": "main",
    "date": "2026-02-14",
    "subject": "tighten deploy defaults",
    "current": true
  }
]
```

`current` is `true` for the commit recorded in the manifest; `ref` is the recorded symbolic ref
(omitted when the item tracks the default branch).

### `drop` — remove an item from the catalog

```text
akit drop [--agent] <id>
```

Removes a skill or agent from your catalog (`skills/<id>/`, or for an agent the whole
`agents/<id>/` package directory). If the
item was pulled, it also prunes its entry from the manifest, so
[`restore`](#restore--rebootstrap-the-catalog-from-the-manifest) won't bring it back. It's the
inverse of [`pull`](#pull--fetch-a-remote-source-into-the-catalog), but unlike the old behavior
it works on **both pulled and hand-authored (local)** items.

```bash
$ akit drop deploy-to-vercel
Dropped skill 'deploy-to-vercel' (from vercel-labs/agent-skills/deploy-to-vercel#main) -> /home/you/.akit/catalog/skills/deploy-to-vercel (removed)

$ akit drop --agent reviewer
Dropped agent 'reviewer' (from acme/kits/agents/reviewer#main) -> /home/you/.akit/catalog/agents/reviewer (removed)

$ akit drop my-local-skill
Dropped skill 'my-local-skill' -> /home/you/.akit/catalog/skills/my-local-skill (removed)
```

- Works on any catalog item. If `<id>` exists neither on disk nor in the manifest, `drop` errors
  and touches nothing.
- For a hand-authored (local) item there's no manifest entry to prune and no source to report.
- It still prunes the manifest entry when the files are already gone (reported as
  `manifest entry pruned; files were already absent`).
- The global `--project` flag is accepted but unused — `drop` only touches the catalog.

With `--json`, `drop` emits a stable object (`source`/`ref` appear only for pulled items;
`item_removed` is `false` when the files were already absent):

```json
{
  "id": "deploy-to-vercel",
  "type": "skill",
  "source": "vercel-labs/agent-skills/deploy-to-vercel",
  "ref": "main",
  "path": "/home/you/.akit/catalog/skills/deploy-to-vercel",
  "item_removed": true,
  "manifest_pruned": true
}
```

### `status` — harness-aware project overview

```bash
akit status
```

A bundle-grouped overview of everything installed in this project, read from the **harness-aware**
`.akit/kit.lock.json` (the same store as [`install`](#install--install-a-skill-or-agent-for-one-or-more-harnesses)
/ [`installed`](#installed--list-harness-aware-installs-and-their-health)). Rows are grouped by
bundle (`BUNDLE` column, standalone items last as `-`) and show the harnesses each item serves and
a per-item **HEALTH** value:

- `ok`: every serving harness has a clean covering materialization.
- `degraded (uncovered: …)`: some serving harness's materialization is missing or modified.
- `missing-source`: the catalog no longer provides this item's source.

Below the table, `status` prints one line per installed bundle summarizing its **completeness**
against the catalog `bundles/<name>.yml` manifest — using each install's `.akit` bundle tag:

- `complete`: every member the manifest declares is installed under that tag.
- `partial`: some declared members aren't installed (missing ids listed). This also surfaces a
  member re-installed standalone (which clears its bundle tag) or a manifest that *grew* upstream.
- `unknown`: the manifest could not be read; a stderr warning is written and the count is still shown.

Example:

```bash
$ akit status
BUNDLE  ID                TYPE   HARNESSES       HEALTH
web     deploy-to-vercel  skill  claude, codex   ok
web     code-reviewer     agent  claude          ok
-       deploy-helper     skill  claude          ok

Bundle 'web': partial (2/3) — missing: lint-fix
```

With `--json`, `status` emits `{ "items", "bundles" }`, where `items` are `reconcile::ItemHealth`
objects (the same shape [`installed`](#installed--list-harness-aware-installs-and-their-health)
emits, each with an optional `bundle` tag) and `bundles` are the completeness objects:

```json
{
  "items": [
    {
      "id": "deploy-to-vercel",
      "type": "skill",
      "source": "local",
      "harnesses": ["claude", "codex"],
      "materializations": [
        { "path": ".agents/skills/deploy-to-vercel", "mode": "copy", "covers": ["codex"], "drift": "clean" },
        { "path": ".claude/skills/deploy-to-vercel", "mode": "copy", "covers": ["claude"], "drift": "clean" }
      ],
      "bundle": "web",
      "source_present": true,
      "degraded": false
    }
  ],
  "bundles": [
    { "name": "web", "expected": 3, "installed": 2, "missing": ["lint-fix"], "state": "partial" }
  ]
}
```

`bundle` is omitted for standalone items. Each materialization's `drift` is `"clean"`, `"missing"`,
or `"modified"`; `mode` is `"copy"` or `"symlink"`. Each bundle's `state` is `"complete"`,
`"partial"`, or `"unknown"`; `expected` is omitted for `unknown`.

> **Breaking change (v0.27.0):** `status` reads the harness-aware `.akit/kit.lock.json`. Its
> `--json` `items[]` are `ItemHealth` (harness/materialization/drift), not the old
> `{ mode, target, status }` rows from the removed legacy `.copilot` lockfile.

> `status` lists what's **installed into the current project**. To list everything **available
> in your catalog**, use [`akit ls`](#ls--list-everything-in-the-catalog).

### `doctor` — read-only harness-aware diagnosis

```bash
akit doctor
```

A read-only diagnosis of the harness-aware `.akit/kit.lock.json` state — item drift, bundle
completeness, and git-exclude drift — without modifying anything. It is the richer companion to
[`status`](#status--harness-aware-project-overview) and the read-only counterpart of
[`sync`](#sync--repair-safe-lockfilefilesystemexclude-drift).

- Prints the same bundle-grouped item table and per-bundle completeness lines as `status`
  (HEALTH = `ok` / `degraded (uncovered: …)` / `missing-source`).
- Reports git-exclude drift in **both** directions: `missing` managed lines the lockfile requires
  (restore with `sync`) and `stale` lines with no owner (prune with `sync`).
- Verdict `Doctor: ok`, or a summary of what's wrong (`N degraded`, `N missing-source`, missing/stale
  exclude lines, `N partial bundle(s)`). A `partial` bundle is **informational** and does not by
  itself make the verdict non-ok.

Example:

```bash
$ akit doctor
BUNDLE  ID             TYPE   HARNESSES  HEALTH
-       deploy-helper  skill  claude     ok

Exclude: ok

Doctor: ok
```

With `--json`, `doctor` emits a `reconcile::Diagnosis`:

```json
{
  "items": [
    {
      "id": "deploy-helper",
      "type": "skill",
      "source": "local",
      "harnesses": ["claude"],
      "materializations": [
        { "path": ".claude/skills/deploy-helper", "mode": "copy", "covers": ["claude"], "drift": "clean" }
      ],
      "source_present": true,
      "degraded": false
    }
  ],
  "bundles": [],
  "stale_excludes": [],
  "missing_excludes": [],
  "lockfile_present": true,
  "healthy": true
}
```

`items` are the same `ItemHealth` objects [`status`](#status--harness-aware-project-overview) emits;
`bundles` is its completeness array; `healthy` is true when nothing drifts and the exclude block
matches the lockfile (partial bundles don't affect it).

> **Breaking change:** `doctor` now diagnoses the harness-aware `.akit/kit.lock.json` (previously the
> legacy `.copilot` lockfile). The `--json` shape changed from the old
> `{ items, bundles, exclude, summary }` `DoctorReport` to the `Diagnosis` above.

### `sync` — repair safe lockfile/filesystem/exclude drift

```bash
akit sync
```

Reconciles the project from the harness-aware `.akit/kit.lock.json`. It is **exactly equivalent to
[`repair`](#repair--detach--forget--adopt--maintain-akit-ownership)** (kept as the familiar name), and
idempotent — running it again after a clean sync is a no-op.

Repairs, touching only akit-owned paths:

- Re-materializes **missing** owned files from the current catalog source (using each
  materialization's recorded mode).
- Resyncs the managed `.git/info/exclude` block from the lockfile — restoring missing lines and
  pruning stale ones — including the `/.akit/kit.lock.json` line.

Does **not** overwrite user data: a **locally-modified** owned file is a conflict and is reported,
not overwritten; an item whose catalog source is gone is reported, not touched.

Example:

```bash
$ akit sync
Restored 1 missing file(s):
  .claude/skills/deploy-helper
```

With `--json`, `sync` emits a `reconcile::RepairReport` (identical to `repair`):

```json
{
  "restored_paths": [".claude/skills/deploy-helper"],
  "skipped_modified": [],
  "missing_source": []
}
```

> **Breaking change:** `sync` now repairs the harness-aware `.akit/kit.lock.json` (previously the
> legacy `.copilot` lockfile). Its `--json` is now the three-list `RepairReport` rather than the
> old `SyncReport` (`{ items, exclude, summary }`).

### `search` — search the catalog

```bash
akit search [<query>]
```

- Scans `<catalog>/skills/<name>/SKILL.md` and **agent packages**
  `<catalog>/agents/<name>/agent.yml`. Legacy flat `agents/<name>.agent.md` files are ignored
  (see [Migrating a legacy flat agent](#migrating-a-legacy-flat-agentsidagentmd)).
- Reads leading YAML-style frontmatter fields: `name`, `description`, and `category` (an agent
  package reads these from its `agent.yml`).
- If `name` is missing, uses the skill directory or package directory name.
- For an agent package, surfaces the **harnesses** it supports (from its variants).
- Fuzzy-matches `<query>` against `name` first and `description` second; best scores print first.
- An omitted or empty query lists every catalog item.
- Missing or malformed frontmatter emits a warning to stderr and falls back to available fields.
- Supports the global `--json` flag. The global `--project` flag is accepted but `search` reads
  only the catalog.

Human output is one hit per line:

```text
type  name  — description (category) [harnesses]
```

If `description` or `category` is empty, that part is omitted; `[harnesses]` appears only for
agent packages.

Example:

```bash
$ akit search deploy
skill  Deploy Helper  — Ship apps safely (ops)
agent  Code Reviewer  — Reviews PRs [copilot, claude]
```

Use `--json` with any command to emit machine-readable JSON.

For `search`, `--json` emits a stable array of objects:

```json
[
  {
    "type": "skill",
    "name": "Deploy Helper",
    "description": "Ship apps safely",
    "category": "ops",
    "score": 10087
  }
]
```

`type` is `"skill"` or `"agent"`. Missing `description` and `category` serialize as empty
strings. Empty-query results use score `0`. Agent packages add a `"harnesses"` array (their
supported harness ids); it is omitted for skills and legacy flat agents.

### `show` — preview a catalog item

```bash
akit show [--agent] <id>
```

- Reads a single item from the catalog and prints its frontmatter and raw content,
  without touching the project.
- Defaults to a skill (`<catalog>/skills/<id>/SKILL.md`); pass `--agent` to read an agent
  **package** (`<catalog>/agents/<id>/agent.yml`) — it previews the `agent.yml` descriptor and
  lists the harnesses it supports. A missing package is an error; a leftover flat
  `agents/<id>.agent.md` is not previewed.
- Reuses the same frontmatter parsing as `search` (`name`, `description`, `category`); a
  missing `name` falls back to the `<id>`, and malformed frontmatter warns to stderr and
  falls back to available fields.
- Exits non-zero with an error when the id or its markdown file is missing.
- Supports the global `--json` flag. The global `--project` flag is accepted but `show`
  reads only the catalog.

Human output is a header (`type · name · category`), the description and source path, then
the raw file content:

```text
$ akit show deploy-helper
skill · Deploy Helper · ops
Ship apps safely
/home/you/.akit/catalog/skills/deploy-helper/SKILL.md

---
name: Deploy Helper
description: Ship apps safely
category: ops
---
# Deploy Helper
...
```

For `show`, `--json` emits a stable object:

```json
{
  "type": "skill",
  "id": "deploy-helper",
  "name": "Deploy Helper",
  "description": "Ship apps safely",
  "category": "ops",
  "path": "/home/you/.akit/catalog/skills/deploy-helper/SKILL.md",
  "content": "---\nname: Deploy Helper\n...\n"
}
```

`type` is `"skill"` or `"agent"`. `name` falls back to `id`; missing `description` and
`category` serialize as empty strings. `path` is the absolute source path (an agent package's
`agent.yml`) and `content` is the full file (frontmatter included). An agent package adds a
`"harnesses"` array; it is omitted for skills and legacy flat agents.

> Remote-source and bundle-member preview are not yet supported — `show` reads local
> catalog items only.

### `ls` — list everything in the catalog

```bash
akit ls
# alias:
akit catalog
```

Lists every skill and agent in your catalog, with the **id** you pass to `install`, `show`, and
`drop`. Unlike [`search`](#search--search-the-catalog) (which fuzzy-matches and shows each
item's frontmatter `name`), `ls` is the catalog-wide inventory keyed by id, and it
records each item's provenance:

- `ls` (catalog scope) lists what's **available in your catalog**;
  [`status`](#status--harness-aware-project-overview) (project scope) lists what's **installed into the
  current project**.
- The `ORIGIN` column shows `owner/repo/path[#ref]` for items recorded as pulled in the
  manifest (`akit.yml`), or `local` for hand-authored items.
- The `HARNESSES` column shows the harnesses an **agent package** supports, `-` for skills and
  legacy flat agents, or `disabled` for an invalid package (its `DESCRIPTION` then carries the
  diagnostic — invalid packages stay visible rather than silently dropped).
- Sorted skills-first, then by id.
- Supports the global `--json` flag. The global `--project` flag is accepted but `ls`
  reads only the catalog.

Example:

```bash
$ akit ls
TYPE   ID             ORIGIN                          HARNESSES        DESCRIPTION
skill  deploy-helper  local                           -                Ship apps safely
skill  grill-me       mattpocock/skills/.../grill-me  -                Stress-test a plan
agent  legacy         local                           -                Review code (flat)
agent  reviewer       local                           copilot, claude  Reviews PRs
```

For `ls`, `--json` emits a stable array of objects:

```json
[
  {
    "type": "agent",
    "id": "reviewer",
    "description": "Reviews PRs",
    "harnesses": ["copilot", "claude"]
  }
]
```

`type` is `"skill"` or `"agent"`. `description` is the frontmatter description, or the load
diagnostic for an invalid package (empty when absent otherwise). `source` is present only for
pulled items; hand-authored (local) items omit it. `harnesses` (an agent package's supported
set) and `disabled` (`true` for an invalid package) are present only when they apply.

## Harness-aware commands (the `.akit` engine)

akit's install engine is **harness-aware**: it materializes an item into **each selected
harness's own discovery paths** — GitHub Copilot CLI, Claude Code, OpenAI Codex CLI, Gemini CLI,
and OpenCode — and tracks ownership in a local-only `.akit/kit.lock.json`. The project-facing
verbs below (`install`/`uninstall`/`installed`/`status`/`doctor`/`sync`/`reset`/`repair`/…) all
operate on this engine. (The old Copilot-only `add`/`rm` commands and their
`.copilot/kit.lock.json` were removed in v0.30.0.)

- **Skills** are portable `SKILL.md` directories, and several harnesses read the *same* project
  directory, so a single materialization can serve several harnesses. The planner runs a set
  cover over the registry's skill paths to minimize physical copies:

  | Path | Discovered by |
  |---|---|
  | `.agents/skills/<id>` | copilot, codex, gemini, opencode |
  | `.claude/skills/<id>` | copilot, claude, opencode |

  Installing for all five harnesses therefore needs exactly **two** directories
  (`.agents/skills` + `.claude/skills`). No shared skill path is symlink-verified end-to-end
  yet, so skills materialize as **copies**.

- **Custom agents** share nothing — every harness uses a proprietary directory, filename, and
  format — so an agent is materialized **once per harness** from an explicit native variant:

  | Harness | Agent destination | Format |
  |---|---|---|
  | copilot | `.github/agents/<id>.agent.md` | Markdown + YAML |
  | claude | `.claude/agents/<id>.md` | Markdown + YAML |
  | codex | `.codex/agents/<id>.toml` | TOML |
  | gemini | `.gemini/agents/<id>.md` | Markdown + YAML |
  | opencode | `.opencode/agent/<id>.md` | Markdown + YAML |

  OpenCode's directory is the **singular** `agent/`. Current OpenCode accepts either spelling, but
  the plural `agents/` was a hard error before v1.0.219, so the singular is the only form correct
  on every release that ever shipped markdown agents — see
  [`harness-registry.md`](harness-registry.md) for the source citations.

  Agents come from a catalog **agent package** — a directory `agents/<id>/` holding an
  `agent.yml` descriptor plus one native file per harness it supports. akit copies a variant's
  bytes **verbatim**; it never converts one format to another. A selected harness with no
  matching variant is reported as a **skipped** issue, not installed. See
  [Authoring an agent package](#authoring-an-agent-package) for the `agent.yml` format.

Everything the engine writes (both materializations and the `.akit/kit.lock.json` itself) is
added to `.git/info/exclude`, so it never touches your tracked `.gitignore` and `git status`
stays clean.

### Target harness selection

`install` needs a target harness set. It is resolved in this precedence order, first match wins:

1. Explicit `--harness`/`-H` flags (repeatable; each value may itself be a comma/space-separated
   list, e.g. `-H claude,codex`).
2. The `AKIT_HARNESSES` environment variable (a comma/space-separated list).
3. A project's `.akit/config.json` `harnesses` array.
4. An interactive picker (only when stdin is a terminal).

When none of these yields a harness **and** stdin is not a terminal, `install` fails with an
actionable message rather than hanging — pass `--harness`, set `AKIT_HARNESSES`, or add
`harnesses` to `.akit/config.json`. Unknown ids are rejected with the supported list.

`.akit/config.json` records per-project defaults:

```json
{ "harnesses": ["copilot", "claude"] }
```

Only the five supported ids are accepted (`copilot`, `claude`, `codex`, `gemini`, `opencode`);
an unknown id makes the config fail to load. `AKIT_HARNESSES` overrides the config, and explicit
flags override both:

```bash
export AKIT_HARNESSES="claude codex"
akit install deploy-to-vercel            # installs for claude + codex
akit install -H claude deploy-to-vercel  # flags win: claude only
```

### `install` — install a skill or agent for one or more harnesses

```bash
akit install [--agent] [-H <id>]... [--dry-run] [--symlink] <id>
akit install [--agent] [-H <id>]... [--force] [--symlink] <owner/repo/path[#ref]>
akit install [-H <id>]... [--dry-run] [--yes] [--symlink] --bundle <name>
```

Installs (or reshapes) catalog item `<id>` for exactly the resolved harness set. `install` is
**absolute**: it makes the item's installed harness set *exactly* the target set, adding
newly-needed materializations and removing now-unneeded ones. Adding or dropping a harness is
therefore just a re-install with the new set — re-running with a different `-H` set **reshapes**
the install (reported as `Reshaped` rather than `Installed`).

- Materializations are written as one atomic transaction: if any destination is occupied by a
  pre-existing **foreign** file (one akit doesn't own and whose bytes don't already match the
  source), the whole install is refused and nothing is written. A destination that already
  exists with byte-identical content is safely **adopted** (no rewrite).
- After a real install, `install` prints **reload guidance per served harness, per primitive** —
  one line per harness, drawn from the capability registry rather than a generic hint. Claude Code
  and Codex watch their directories and pick a new skill up live; Copilot CLI and Gemini need
  their in-session reload command (`/skills reload`); OpenCode caches its skill list and needs a
  restart. Agents differ from skills for the same harness (Copilot: skills reload by command,
  agents need a restart), which is exactly why the two are reported separately. Where no primary
  source establishes the behavior — Codex custom agents today — the line degrades to an honest
  "restart the harness if it does not appear". The per-cell evidence is in
  [`harness-registry.md`](harness-registry.md).
- Skipped harnesses (incompatible skill, missing agent variant) are listed under `skipped:` and
  simply not served.

```bash
$ akit install -H copilot -H claude deploy-to-vercel
Installed skill 'deploy-to-vercel' for copilot, claude
  .claude/skills/deploy-to-vercel  (copilot, claude)
reload:
  copilot skill: run the harness's reload command to pick it up this session
  claude skill: picked up automatically; no restart needed

$ akit install -H claude deploy-to-vercel
Reshaped skill 'deploy-to-vercel' for claude
  .claude/skills/deploy-to-vercel  (claude)
reload:
  claude skill: picked up automatically; no restart needed
```

#### `<owner/repo/path[#ref]>` — install straight from a remote

When `<id>` parses as a remote source (`owner/repo/path`, optionally `#ref`) instead of a plain
catalog id, `install` **pulls it into your catalog first, then installs it** — the one-step form of
`akit pull … && akit install <id>`. The pulled item is recorded in the catalog manifest (`akit.yml`)
exactly as `pull` would record it, so `update`/`restore`/`log` work on it afterwards. The install id
is the source's last path segment; use `--agent` for a
remote agent. `--force` re-pulls when the catalog already holds a **differing** copy of that id
(without it, a drifted copy is an error, matching `pull`). Because previewing would require fetching,
`--dry-run` is refused for a remote source — `pull` it, then `install --dry-run <id>`. To install a
remote agent it must be a **package** (`agents/<id>/` with `agent.yml`); a source resolving to a
single-file `.agent.md` is rejected with a migration hint.

```bash
$ akit install -H claude acme/kit-skills/deploy-to-vercel#main
Pulled skill 'deploy-to-vercel' from acme/kit-skills/deploy-to-vercel#main -> …/skills/deploy-to-vercel (copied)
Installed skill 'deploy-to-vercel' for claude
  .claude/skills/deploy-to-vercel  (claude)
reload:
  claude skill: picked up automatically; no restart needed
```

With `--json`, a remote `install` emits the same `InstallReport` as a local one (pull provenance is
in the catalog manifest / available via `akit pull --json`).

#### `--symlink` — symlink skills to the catalog instead of copying

By default skills are **copied** into each harness's discovery path. `--symlink` requests a live
symlink to the catalog source instead (`.claude/skills/deploy -> <catalog>/skills/deploy`), so
edits to the catalog are picked up without re-installing.

It is applied **per materialization, only where safe.** A skill directory is symlinked only when
*every* harness that materialization serves is a **confirmed symlink-follower** — currently Claude
and Codex. If a materialization is shared with any other harness (e.g. installing for
`copilot,claude` collapses onto the single `.claude/skills` path that also serves Copilot), that
path stays a **copy**, and `install` prints a `note:` saying which harness forced the copy. This
guarantees `--symlink` never leaves a harness with a symlink it can't discover. Two consequences:

- **Agents are always copied** — no harness is a confirmed follower for native agent files — so
  `--symlink --agent` is a no-op (with a note).
- On a transport that can't symlink (remote/SFTP embedding hosts), the request silently downgrades
  to a copy.
- Drift detection is weaker for symlinks: a symlinked skill is `clean` as long as the link exists;
  edits made *through* it (to the catalog source) are not flagged the way a modified copy is.

```bash
$ akit install --symlink -H claude deploy-to-vercel
Installed skill 'deploy-to-vercel' for claude
  .claude/skills/deploy-to-vercel  (claude)
reload:
  claude skill: picked up automatically; no restart needed

$ akit install --symlink -H copilot -H claude deploy-to-vercel
Installed skill 'deploy-to-vercel' for copilot, claude
  .claude/skills/deploy-to-vercel  (copilot, claude)
note: .claude/skills/deploy-to-vercel copied — symlink-following not confirmed for: copilot
```

`--dry-run --symlink` shows the resulting `[symlink]`/`[copy]` mode per planned materialization.

#### `--dry-run` — preview the plan

`--dry-run` computes the plan, diffs it against the current `.akit/kit.lock.json`, and prints
what *would* happen **without changing anything**: materializations to `create`, ones already
present and `unchanged`, paths a reshape would `remove`, and selected harnesses that would be
`skipped`.

```bash
$ akit install --dry-run -H claude deploy-to-vercel
Plan: skill 'deploy-to-vercel' for claude  (reshapes an existing install)
  create:
    .claude/skills/deploy-to-vercel  (claude)  [copy]
  remove (reshape):
    .agents/skills/deploy-to-vercel
(dry run — nothing changed; re-run without --dry-run to apply)
```

With `--json`, `--dry-run` emits the `InstallPreview` object:

```json
{
  "id": "deploy-to-vercel",
  "item_type": "skill",
  "harnesses": ["claude"],
  "create": [
    {
      "path": ".claude/skills/deploy-to-vercel",
      "mode": "copy",
      "covers": ["claude"],
      "kind": "skill_dir",
      "source_file": null
    }
  ],
  "unchanged": [],
  "remove": [".agents/skills/deploy-to-vercel"],
  "issues": [],
  "replaces": true
}
```

`item_type` is `"skill"` or `"agent"`; `mode` is `"copy"` or `"symlink"`; `kind` is `"skill_dir"`
or `"agent_file"`; `source_file` is the package-relative variant file for agents and `null` for
skills. `replaces` is `true` when an existing install would be reshaped. Each `issues` entry is
`{ "harness", "reason" }` where `reason` is `"skill_incompatible"`, `"no_agent_variant"`, or
`"needs_probe"`. `"needs_probe"` remains part of the contract but has no subject in the current
registry — every agent target's directory is now pinned with primary-source evidence.

A **real** `install` (no `--dry-run`) with `--json` emits the `InstallReport` object — the served
`harnesses`, the `materializations` now backing the install (each
`{ "path", "mode", "covers" }`, plus `"hash"` for copies), any `issues`, `replaced` (an existing
install was reshaped), and `not_a_git_repo`.

#### `--bundle <name>` — install a whole bundle, harness-aware

With `--bundle <name>` (in place of an `<id>`), `install` reads `<catalog>/bundles/<name>.yml` and
installs **every** listed skill and agent for the resolved harness set. Each member is planned and
materialized independently — the same harness-compatibility rules apply per member — so a member
that can't be served for a selected harness is **skipped** (listed under `skipped:`) rather than
failing the whole bundle. A missing manifest, or a member missing from the catalog, fails up front
before anything is installed. Each installed member is tagged with the bundle name in
`.akit/kit.lock.json`. `--bundle` cannot be combined with `--agent` or a positional `<id>`.

When the plan is **partial** — at least one member can't be served for every selected harness —
`install` prints the plan and asks for confirmation before applying. Pass `--yes` to skip the
prompt (required non-interactively, e.g. in CI). A fully-servable bundle installs with no prompt.
`--dry-run` prints the aggregated per-member plan and changes nothing.

```bash
$ akit install -H claude -H codex --bundle web
Plan: bundle 'web' for claude, codex
  skill 'deploy':
    create:
      .agents/skills/deploy  (codex)  [copy]
      .claude/skills/deploy  (claude)  [copy]
  skill 'clauded':
    create:
      .claude/skills/clauded  (claude)  [copy]
    skipped:
      codex: skill declares it is not compatible
(partial — some items can't be served for every selected harness)
Proceed with a partial install (skipping the items above)? [y/N] y
Installed bundle 'web' for claude, codex (2 item(s))
…
```

With `--json`, `--dry-run --bundle` emits a `BundleInstallPreview` (`{ "bundle", "harnesses",
"items": [InstallPreview…] }`) and a real `--bundle` install emits a `BundleInstallReport`
(`{ "bundle", "harnesses", "items": [InstallReport…] }`), each member object being exactly the
single-item shape documented above.

### `uninstall` — remove a harness-aware install

```bash
akit uninstall [--agent] [-H <id>]... <id>
akit uninstall [-H <id>]... --bundle <name>
```

- With **no** `-H`, fully uninstalls `<id>`: removes every materialization and drops the
  installation from `.akit/kit.lock.json`.
- With `-H`, removes only those harnesses and **reshapes** the rest — a shared path is kept as
  long as any remaining harness still needs it, and dropped once none do.
- Removing something that isn't installed exits successfully (`not_installed`).

```bash
$ akit uninstall deploy-to-vercel
Uninstalled skill 'deploy-to-vercel' (1 file(s) removed)

$ akit uninstall -H claude deploy-to-vercel
Removed skill 'deploy-to-vercel' from selected harness(es); still installed for copilot, codex
```

With `--json`, `uninstall` emits the `RemoveReport` object: `id`, `item_type`, `removed_paths`,
`remaining_harnesses` (empty on a full uninstall), and `not_installed`.

#### `--bundle <name>` — uninstall a whole bundle

With `--bundle <name>` (in place of an `<id>`), `uninstall` removes every install **tagged** with
that bundle in `.akit/kit.lock.json`. Like legacy `rm --bundle`, it's driven by the lockfile tag,
**not** the current manifest — a member dropped from `bundles/<name>.yml` after install is still
removed, and a member re-installed standalone (which clears its tag) is left alone. `-H` applies
the same scoped reshape to each member; no `-H` fully removes them. `--bundle` cannot be combined
with `--agent` or a positional `<id>`; an untagged/unknown bundle is a successful no-op.

```bash
$ akit uninstall --bundle web
Uninstalled bundle 'web' (2 item(s), 3 file(s) removed)
  skill 'deploy' — removed (2 file(s))
  skill 'lint' — removed (1 file(s))
```

With `--json`, it emits a `BundleRemoveReport` (`{ "bundle", "items": [RemoveReport…] }`), each
member being the single-item `RemoveReport` shape above.

### `installed` — list harness-aware installs and their health

```bash
akit installed
```

Lists every install recorded in `.akit/kit.lock.json`, one row per item with its type, the
harnesses it serves, and a per-item **HEALTH** value:

- `ok` — every selected harness is covered by a clean materialization.
- `degraded (uncovered: …)` — a materialization is missing or modified, leaving the listed
  harnesses without a clean covering copy.
- `missing-source` — the catalog no longer provides this item's source.

Below the table it lists any **stale exclude lines** (managed `.git/info/exclude` entries no
longer owned by any install) and an overall `Health:` line. `installed` needs a locatable
catalog (it reads sources to tell whether each install's source still exists).

```bash
$ akit installed
ID                           TYPE    HARNESSES                HEALTH
deploy-to-vercel             skill   copilot, claude          ok
reviewer                     agent   codex                    degraded (uncovered: codex)
Health: 1 degraded
```

With `--json`, `installed` emits the `HealthReport` object:

```json
{
  "items": [
    {
      "id": "deploy-to-vercel",
      "type": "skill",
      "source": "local",
      "harnesses": ["copilot", "claude"],
      "materializations": [
        {
          "path": ".claude/skills/deploy-to-vercel",
          "mode": "copy",
          "covers": ["copilot", "claude"],
          "drift": "clean"
        }
      ],
      "source_present": true,
      "degraded": false
    }
  ],
  "stale_excludes": [],
  "lockfile_present": true,
  "healthy": true
}
```

Each item's `type` is `"skill"`/`"agent"`; each materialization's `drift` is `"clean"`,
`"missing"`, or `"modified"`. `degraded` is `true` when a selected harness lacks a clean covering
materialization; `source_present` is `false` for the `missing-source` case. `healthy` is `true`
only when every item is clean and there are no stale excludes.

### `reset` — remove every harness-aware install

```bash
akit reset [--yes]
```

Removes **every akit-owned file** recorded in `.akit/kit.lock.json` and clears the lockfile
(which also removes the managed `.git/info/exclude` block). Only files akit recorded are touched —
unrelated files are never removed. It first lists the owned files it would delete, then requires
confirmation:

```bash
$ akit reset
Reset would remove these akit-owned files:
  .agents/skills/deploy-to-vercel
  .github/agents/reviewer.agent.md
Remove 2 akit-owned file(s) across 2 install(s)? [y/N] y
Reset complete — removed 2 file(s) across 2 install(s).
```

- `--yes` skips the prompt (for scripts).
- Without `--yes`, `reset` **refuses to run non-interactively** (no terminal) rather than
  destroying files unprompted — re-run with `--yes` to confirm.
- When nothing is recorded, it reports `Nothing to reset` and exits successfully.

With `--json`, `reset` emits the `ResetReport` object (`removed_paths`, `cleared_items`) and skips
the interactive preview/prompt.

### `verify` — check harness support on this host

```bash
akit verify
```

Probes each supported harness binary on the local host and combines the result with akit's static
capability registry to decide whether the harness is actually usable here. **No model/LLM is
involved:** "verified" means the binary is present, any known version gate is satisfied, and akit
statically supports at least one primitive (skill or agent) for it.

```bash
$ akit verify
✓ GitHub Copilot CLI verified on local (skills + agents)
✓ Claude Code verified on local (skills + agents)
✗ OpenAI Codex CLI: `codex` not found on local
```

With `--json`, `verify` emits an array of `HostVerification` objects (`harness`, `hostKey`,
`binary`, `present`, `version`, `minVersion`, `versionOk`, `skillSupported`, `agentSupported`,
`verified`, `detail` — camelCase keys). The same routine is what an embedding host runs against a
remote host over SSH before enabling kit support there.

### `repair` / `detach` / `forget` / `adopt` — maintain `.akit` ownership

Where [`installed`](#installed--list-harness-aware-installs-and-their-health) *reports* drift
read-only, these commands *act* on it. Every one operates strictly on the ownership recorded in
`.akit/kit.lock.json`: they never overwrite, delete, or claim unmanaged bytes without an exact
content match, so they are always safe to run.

```bash
akit repair
```

Re-materializes every **missing** akit-owned file from its catalog source and resyncs the managed
`.git/info/exclude` block (pruning stale lines). A locally **modified** copy is a conflict and is
left untouched; an item whose catalog source is gone is reported, not repaired. This is the
"put it back the way the lockfile says" command.

```bash
$ akit repair
Restored 1 missing file(s):
  .agents/skills/deploy-to-vercel
Skipped 1 locally-modified file(s) (not overwritten):
  .claude/agents/reviewer.md
```

With `--json`, `repair` emits a `RepairReport` (`restored_paths`, `skipped_modified`,
`missing_source`).

```bash
akit detach [--agent] <id>
```

Drops akit's ownership of an item while **keeping its files on disk**, and removes its managed
exclude lines so Git can now see them. Use this to "graduate" a materialized skill/agent into
tracked project files that you maintain yourself.

```bash
akit forget [--agent] <id>
```

Drops an **orphaned** ownership record (e.g. whose files you already deleted by hand) without
touching any files, then resyncs the exclude block. Reports cleanly when there is no such record.

```bash
akit adopt [--agent] [--harness <id> …] <id>
```

The inverse of a lost lockfile: claims **already-present, exact-content** files as akit-owned
without rewriting a byte — the safe recovery when `.akit/kit.lock.json` was deleted but the
materialized files still match the catalog. A destination that exists but differs is reported as a
conflict and never overwritten; an absent destination is simply not adopted (use `install`
instead). Target harnesses resolve exactly as they do for
[`install`](#target-harness-selection) (`--harness`/`-H` → `AKIT_HARNESSES` →
`.akit/config.json` → interactive picker).

```bash
$ akit adopt -H copilot deploy-to-vercel
Adopted skill 'deploy-to-vercel' for copilot:
  .agents/skills/deploy-to-vercel
```

All three of `detach`/`forget`/`adopt` accept `--json` (`detach`/`forget` emit a `DetachReport`
with `id`, `type`, `paths`, `not_installed`; `adopt` emits an `AdoptReport` with `harnesses`,
`adopted_paths`, `conflicts`).

## How it stays out of your repo

The harness-aware engine keeps materializations under each harness's discovery paths
(`.agents/skills`, `.claude/skills`, `.github/agents`, …) plus its `.akit/kit.lock.json` lockfile.
It adds every path it writes to `.git/info/exclude` (a local, untracked ignore list). Your tracked
`.gitignore` is never touched, and `git status` stays clean.
