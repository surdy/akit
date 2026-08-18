# Embedding akit (library API)

`akit` is a library crate as well as a CLI. A Rust host — for example
[pterm](https://github.com/surdy/pterm), which puts a GUI on top — depends on the
crate and drives the same core the CLI uses. There is no separate binary to bundle
or shell out to.

> The CLI in `main.rs` is itself a thin wrapper over this API, so the library and
> the `--json` CLI always behave identically.

## Add the dependency

```toml
# Cargo.toml
[dependencies]
# pin to a tag/commit for reproducible builds
akit = { git = "https://github.com/surdy/akit", rev = "<commit-or-tag>" }
serde_json = "1"
```

A path dependency (`akit = { path = "../akit" }`) also works for local
co-development.

## The two anchors

Every operation takes a **`Project`** (where items are materialized) and, for most,
a **`Catalog`** (where items come from):

```rust
use akit::project::Project;
use akit::catalog::Catalog;

// Resolve the project: explicit dir, else the enclosing git root, else cwd.
let project = Project::locate(Some(workspace_dir))?;

// Resolve the catalog: explicit root, or `$KIT_CATALOG_DIR`
// (default `~/.akit/catalog`) via `Catalog::locate()`.
let catalog = Catalog::with_root(catalog_dir); // or Catalog::locate()?
```

## Operations

All report types derive `serde::Serialize`, so a host can return them straight to
its frontend (e.g. as a Tauri command result).

```rust
use akit::{ops, search, doctor};
use akit::lockfile::{ItemType, Mode};

// List installed items with health (ok / orphaned / missing / drifted).
let items = ops::list_items_with_catalog(&project, &catalog)?;
let json  = serde_json::to_string(&items)?; // hand to the GUI

// List the whole catalog (every skill/agent + provenance), independent of any project.
let catalog_items = ops::list_catalog(&catalog)?;

// Add a skill (symlink by default) or an agent.
ops::add_skill(&project, &catalog, "deploy-helper")?;
ops::add_item(&project, &catalog, ItemType::Agent, "reviewer", Mode::Symlink, None)?;

// Add a named bundle, or a remote `owner/repo/path#ref` source.
ops::add_bundle(&project, &catalog, "web", Mode::Symlink)?;

// Remove.
ops::remove_skill(&project, "deploy-helper")?;

// Search the catalog by frontmatter (name / description / category).
let hits = search::search(&catalog, "deploy")?;

// Reconcile: read-only report, or repair safe drift.
let report = doctor::diagnose(&project, &catalog)?;
let synced = doctor::sync(&project, &catalog)?;
```

Key types: `ops::{AddReport, RemoveReport, ListItem, CatalogItem, HealthStatus}`,
`search::SearchHit`, `doctor::{DoctorReport, SyncReport}`, and
`lockfile::{ItemType, Mode}` — all `Serialize`.

## The harness-aware engine (v0.10+)

The `ops::*`/`doctor::*` surface above is the legacy, Copilot-shaped model. From v0.10 akit also
ships a **harness-aware** engine (`install`, `reconcile`) that materializes an item into **each
selected harness's own discovery paths** (Copilot, Claude, Codex, Gemini, OpenCode) and records
ownership in a separate `.akit/kit.lock.json`. This is the surface an embedding host like
[madari](https://github.com/surdy/madari) drives.

### Explicit context, explicit transport

Two design choices make the engine safe to embed and to run concurrently or against a remote root:

- **`install::HarnessContext`** — the target harness set is passed as an explicit, immutable,
  non-empty value object. The library **never** reads a process-global env (`AKIT_HARNESSES`) or
  interactive prompt to decide targets — that resolution is the CLI's job. Two concurrent callers
  (e.g. two madari panes) therefore never race on ambient state.

  ```rust
  use akit::install::HarnessContext;
  use akit::harness::HarnessId;

  // Deduped + sorted; empty is an error.
  let ctx = HarnessContext::new([HarnessId::Copilot, HarnessId::Claude])?;
  ```

- **`transport::FsTransport`** — every `*_with` entry point takes an explicit destination
  filesystem. All engine I/O (materializations, the `.akit` lockfile, and the managed
  git-exclude block) flows through it, so a host can redirect writes to a remote root (SFTP) or a
  test double. `transport::LocalFs` is the local-disk implementation the plain (non-`_with`)
  wrappers use.

### Operations

```rust
use akit::install::{self, HarnessContext, RemoveScope};
use akit::reconcile;
use akit::transport::LocalFs;
use akit::lockfile::ItemType;

let fs = LocalFs; // or a host-supplied `&dyn FsTransport` (e.g. an SFTP transport)

// Install (or reshape) an item for exactly `ctx`'s harnesses. Absolute: re-running
// with a different set reshapes the install to that set.
let report = install::install_with(&fs, &project, &catalog, ItemType::Skill, "deploy", &ctx)?;

// Preview it (dry-run): diff the plan against `.akit/kit.lock.json` without writing.
let preview = install::plan_install_with(&fs, &project, &catalog, ItemType::Skill, "deploy", &ctx)?;

// Uninstall fully, or scope to some harnesses (reshapes the rest).
install::remove_with(&fs, &project, ItemType::Skill, "deploy", RemoveScope::All)?;
install::remove_with(&fs, &project, ItemType::Skill, "deploy",
    RemoveScope::Harnesses(vec![HarnessId::Claude]))?;

// List installs recorded in `.akit/kit.lock.json` (no drift check).
let installs = install::status_with(&fs, &project)?;

// Remove every akit-owned file and clear the lockfile.
let reset = install::reset_with(&fs, &project)?;

// Read-only per-item drift + degraded-harness + stale-exclude report.
let health = reconcile::health_with(&fs, &project, &catalog)?;
```

Each function has a plain wrapper (`install`, `plan_install`, `remove`, `status`, `reset`,
`reconcile::health`) that supplies `LocalFs` for callers that don't need a custom transport.

Key types (all `Serialize`): `install::{HarnessContext, RemoveScope, InstallReport, InstallPreview,
RemoveReport, ResetReport}`, `reconcile::{HealthReport, ItemHealth, MaterializationHealth}`,
`plan::{PlannedMaterialization, PlanIssue}`, `harness::HarnessId`, `ownership::{Installation,
MaterializationRecord}`, and `materialize::Drift`. `HarnessContext` and `RemoveScope` are inputs
(not `Serialize`); everything else round-trips to the same JSON the `--json` CLI emits.

> `reconcile` also exposes safe recovery operations a host renders behind confirmation UX —
> `repair_with` (re-materialize *missing* records, never overwriting modified copies),
> `detach_with` (drop ownership, keep bytes, make files git-visible), `forget_with` (drop an
> orphaned record), `remove_stale_excludes_with`, and `adopt_with` (claim existing exact-content
> files as owned) — each with a `HealthReport`/report type.

### Per-host capability verification

`verify::verify_all` / `verify::verify_harness` decide whether a harness is actually supported on a
given host by combining a live binary+version probe (through the `exec::CommandRunner` seam) with
akit's static registry facts. A host supplies its own runner — `exec::LocalRunner` for the local
machine, or an SSH-backed runner for a remote host — and enables kit support only once the outcome
is `verified`. No model/LLM is involved.

```rust
use akit::{verify, exec::LocalRunner};

let verifications = verify::verify_all(&LocalRunner, "local")?; // Vec<HostVerification>
```

Each primitive is gated independently, so an unmet gate for one never suppresses the other.
`minVersion`/`versionOk` are the **agent** gate; `skillMinVersion`/`skillVersionOk` (added in the
#46 registry pass) are the **skill** gate. The two skill fields are strictly additive — the
existing agent fields kept their meaning — and both gates come from
[`harness-registry.md`](harness-registry.md).

## Stability

The crate follows 0.x semver: minor versions may make breaking changes, so hosts
should pin a specific tag or commit. `tests/embed.rs` exercises this whole surface
as an external consumer and is the contract the GUI relies on.
