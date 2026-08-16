# akit usage

`akit` pulls personal agent customizations (skills and custom agents) from a central
**catalog** into a project on demand, kept personal and gitignored, tracked by a lockfile.

It exposes two command families. The **legacy** commands
([`add`](#add--pull-a-skill-or-agent-into-the-project) / [`rm`](#rm--remove-a-skill-or-agent-from-the-project)
/ [`status`](#status--list-installed-items) / [`sync`](#sync--repair-safe-lockfilefilesystemexclude-drift)
/ [`doctor`](#doctor--read-only-reconcile-report)) materialize into `.github/{skills,agents}` for
GitHub Copilot CLI. The newer **harness-aware** commands
([`install`](#install--install-a-skill-or-agent-for-one-or-more-harnesses) /
[`uninstall`](#uninstall--remove-a-harness-aware-install) /
[`installed`](#installed--list-harness-aware-installs-and-their-health) /
[`reset`](#reset--remove-every-harness-aware-install) /
[`verify`](#verify--check-harness-support-on-this-host) /
[`repair`/`detach`/`forget`/`adopt`](#repair--detach--forget--adopt--maintain-akit-ownership))
materialize into **each** selected harness's own discovery paths across Copilot, Claude Code,
Codex, Gemini, and OpenCode — see
[Harness-aware commands](#harness-aware-commands-the-akit-engine).

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
    agents/<name>.agent.md    # legacy flat agent (Copilot-shaped)
    agents/<name>/agent.yml   # harness-aware agent package (+ one native file per harness)
    bundles/<name>.yml
  ```

Move your personal skills/agents here (out of `~/.copilot/`, which is auto-loaded in *every*
project). Skills are directories containing `SKILL.md`. Agents come in two shapes: a legacy
single `agents/<name>.agent.md` file (used by [`add`](#add--pull-a-skill-or-agent-into-the-project),
Copilot-shaped), and a harness-aware **agent package** `agents/<name>/` (an `agent.yml` plus one
native variant file per harness, used by [`install --agent`](#install--install-a-skill-or-agent-for-one-or-more-harnesses)).
The read/browse commands ([`ls`](#ls--list-everything-in-the-catalog) /
[`search`](#search--search-the-catalog) / [`show`](#show--preview-a-catalog-item)) surface both,
preferring the package when an id exists in both shapes. `akit` then materializes only the ones
you select into a given project.

You can populate the catalog by hand (move/copy files into the layout above) or fetch a
remote source straight into it with [`akit pull`](#pull--fetch-a-remote-source-into-the-catalog).
Each `pull` records its source in a catalog manifest (`akit.yml`) so a new machine can be
rebootstrapped with [`akit restore`](#restore--rebootstrap-the-catalog-from-the-manifest).

Bundles are named YAML manifests that install a set of skills and agents together:

```yaml
skills: [deploy-to-vercel, lint-fix]
agents: [code-reviewer]
```

Either key may be omitted and is treated as an empty list. Bundle adds validate every referenced
skill and agent before materializing anything; if an id is missing, the whole bundle add fails.

## Global flags

| Flag | Meaning |
|---|---|
| `--project <dir>` | Target project (defaults to the enclosing git repo root, else the current dir). |
| `--json` | Emit machine-readable JSON instead of human text. |

## Commands

### `add` — pull a skill or agent into the project

```bash
akit add [--agent] [--copy] <name>
akit add [--agent] [--copy] owner/repo/path[#ref]
akit add [--copy] --bundle <name>
```

- By default, symlinks `<catalog>/skills/<name>` into `<project>/.github/skills/<name>`
  (Copilot loads it as a **project-scope** skill).
- With `--agent`, symlinks `<catalog>/agents/<name>.agent.md` into
  `<project>/.github/agents/<name>.agent.md`.
- With `--copy`, copies the source files instead of symlinking them and records `"mode": "copy"`
  in the lockfile and `--json` add report.
- If `<name>` contains `/`, `akit` treats it as a remote source spec instead of a local catalog
  name. The syntax is `owner/repo/path[#ref]`; `path` points at a skill directory containing
  `SKILL.md` (or, with `--agent`, a `.agent.md` file). For skill repositories with a top-level
  `skills/` directory, a single-segment path like `deploy-to-vercel` also resolves to
  `skills/deploy-to-vercel`. The installed id/target comes from the last path segment, so
  `vercel-labs/agent-skills/deploy-to-vercel#main` lands at `.github/skills/deploy-to-vercel`.
- Remote sources are fetched with `git` into a local cache, then materialized through the same
  symlink/copy pipeline as local items. The default cache is
  `~/.cache/akit/sources/<owner>/<repo>@<ref-or-default>`; `$XDG_CACHE_HOME` changes the cache base
  to `$XDG_CACHE_HOME/akit`, and `$KIT_CACHE_DIR` overrides it entirely. The CLI fetches from
  `https://github.com/<owner>/<repo>` by default; `$KIT_REMOTE_BASE_URL` can point at another git
  URL base (for example, a local `file://` mirror).
- Remote lockfile entries record `"source": "owner/repo/path"` and `"ref": "<ref>"` when a ref was
  supplied. The future intended backend is APM; the current git-fetch cache is the equivalent
  offline-friendly mechanism used today.
- With `--bundle <name>`, reads `<catalog>/bundles/<name>.yml` and adds every listed skill and
  agent through the same add pipeline. `--copy` applies to every item. `--agent` is not used with
  bundles because the manifest already distinguishes item types.
- If symlink creation fails at runtime (for example, Windows without symlink privilege), `akit`
  warns on stderr, falls back to copying, and records the effective `"mode": "copy"`.
- Appends the pull and the lockfile to `.git/info/exclude`, so nothing is committed and your
  teammates are unaffected. This applies to both local and remote pulls.
- Records the item in `<project>/.copilot/kit.lock.json`. Bundle-installed entries carry
  `"bundle": "<name>"`.
- Idempotent: re-running is a safe no-op.

Example:

```bash
$ akit add deploy-helper
Added skill 'deploy-helper' -> .github/skills/deploy-helper (linked)

$ akit add --agent reviewer
Added agent 'reviewer' -> .github/agents/reviewer.agent.md (linked)

$ akit add --copy deploy-helper
Added skill 'deploy-helper' -> .github/skills/deploy-helper (copied)

$ akit add vercel-labs/agent-skills/deploy-to-vercel#main
Added skill 'deploy-to-vercel' -> .github/skills/deploy-to-vercel (linked)

$ akit add --bundle web
Added bundle 'web' (3 items)
  Added skill 'deploy-to-vercel' -> .github/skills/deploy-to-vercel (linked)
  Added skill 'lint-fix' -> .github/skills/lint-fix (linked)
  Added agent 'code-reviewer' -> .github/agents/code-reviewer.agent.md (linked)
```

### `pull` — fetch a remote source into the catalog

```bash
akit pull [--agent] [--as <id>] [--force] owner/repo/path[#ref]
```

Where `add` materializes items *into a project*, `pull` copies a remote source *into your local
catalog* so it becomes a reusable item you can later `add`, `search`, and `show` like any
hand-authored kit. This is how you populate the catalog from shared repositories without
cloning and copying by hand.

- Fetches `owner/repo/path[#ref]` through the same git-fetch cache as `add` (honoring
  `$KIT_CACHE_DIR` and `$KIT_REMOTE_BASE_URL`), then **copies** the resolved item into the
  catalog — a standalone copy, independent of the cache.
- By default the source is a **skill** (`<catalog>/skills/<id>/`); with `--agent` it is an
  agent. An agent may be either a harness-aware **package** — a directory `agents/<id>/` holding
  an `agent.yml` (stored at `<catalog>/agents/<id>/`) — or a legacy flat `.agent.md` file
  (stored at `<catalog>/agents/<id>.agent.md`); `pull` detects which the source is and stores it
  in the matching shape. The same path resolution as `add` applies, so a single-segment `path`
  like `deploy-to-vercel` resolves to `skills/deploy-to-vercel` (or, with `--agent`, an
  `agents/deploy-to-vercel/` package if present, else `agents/deploy-to-vercel.agent.md`) in the
  source repo.
- The catalog **id** defaults to the source's last path segment; `--as <id>` stores it under
  a different name. Ids must be a single path segment (no `/`).
- Validates the fetched source before writing: a skill must be a directory containing `SKILL.md`;
  an agent must be either a valid package directory (`agent.yml` + declared variant files) or a
  `.agent.md` file.
- Records an agent package with its real directory path and an explicit `type: agent` in the
  manifest (flat agents keep the `.agent.md` shorthand), so
  [`restore`](#restore--rebootstrap-the-catalog-from-the-manifest) rebuilds the whole package.
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
$ akit add deploy-to-vercel   # materialize it into a project
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
  pulled agent 'reviewer' from acme/kits/reviewer.agent.md#main
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
    - acme/kits/reviewer.agent.md#main                 # agent, no recorded commit (legacy form)
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

An entry is stored as the APM **string shorthand** `owner/repo/path[#ref]` (agents use the
`.agent.md` extension, APM's file-primitive convention) only when it has no recorded commit and
the default id. As soon as a resolved **`commit`** is recorded — which every `pull`/`update`
does now — the entry switches to the **object form** (`git` + `path` + `ref` + `commit`, plus
`alias` for a `--as <id>` pull), because a single string can't carry both the symbolic ref and
the commit. The loader still accepts the legacy string form, so older `akit.yml` files keep
working. Entries are upserted by `(type, id)`, and unknown keys (`name`, `author`, …) are
preserved across rewrites. `restore` classifies an entry as an agent when its path ends in
`.agent.md`, otherwise a skill.

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
  up to date agent 'reviewer' from acme/kits/reviewer.agent.md#main
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
`agents/<id>/` package directory when present, else the legacy flat `agents/<id>.agent.md`). If the
item was pulled, it also prunes its entry from the manifest, so
[`restore`](#restore--rebootstrap-the-catalog-from-the-manifest) won't bring it back. It's the
inverse of [`pull`](#pull--fetch-a-remote-source-into-the-catalog), but unlike the old behavior
it works on **both pulled and hand-authored (local)** items.

```bash
$ akit drop deploy-to-vercel
Dropped skill 'deploy-to-vercel' (from vercel-labs/agent-skills/deploy-to-vercel#main) -> /home/you/.akit/catalog/skills/deploy-to-vercel (removed)

$ akit drop --agent reviewer
Dropped agent 'reviewer' (from acme/kits/reviewer.agent.md#main) -> /home/you/.akit/catalog/agents/reviewer.agent.md (removed)

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

### `rm` — remove a skill or agent from the project

```bash
akit rm [--agent] <name>
akit rm --bundle <name>
```

- Removes the materialized target from `.github/skills/` or `.github/agents/`.
- Removes that target's `.git/info/exclude` line.
- Removes the lockfile entry.
- Remote items are removed by their installed id (the source path leaf), so a remote add of
  `owner/repo/deploy-to-vercel#main` is reversed with `akit rm deploy-to-vercel`.
- With `--bundle <name>`, removes exactly the installed lockfile entries tagged with that bundle.
  The current manifest is not consulted, so removal stays precise even if the manifest changed.
- Idempotent: removing an item that is not installed exits successfully.

Example:

```bash
$ akit rm deploy-helper
Removed skill 'deploy-helper' -> .github/skills/deploy-helper (removed)

$ akit rm --agent reviewer
Removed agent 'reviewer' -> .github/agents/reviewer.agent.md (removed)

$ akit rm --bundle web
Removed bundle 'web' (3 items)
  Removed skill 'deploy-to-vercel' -> .github/skills/deploy-to-vercel (removed)
  Removed skill 'lint-fix' -> .github/skills/lint-fix (removed)
  Removed agent 'code-reviewer' -> .github/agents/code-reviewer.agent.md (removed)
```

### `status` — list installed items

```bash
akit status
```

Lists lockfile entries grouped by bundle and labeled in the `BUNDLE` column. Standalone entries
show `-`. Health values:

- `ok`: target exists and, for symlinks, resolves to an existing source.
- `orphaned`: target is a symlink whose source no longer exists.
- `missing`: lockfile entry exists but the target is gone.
- `drifted`: copy-mode target exists, but its content differs from the current catalog source.

Below the table, `status` prints one line per installed bundle summarizing its **completeness**
against the catalog `bundles/<name>.yml` manifest:

- `complete`: every member the manifest declares is installed.
- `partial`: some declared members are not installed (the missing ids are listed). This also
  surfaces a bundle whose manifest *grew* upstream after you installed it — reporting never
  mutates the lockfile.
- `unknown`: the manifest could not be read (absent, unparseable, or no catalog is available); a
  warning is written to stderr and the installed count is still reported.

Example:

```bash
$ akit status
BUNDLE  TYPE   ID                MODE     TARGET                                      STATUS
web     skill  deploy-to-vercel  symlink  .github/skills/deploy-to-vercel             ok
web     agent  code-reviewer     symlink  .github/agents/code-reviewer.agent.md       ok
-       skill  deploy-helper     symlink  .github/skills/deploy-helper                ok

Bundle 'web': partial (2/3) — missing: lint-fix
```

With `--json`, `status` emits an object with `items` and `bundles`:

```json
{
  "items": [
    {
      "id": "deploy-to-vercel",
      "type": "skill",
      "mode": "symlink",
      "target": ".github/skills/deploy-to-vercel",
      "bundle": "web",
      "status": "ok"
    }
  ],
  "bundles": [
    {
      "name": "web",
      "expected": 3,
      "installed": 2,
      "missing": ["lint-fix"],
      "state": "partial"
    }
  ]
}
```

Each item's `status` is lowercase (`"ok"`, `"orphaned"`, `"missing"`, or `"drifted"`), `mode`
is `"symlink"` or `"copy"`, and `bundle` is `null` for standalone items. Each bundle's `state`
is `"complete"`, `"partial"`, or `"unknown"`; `missing` is empty except for `partial`; and
`expected` is omitted for `unknown` (the manifest count is unknown).

> **Note:** prior to this the `status --json` output was a bare array of items. It is now the
> `{ "items", "bundles" }` object shown above.

> `status` lists what's **installed into the current project**. To list everything **available
> in your catalog**, use [`akit ls`](#ls--list-everything-in-the-catalog).

### `doctor` — read-only reconcile report

```bash
akit doctor
```

Checks the lockfile against the project filesystem, the current catalog, and
`.git/info/exclude` without modifying anything.

- Reports each lockfile item as `ok`, `orphaned`, `missing`, or `drifted`.
- Shows whether the catalog source exists, the project target exists, and the target's
  `/.github/...` exclude line is present.
- Reports missing managed exclude lines, including `/.copilot/kit.lock.json`.
- Flags stale managed exclude lines (for example, a `/.github/skills/...` line with no matching
  lockfile entry) but does not remove them.
- Prints the same per-bundle completeness lines as [`status`](#status--list-installed-items)
  (`complete` / `partial` / `unknown`). A `partial` bundle is **informational** — its missing
  members were simply never installed — so it does not flip overall `Health`.

Example:

```bash
$ akit doctor
BUNDLE  TYPE   ID             MODE     TARGET                                STATUS    EXCLUDE
-       skill  deploy-helper  symlink  .github/skills/deploy-helper          ok        present
Exclude: ok
Health: ok
```

With `--json`, `doctor` emits:

```json
{
  "items": [
    {
      "id": "deploy-helper",
      "type": "skill",
      "mode": "symlink",
      "target": ".github/skills/deploy-helper",
      "bundle": null,
      "status": "ok",
      "source_present": true,
      "target_present": true,
      "exclude_present": true
    }
  ],
  "bundles": [],
  "exclude": {
    "checked": true,
    "path": "<project>/.git/info/exclude",
    "lockfile_present": true,
    "missing": [],
    "stale": []
  },
  "summary": {
    "total": 1,
    "ok": 1,
    "orphaned": 0,
    "missing": 0,
    "drifted": 0,
    "missing_exclude_lines": 0,
    "stale_exclude_lines": 0,
    "partial_bundles": 0,
    "not_a_git_repo": false,
    "healthy": true
  }
}
```

The top-level `bundles` array has the same shape as [`status`](#status--list-installed-items)'s,
and `summary.partial_bundles` counts bundles in the `partial` state.

### `sync` — repair safe lockfile/filesystem/exclude drift

```bash
akit sync
```

Reconciles the project from the lockfile. It is idempotent: running it again after a clean sync is a
no-op.

Repairs:

- Missing materialized targets, using the recorded `mode` (`symlink` or `copy`) and the current
  catalog source.
- Missing `.git/info/exclude` lines for locked targets.
- The lockfile's own `/.copilot/kit.lock.json` exclude line.

Does **not** silently delete or overwrite user data:

- Orphaned items whose catalog source is gone are reported and skipped.
- Drifted copy-mode targets are reported and not overwritten.
- Stale exclude lines are reported and not removed.

Example:

```bash
$ akit sync
Restored skill 'deploy-helper' -> .github/skills/deploy-helper (symlink)
Added exclude /.copilot/kit.lock.json
```

With `--json`, `sync` emits:

```json
{
  "items": [
    {
      "id": "deploy-helper",
      "type": "skill",
      "mode": "symlink",
      "target": ".github/skills/deploy-helper",
      "bundle": null,
      "status_before": "missing",
      "status_after": "ok",
      "source_present": true,
      "restored": true,
      "exclude_added": false,
      "skipped_orphan": false,
      "drifted": false
    }
  ],
  "exclude": {
    "checked": true,
    "path": "<project>/.git/info/exclude",
    "lockfile_added": true,
    "target_lines_added": [],
    "missing_after": [],
    "stale": []
  },
  "summary": {
    "total": 1,
    "restored": 1,
    "exclude_added": 1,
    "skipped_orphan": 0,
    "drifted": 0,
    "missing_after": 0,
    "missing_exclude_lines": 0,
    "stale_exclude_lines": 0,
    "not_a_git_repo": false,
    "healthy": true
  }
}
```

### `search` — search the catalog

```bash
akit search [<query>]
```

- Scans `<catalog>/skills/<name>/SKILL.md`, legacy flat agents `<catalog>/agents/<name>.agent.md`,
  and harness-aware **agent packages** `<catalog>/agents/<name>/agent.yml`. When both an agent
  package and a flat file share an id, the package wins (it is the target contract).
- Reads leading YAML-style frontmatter fields: `name`, `description`, and `category` (an agent
  package reads these from its `agent.yml`).
- If `name` is missing, uses the skill directory or agent file name.
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
- Defaults to a skill (`<catalog>/skills/<id>/SKILL.md`); pass `--agent` to read an agent.
  For agents, a harness-aware **package** (`<catalog>/agents/<id>/agent.yml`) is preferred when
  present — it previews the `agent.yml` descriptor and lists the harnesses it supports — falling
  back to a legacy flat `<catalog>/agents/<id>.agent.md` otherwise.
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

Lists every skill and agent in your catalog, with the **id** you pass to `add`, `show`, and
`drop`. Unlike [`search`](#search--search-the-catalog) (which fuzzy-matches and shows each
item's frontmatter `name`), `ls` is the catalog-wide inventory keyed by id, and it
records each item's provenance:

- `ls` (catalog scope) lists what's **available in your catalog**;
  [`status`](#status--list-installed-items) (project scope) lists what's **installed into the
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

The commands above (`add`/`rm`/`status`/`sync`/`doctor`) are the **legacy, Copilot-shaped**
family: they materialize into `.github/{skills,agents}` and track ownership in
`.copilot/kit.lock.json`. Alongside them, akit ships a **harness-aware install engine** that
materializes an item into **each selected harness's own discovery paths** — GitHub Copilot CLI,
Claude Code, OpenAI Codex CLI, Gemini CLI, and OpenCode — and tracks that in a separate,
local-only `.akit/kit.lock.json`.

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
  | opencode | `.opencode/agent/<id>.md` (probe-gated) | Markdown + YAML |

  Harness-aware agents come from a catalog **agent package** — a directory `agents/<id>/`
  holding an `agent.yml` descriptor plus one native file per harness it supports. akit copies a
  variant's bytes **verbatim**; it never converts one format to another. (This is a distinct
  catalog shape from the legacy `agents/<id>.agent.md` single file that
  [`add`](#add--pull-a-skill-or-agent-into-the-project) / [`pull`](#pull--fetch-a-remote-source-into-the-catalog)
  use.) A selected harness with no matching variant — or OpenCode's probe-gated target, whose
  exact directory is version-dependent — is reported as a **skipped** issue, not installed.

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
akit install [--agent] [-H <id>]... [--dry-run] <id>
akit install [-H <id>]... [--dry-run] [--yes] --bundle <name>
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
- After a real install, `install` prints per-harness **reload/restart guidance**. For agents it
  is precise per harness (Claude picks agents up live; Copilot needs a restart; Codex/Gemini are
  treated conservatively as restart). Skills get a single honest, harness-agnostic hint (start a
  new session or run your harness's skills-reload command).
- Skipped harnesses (incompatible skill, missing agent variant, probe-gated target) are listed
  under `skipped:` and simply not served.

```bash
$ akit install -H copilot -H claude deploy-to-vercel
Installed skill 'deploy-to-vercel' for copilot, claude
  .claude/skills/deploy-to-vercel  (copilot, claude)
reload:
  skills: start a new session (or run your harness's skills-reload command) if it does not appear

$ akit install -H claude deploy-to-vercel
Reshaped skill 'deploy-to-vercel' for claude
  .claude/skills/deploy-to-vercel  (claude)
reload:
  skills: start a new session (or run your harness's skills-reload command) if it does not appear
```

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
`"needs_probe"`.

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

The legacy family keeps pulls under `.github/skills/`, `.github/agents/`, and
`.copilot/kit.lock.json`; the harness-aware engine keeps materializations under each harness's
discovery paths (`.agents/skills`, `.claude/skills`, `.github/agents`, …) plus
`.akit/kit.lock.json`. Both add every path they write to `.git/info/exclude` (a local, untracked
ignore list). Your tracked `.gitignore` is never touched, and `git status` stays clean.
