# Project storage foundation (module schema 2)

This module uses the host's existing SQLite connection and worker lifecycle.
It creates only `pt_` tables, preserves AW `user_version`, and leaves bucket,
event, settings and query schemas unchanged. Writable startup migrates the
module transactionally; read-only peer startup never creates module tables.
Future/modified migration ledgers return an error rather than being overwritten.
Migration checksums use SHA-256 of the embedded SQL. Do not change an applied
migration; add a new version. Startup error propagation currently follows the
existing worker's failure behavior; friendly server startup diagnostics remain
future work.

`get_projects(workspace_id)` returns revision zero for an absent workspace.
`save_projects(candidate)` is an internal compare-and-swap snapshot primitive:
it validates hierarchy and mapping rules, rejects workspace mismatches and node
hard deletion, persists the entire candidate plus before/after revision history,
and responds only after the outer SQLite transaction commits. Savepoints roll
back failed operations within an existing watcher batch. Foreign keys are enabled
on writable worker connections, including existing AW relationships; the existing
datastore suite validates bucket deletion, replacement and migration behavior.

The schema stores indexed relational identifiers, parents, names and mapping date
ranges plus serialized typed node/mapping bodies. These bodies currently include
basic billing/reporting enums. Rates, fees, currency, workspace name/timezone,
per-entity revisions will arrive with the remaining stage-1
policy and API work, through new migrations. Snapshot revisions currently detect
concurrent edits across the whole workspace. Snapshot replacement is not intended
as a public bulk-overwrite endpoint or the final efficient per-entity CRUD adapter.
Full before/after snapshots are a simple initial audit format with storage cost
proportional to workspace size; later entity commands must retain prior history.

No watcher events are emitted by this persistence module. It does not collect or
send application context, SQL/message contents or credentials. Project names and
CaseId/ActivityId values are local domain data and are retained in revision history;
archive does not erase them. Existing AW bucket exports/backups and aw-sync do not
include this domain through their current logical APIs. A separate backup contract
and synchronization policy remain required before the first usable release.

Validation on Windows with Rust 1.99.0:

- `cargo test -p aw-datastore --locked --offline`: 67 passed, one existing legacy
  import test ignored, including eight new persistence tests.
- `cargo check -p aw-server --locked --offline`: passed; this checkout has no
  built embedded UI yet, so the server emits its existing missing-assets warning.
- Full all-target datastore Clippy with warnings denied is blocked by existing
  `io_other_error` in `export.rs` tests and `unnecessary_mut_passed` in
  `legacy_import.rs` tests. Those unrelated upstream files were not changed.
- Scoped Clippy (`--lib --test projects -- -D warnings`) passed.
- New tests cover separate-reader durability before close, reopening, concurrent
  revision conflicts, invalid edits, audit preservation, mappings/archive, SQL
  rollback, repeat migrations, unknown versions, failed migrations and unchanged
  read-only legacy initialization.

No API, UI, installed-host lifecycle, crash-injection or cross-platform validation
is implied by these synthetic SQLite tests.

Schema 2 adds persistent idempotency records. Same-key/same-request replay returns
the original committed snapshot, including its historical revision. Key reuse for
a different request is rejected. Savepoints atomically roll back the edit, audit
and key if record insertion fails. Successful keys and responses are retained
indefinitely in the local database; no pruning is implemented. Migration 1 is
unchanged and checksummed upgrades from it are supported. Run the repository's
`scripts/validate-project-tracking.ps1` for current local checks; GitHub Actions is
not used.
