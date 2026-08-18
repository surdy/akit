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
MaterializationRecord}`, `reconcile::{Diagnosis, ForeignPath}`, and `materialize::Drift`. `HarnessContext` and `RemoveScope` are inputs
(not `Serialize`); everything else round-trips to the same JSON the `--json` CLI emits.

> `reconcile` also exposes safe recovery operations a host renders behind confirmation UX —
> `repair_with` (re-materialize *missing* records, never overwriting modified copies),
> `detach_with` (drop ownership, keep bytes, make files git-visible), `forget_with` (drop an
> orphaned record), `remove_stale_excludes_with`, and `adopt_with` (claim existing exact-content
> files as owned) — each with a `HealthReport`/report type.

### Cross-project awareness (the global install index)

`index` keeps a local-only list of the project directories akit has installed into
(`~/.akit/state/installs.json`, or `$AKIT_STATE_DIR`) — project paths plus a timestamp, never
anything about the items themselves. It powers two cross-project reads:

```rust
use akit::index;

// `akit where <id>`: every known project holding an item, with its health there.
let hits = index::locate(&catalog, ItemType::Skill, "deploy-to-vercel")?;

// `akit update --propagate`: re-materialize copy installs of refreshed items, skipping
// drifted copies (conflicts) and symlinks (already live).
let report = index::propagate(&catalog, &[(ItemType::Skill, "deploy-to-vercel".to_string())])?;
```

```rust
// `akit doctor --all`: comparable *copy* installs holding different bytes in
// different projects. Symlink installs track the catalog live and never diverge.
let report = index::divergences()?;
for d in &report.items {
    // `d.harness` names the agent variant this family belongs to (None for skills).
    for content in &d.contents {
        println!("{} {}: {:?}", d.id, content.hash, content.paths);
    }
}
for s in &report.skipped {
    eprintln!("could not inspect {}: {}", s.project, s.error);
}
```

Key types (all `Serialize`/`Deserialize`): `index::{InstallIndex, ProjectEntry, WhereReport,
WhereProject, SkippedProject, ContentVariant, VariantGroup, Divergence, DivergenceReport,
PropagationReport, ProjectPropagation, PropagatedItem, PropagatedPath, PropagateStatus,
PropagateSummary}`. `ops::UpdateReport` gained an optional `propagation: Option<PropagationReport>`
field, skipped in JSON when absent — additive to the existing `update` shape. `reconcile::Diagnosis`
gained the same shape for issue #41: `foreign: Vec<ForeignPath>` (the unmanaged occupants of harness
target paths, also available on its own via `reconcile::foreign_paths_with`) plus
`divergences: Option<DivergenceReport>`, absent from JSON unless a caller fills it in — which is how
`doctor --all` round-trips through a library type rather than a CLI-private struct:

```rust
let mut diagnosis = reconcile::diagnose(&project, &catalog)?;
// Hand the just-computed drift over so this project's clean copies are not re-hashed.
diagnosis.divergences = Some(index::divergences_with(Some((&project.root, &diagnosis.items)))?);
```

`WhereReport` likewise gained `variants: Vec<VariantGroup>` and `diverged: bool`. A `VariantGroup` is
one family of *comparable* copies — all copies of a skill (one skill directory), but agent copies
grouped by the harness whose native variant file they came from, since those are different bytes by
construction. `diverged` requires two different **projects** to hold different content; two copies
inside one project are drift, which `health` already reports as `modified`. Divergence and foreign
detection are strictly read-only, including of the index file itself: reads filter stale entries in
memory and never rewrite it (only `record_install` compacts it).

Every entry point has an `_at(index_path, …)` variant (`locate_at`, `propagate_at`,
`divergences_at`, `divergences_at_with`, `known_projects_at`, `record_install_at`) so a host can keep
its own state file. Index I/O is always local `std::fs`, never the `FsTransport` seam — the index is
host state about *this* machine, while the seam exists to reach a remote project root.

Index writes are **not** part of the engine's install path: the CLI calls
`index::record_install(&project.root)` after a successful install. A host should do the same for
local project roots it wants `where`/`propagate` to see, and should *not* record roots it installed
into over a remote transport.

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
