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
  ```

Move your personal skills/agents here (out of `~/.copilot/`, which is auto-loaded in *every*
project). Skills are directories containing `SKILL.md`; agents are single
`agents/<name>.agent.md` files. `ckit` then materializes only the ones you select into a given
project.

## Global flags

| Flag | Meaning |
|---|---|
| `--project <dir>` | Target project (defaults to the enclosing git repo root, else the current dir). |
| `--json` | Emit machine-readable JSON instead of human text. |

## Commands

### `add` — pull a skill or agent into the project

```bash
ckit add [--agent] [--copy] <name>
```

- By default, symlinks `<collection>/skills/<name>` into `<project>/.github/skills/<name>`
  (Copilot loads it as a **project-scope** skill).
- With `--agent`, symlinks `<collection>/agents/<name>.agent.md` into
  `<project>/.github/agents/<name>.agent.md`.
- With `--copy`, copies the source files instead of symlinking them and records `"mode": "copy"`
  in the lockfile and `--json` add report.
- If symlink creation fails at runtime (for example, Windows without symlink privilege), `ckit`
  warns on stderr, falls back to copying, and records the effective `"mode": "copy"`.
- Appends the pull and the lockfile to `.git/info/exclude`, so nothing is committed and your
  teammates are unaffected.
- Records the item in `<project>/.copilot/kit.lock.json`.
- Idempotent: re-running is a safe no-op.

Example:

```bash
$ ckit add deploy-helper
Added skill 'deploy-helper' -> .github/skills/deploy-helper (linked)

$ ckit add --agent reviewer
Added agent 'reviewer' -> .github/agents/reviewer.agent.md (linked)

$ ckit add --copy deploy-helper
Added skill 'deploy-helper' -> .github/skills/deploy-helper (copied)
```

### `rm` — remove a skill or agent from the project

```bash
ckit rm [--agent] <name>
```

- Removes the materialized target from `.github/skills/` or `.github/agents/`.
- Removes that target's `.git/info/exclude` line.
- Removes the lockfile entry.
- Idempotent: removing an item that is not installed exits successfully.

Example:

```bash
$ ckit rm deploy-helper
Removed skill 'deploy-helper' -> .github/skills/deploy-helper (removed)

$ ckit rm --agent reviewer
Removed agent 'reviewer' -> .github/agents/reviewer.agent.md (removed)
```

### `ls` / `status` — list installed items

```bash
ckit ls
# alias:
ckit status
```

Lists lockfile entries with health:

- `ok`: target exists and, for symlinks, resolves to an existing source.
- `orphaned`: target is a symlink whose source no longer exists.
- `missing`: lockfile entry exists but the target is gone.
- `drifted`: copy-mode target exists, but its content differs from the current collection source.

Example:

```bash
$ ckit ls
TYPE   ID             MODE     TARGET                                  STATUS
skill  deploy-helper  symlink  .github/skills/deploy-helper           ok
agent  reviewer       symlink  .github/agents/reviewer.agent.md       ok
```

With `--json`, `status` is serialized as lowercase (`"ok"`, `"orphaned"`, `"missing"`, or
`"drifted"`), and `mode` is `"symlink"` or `"copy"`.

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
