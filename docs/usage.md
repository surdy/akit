# ckit usage

`ckit` pulls personal Copilot customizations (skills and custom agents) from a central
**collection** into a project on demand, kept personal and gitignored, tracked by a lockfile.

## Install / build

```bash
git clone https://github.com/surdy/ckit.git
cd ckit
cargo build --release
# binary at target/release/ckit
```

## Your collection

`ckit` reads from a single local collection directory:

- Location: `$KIT_COLLECTION_DIR`, or `~/.copilot-kit/collection` by default.
- Layout:

  ```text
  <collection>/
    skills/<name>/SKILL.md
    agents/<name>.agent.md
    bundles/<name>.yml
  ```

Move your personal skills/agents here (out of `~/.copilot/`, which is auto-loaded in *every*
project). Skills are directories containing `SKILL.md`; agents are single
`agents/<name>.agent.md` files. `ckit` then materializes only the ones you select into a given
project.

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
ckit add [--agent] [--copy] <name>
ckit add [--agent] [--copy] owner/repo/path[#ref]
ckit add [--copy] --bundle <name>
```

- By default, symlinks `<collection>/skills/<name>` into `<project>/.github/skills/<name>`
  (Copilot loads it as a **project-scope** skill).
- With `--agent`, symlinks `<collection>/agents/<name>.agent.md` into
  `<project>/.github/agents/<name>.agent.md`.
- With `--copy`, copies the source files instead of symlinking them and records `"mode": "copy"`
  in the lockfile and `--json` add report.
- If `<name>` contains `/`, `ckit` treats it as a remote source spec instead of a local collection
  name. The syntax is `owner/repo/path[#ref]`; `path` points at a skill directory containing
  `SKILL.md` (or, with `--agent`, a `.agent.md` file). For skill repositories with a top-level
  `skills/` directory, a single-segment path like `deploy-to-vercel` also resolves to
  `skills/deploy-to-vercel`. The installed id/target comes from the last path segment, so
  `vercel-labs/agent-skills/deploy-to-vercel#main` lands at `.github/skills/deploy-to-vercel`.
- Remote sources are fetched with `git` into a local cache, then materialized through the same
  symlink/copy pipeline as local items. The default cache is
  `~/.cache/ckit/sources/<owner>/<repo>@<ref-or-default>`; `$XDG_CACHE_HOME` changes the cache base
  to `$XDG_CACHE_HOME/ckit`, and `$KIT_CACHE_DIR` overrides it entirely. The CLI fetches from
  `https://github.com/<owner>/<repo>` by default; `$KIT_REMOTE_BASE_URL` can point at another git
  URL base (for example, a local `file://` mirror).
- Remote lockfile entries record `"source": "owner/repo/path"` and `"ref": "<ref>"` when a ref was
  supplied. The future intended backend is APM; the current git-fetch cache is the equivalent
  offline-friendly mechanism used today.
- With `--bundle <name>`, reads `<collection>/bundles/<name>.yml` and adds every listed skill and
  agent through the same add pipeline. `--copy` applies to every item. `--agent` is not used with
  bundles because the manifest already distinguishes item types.
- If symlink creation fails at runtime (for example, Windows without symlink privilege), `ckit`
  warns on stderr, falls back to copying, and records the effective `"mode": "copy"`.
- Appends the pull and the lockfile to `.git/info/exclude`, so nothing is committed and your
  teammates are unaffected. This applies to both local and remote pulls.
- Records the item in `<project>/.copilot/kit.lock.json`. Bundle-installed entries carry
  `"bundle": "<name>"`.
- Idempotent: re-running is a safe no-op.

Example:

```bash
$ ckit add deploy-helper
Added skill 'deploy-helper' -> .github/skills/deploy-helper (linked)

$ ckit add --agent reviewer
Added agent 'reviewer' -> .github/agents/reviewer.agent.md (linked)

$ ckit add --copy deploy-helper
Added skill 'deploy-helper' -> .github/skills/deploy-helper (copied)

$ ckit add vercel-labs/agent-skills/deploy-to-vercel#main
Added skill 'deploy-to-vercel' -> .github/skills/deploy-to-vercel (linked)

$ ckit add --bundle web
Added bundle 'web' (3 items)
  Added skill 'deploy-to-vercel' -> .github/skills/deploy-to-vercel (linked)
  Added skill 'lint-fix' -> .github/skills/lint-fix (linked)
  Added agent 'code-reviewer' -> .github/agents/code-reviewer.agent.md (linked)
```

### `rm` — remove a skill or agent from the project

```bash
ckit rm [--agent] <name>
ckit rm --bundle <name>
```

- Removes the materialized target from `.github/skills/` or `.github/agents/`.
- Removes that target's `.git/info/exclude` line.
- Removes the lockfile entry.
- Remote items are removed by their installed id (the source path leaf), so a remote add of
  `owner/repo/deploy-to-vercel#main` is reversed with `ckit rm deploy-to-vercel`.
- With `--bundle <name>`, removes exactly the installed lockfile entries tagged with that bundle.
  The current manifest is not consulted, so removal stays precise even if the manifest changed.
- Idempotent: removing an item that is not installed exits successfully.

Example:

```bash
$ ckit rm deploy-helper
Removed skill 'deploy-helper' -> .github/skills/deploy-helper (removed)

$ ckit rm --agent reviewer
Removed agent 'reviewer' -> .github/agents/reviewer.agent.md (removed)

$ ckit rm --bundle web
Removed bundle 'web' (3 items)
  Removed skill 'deploy-to-vercel' -> .github/skills/deploy-to-vercel (removed)
  Removed skill 'lint-fix' -> .github/skills/lint-fix (removed)
  Removed agent 'code-reviewer' -> .github/agents/code-reviewer.agent.md (removed)
```

### `ls` / `status` — list installed items

```bash
ckit ls
# alias:
ckit status
```

Lists lockfile entries grouped by bundle and labeled in the `BUNDLE` column. Standalone entries
show `-`. Health values:

- `ok`: target exists and, for symlinks, resolves to an existing source.
- `orphaned`: target is a symlink whose source no longer exists.
- `missing`: lockfile entry exists but the target is gone.
- `drifted`: copy-mode target exists, but its content differs from the current collection source.

Example:

```bash
$ ckit ls
BUNDLE  TYPE   ID                MODE     TARGET                                      STATUS
web     skill  deploy-to-vercel  symlink  .github/skills/deploy-to-vercel             ok
web     agent  code-reviewer     symlink  .github/agents/code-reviewer.agent.md       ok
-       skill  deploy-helper     symlink  .github/skills/deploy-helper                ok
```

With `--json`, `status` is serialized as lowercase (`"ok"`, `"orphaned"`, `"missing"`, or
`"drifted"`), `mode` is `"symlink"` or `"copy"`, and every item includes `bundle` (`null` for
standalone items).

### `doctor` — read-only reconcile report

```bash
ckit doctor
```

Checks the lockfile against the project filesystem, the current collection, and
`.git/info/exclude` without modifying anything.

- Reports each lockfile item as `ok`, `orphaned`, `missing`, or `drifted`.
- Shows whether the collection source exists, the project target exists, and the target's
  `/.github/...` exclude line is present.
- Reports missing managed exclude lines, including `/.copilot/kit.lock.json`.
- Flags stale managed exclude lines (for example, a `/.github/skills/...` line with no matching
  lockfile entry) but does not remove them.

Example:

```bash
$ ckit doctor
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
ckit sync
```

Reconciles the project from the lockfile. It is idempotent: running it again after a clean sync is a
no-op.

Repairs:

- Missing materialized targets, using the recorded `mode` (`symlink` or `copy`) and the current
  collection source.
- Missing `.git/info/exclude` lines for locked targets.
- The lockfile's own `/.copilot/kit.lock.json` exclude line.

Does **not** silently delete or overwrite user data:

- Orphaned items whose collection source is gone are reported and skipped.
- Drifted copy-mode targets are reported and not overwritten.
- Stale exclude lines are reported and not removed.

Example:

```bash
$ ckit sync
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

### `search` — search the collection

```bash
ckit search [<query>]
```

- Scans `<collection>/skills/<name>/SKILL.md` and `<collection>/agents/<name>.agent.md`.
- Reads leading YAML-style frontmatter fields: `name`, `description`, and `category`.
- If `name` is missing, uses the skill directory or agent file name.
- Fuzzy-matches `<query>` against `name` first and `description` second; best scores print first.
- An omitted or empty query lists every collection item.
- Missing or malformed frontmatter emits a warning to stderr and falls back to available fields.
- Supports the global `--json` flag. The global `--project` flag is accepted but `search` reads
  only the collection.

Human output is one hit per line:

```text
type  name  — description (category)
```

If `description` or `category` is empty, that part is omitted.

Example:

```bash
$ ckit search deploy
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

## How it stays out of your repo

Pulls live under `.github/skills/`, `.github/agents/`, and `.copilot/kit.lock.json`, all added to
`.git/info/exclude` (a local, untracked ignore list). Your tracked `.gitignore` is never touched,
and `git status` stays clean.
