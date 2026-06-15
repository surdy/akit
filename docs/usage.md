# akit usage

`akit` pulls personal agent customizations (skills and custom agents) from a central
**catalog** into a project on demand, kept personal and gitignored, tracked by a lockfile.

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
    agents/<name>.agent.md
    bundles/<name>.yml
  ```

Move your personal skills/agents here (out of `~/.copilot/`, which is auto-loaded in *every*
project). Skills are directories containing `SKILL.md`; agents are single
`agents/<name>.agent.md` files. `akit` then materializes only the ones you select into a given
project.

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
  agent (`<catalog>/agents/<id>.agent.md`). The same path resolution as `add` applies, so a
  single-segment `path` like `deploy-to-vercel` resolves to `skills/deploy-to-vercel` (or, with
  `--agent`, `agents/deploy-to-vercel.agent.md`) in the source repo.
- The catalog **id** defaults to the source's last path segment; `--as <id>` stores it under
  a different name. Ids must be a single path segment (no `/`).
- Validates the fetched source before writing: a skill must be a directory containing `SKILL.md`;
  an agent must be a `.agent.md` file.
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

$ akit pull --agent acme/kits/reviewer#main
Pulled agent 'reviewer' from acme/kits/reviewer#main -> /home/you/.akit/catalog/agents/reviewer.agent.md (copied)

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

### `drop` — remove an item from the catalog

```text
akit drop [--agent] <id>
```

Removes a skill or agent from your catalog (`skills/<id>/` or `agents/<id>.agent.md`). If the
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

Example:

```bash
$ akit status
BUNDLE  TYPE   ID                MODE     TARGET                                      STATUS
web     skill  deploy-to-vercel  symlink  .github/skills/deploy-to-vercel             ok
web     agent  code-reviewer     symlink  .github/agents/code-reviewer.agent.md       ok
-       skill  deploy-helper     symlink  .github/skills/deploy-helper                ok
```

With `--json`, `status` is serialized as lowercase (`"ok"`, `"orphaned"`, `"missing"`, or
`"drifted"`), `mode` is `"symlink"` or `"copy"`, and every item includes `bundle` (`null` for
standalone items).

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
    "not_a_git_repo": false,
    "healthy": true
  }
}
```

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

- Scans `<catalog>/skills/<name>/SKILL.md` and `<catalog>/agents/<name>.agent.md`.
- Reads leading YAML-style frontmatter fields: `name`, `description`, and `category`.
- If `name` is missing, uses the skill directory or agent file name.
- Fuzzy-matches `<query>` against `name` first and `description` second; best scores print first.
- An omitted or empty query lists every catalog item.
- Missing or malformed frontmatter emits a warning to stderr and falls back to available fields.
- Supports the global `--json` flag. The global `--project` flag is accepted but `search` reads
  only the catalog.

Human output is one hit per line:

```text
type  name  — description (category)
```

If `description` or `category` is empty, that part is omitted.

Example:

```bash
$ akit search deploy
skill  Deploy Helper  — Ship apps safely (ops)
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
strings. Empty-query results use score `0`.

### `show` — preview a catalog item

```bash
akit show [--agent] <id>
```

- Reads a single item from the catalog and prints its frontmatter and raw content,
  without touching the project.
- Defaults to a skill (`<catalog>/skills/<id>/SKILL.md`); pass `--agent` to read an
  agent (`<catalog>/agents/<id>.agent.md`).
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
`category` serialize as empty strings. `path` is the absolute source path and `content` is
the full file (frontmatter included).

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
- Sorted skills-first, then by id.
- Supports the global `--json` flag. The global `--project` flag is accepted but `ls`
  reads only the catalog.

Example:

```bash
$ akit ls
TYPE   ID             ORIGIN                              DESCRIPTION
skill  deploy-helper  local                               Ship apps safely
skill  grill-me       mattpocock/skills/.../grill-me      Stress-test a plan
agent  reviewer       local                               Review code
```

For `ls`, `--json` emits a stable array of objects:

```json
[
  {
    "type": "skill",
    "id": "grill-me",
    "description": "Stress-test a plan",
    "source": "mattpocock/skills/skills/productivity/grill-me#main"
  }
]
```

`type` is `"skill"` or `"agent"`. `description` is the frontmatter description (empty when
absent). `source` is present only for pulled items; hand-authored (local) items omit it.

## How it stays out of your repo

Pulls live under `.github/skills/`, `.github/agents/`, and `.copilot/kit.lock.json`, all added to
`.git/info/exclude` (a local, untracked ignore list). Your tracked `.gitignore` is never touched,
and `git status` stays clean.
