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

## Stability

The crate follows 0.x semver: minor versions may make breaking changes, so hosts
should pin a specific tag or commit. `tests/embed.rs` exercises this whole surface
as an external consumer and is the contract the GUI relies on.
