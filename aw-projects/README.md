# Project tracking domain foundation

Internal ActivityWatch Rust module, not a standalone product. This first
increment validates nested project/task snapshots, resolves billing and
reporting inheritance, and selects dated ActivityId mappings by work date.
It stores no activity data, emits no watcher events and has no network access.

`SelfNode` reports to the node owning the explicit policy. Inheriting that policy
retains its target. With no explicit reporting policy, the root is the target.
Unspecified billing defaults to non-billable. Fixed-price effort is represented
separately from T&M; this crate does not calculate invoice charges.

Mappings are half-open local calendar-date ranges on reporting targets. Missing
mappings remain unknown. The caller must derive the work date from the entry's
reporting timezone, never the export date. Export snapshots and timezone slicing
are later work. Database mutations must validate the entire candidate hierarchy
inside the transaction, especially after reparenting.

Current limit: 128 nodes along a path, including assignment and root.
Persistence, migrations, HTTP API, money/rate policies, UI, manual entries,
export history and sync are not implemented by this foundation.

Validation: `cargo test -p aw-projects --locked` and
`cargo clippy -p aw-projects --all-targets --locked -- -D warnings`.
Fixtures are synthetic; no installed-host or platform packaging claims.
