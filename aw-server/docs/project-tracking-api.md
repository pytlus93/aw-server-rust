# Project tracking API foundation

Base: `/api/0/project-tracking/v1`. Existing AW routes and Python server remain
unchanged. All new routes use the server's existing authentication, host and
CORS protections. The Python server does not implement this capability.

This is an initial API increment, not complete stage 1. Workspace metadata,
timezone, rates/fees/currency, per-entity revisions and UI are
pending. Callers must not treat capabilities as implying those features.

| Method/path below base | Contract |
|---|---|
| `GET /capabilities` | Supported feature list and explicit unavailable features. |
| `GET /workspaces/{uuid}/nodes` | Search `q` in name/CaseId; `include_archived`, UUID `after` cursor and `limit` 1–200 (default 100). Archived ancestors hide descendants by default. Returns nodes and next cursor. |
| `GET /workspaces/{uuid}/nodes/tree` | Full flat hierarchy with parent IDs, mappings and workspace revision. Absent workspace is empty at revision zero. |
| `PUT /workspaces/{uuid}/nodes/{uuid}` | Create/replace a complete node using caller-generated stable ID. Covers editing, reparenting, archive/unarchive and basic billing/reporting policy. URL and body identity must match. No hard delete. |
| `GET /workspaces/{uuid}/nodes/{uuid}/resolved-policy?work_date=YYYY-MM-DD` | Effective billing/reporting owners and target plus applicable ActivityId mapping, or null. Uses work date, never export date. Unknown node is 404. |
| `GET /workspaces/{uuid}/activity-id-mappings` | Current mappings and workspace revision. |
| `PUT /workspaces/{uuid}/activity-id-mappings/{uuid}` | Create/replace complete dated mapping; overlap/range/owner validation in persisted transaction. |
| `DELETE /workspaces/{uuid}/activity-id-mappings/{uuid}` | Remove current mapping while preserving audit snapshots; unknown mapping is 404. |

Domain responses carry `ETag: "<workspace revision>"` and `Cache-Control: no-store`.
Every mutation requires the exact strong ETag as `If-Match`. Missing header is
428; mismatched, weak, wildcard or stale header is 412. A racing client can pass
the initial HTTP check and still receive 412 from the datastore compare-and-swap.
Initial creation uses `If-Match: "0"`. PUT returns 200 with the complete committed
snapshot and updated ETag. IDs are UUIDs, never inferred from titles or CaseId.

Node body fields: `id`, `workspace_id`, nullable `parent_id`, `kind` (`project` or
`task`), `name`, nullable `case_id`, `archived`, `billing` (`inherited`, `tm`,
`fixed`, `non_billable`), and `reporting` (`{"mode":"inherited"}`,
`{"mode":"self"}`, or `{"mode":"ancestor","target_id":"UUID"}`).
An inherited self target remains the explicit policy owner's node. Root default
is non-billable with root reporting. Hierarchy depth limit is 128.

Mapping body: `id`, `owner_node_id`, `valid_from`, nullable exclusive `valid_until`,
and nonempty `activity_id`. Date ranges cannot overlap on the same owner.
Missing mappings do not fall back to CaseId. Fixed mode does not calculate hourly
charges.

Schema 3 adds `GET /workspace`, which returns the stable local workspace
`a8e34d04-6b7d-4c73-a15e-26eb24204ef1` (reading does not create it), and
`PUT /workspaces/{uuid}/settings` with the same ETag/retry contract. Settings
contain a nonempty `name` (256 UTF-8 bytes maximum) and an IANA
`reporting_timezone`, defaulting to `Projects` and `UTC`. No browser timezone
is silently substituted. Existing workspaces and historical snapshots receive
these defaults during migration/deserialization.

Nodes add `description` (default empty, maximum 16,384 UTF-8 bytes) and
`billing_terms` (default all null): `currency`, `hourly_rate`, `fixed_fee`.
Amounts are decimal **strings**, never binary floats: 1–18 whole digits and
optionally 1–6 fractional digits, nonnegative, with no exponent or grouping.
Currency is a three-letter uppercase code; a provided amount requires currency.
T&M permits only hourly rate, fixed permits only fixed fee; inherited and
non-billable require null terms. Effective billing inherits mode and terms
together from the nearest explicit owner. These are configured terms, not an
invoice or exchange-rate calculation. Description and terms remain local
project metadata; they are not copied into watcher events.

Malformed UUID/date/query limits return 400; malformed JSON follows Rocket's
JSON responder behavior. Invalid hierarchy/policy/mapping is 422. Storage failures
return 500. Error bodies follow existing AW `HttpErrorJson`; structured domain
error codes are future work. Names are limited to 1,024 UTF-8 bytes; CaseId and
ActivityId to 256; snapshot adapter limits are 10,000 nodes/50,000 mappings.

Mutations accept `Idempotency-Key` (1–128 visible ASCII characters). Use a random
key per intended edit. Retry with the same key, method/path, typed JSON body and
original If-Match. A committed retry returns its original snapshot/ETag even after
later edits or restart; refresh current state afterwards. Different requests using
the same key return 409. SHA-256 identity includes method/path, original ETag and
canonical typed JSON. Invalid edits reserve no key. Response/edit/audit records
commit atomically. Keys currently have indefinite local retention; no pruning or
expiry is implemented. Do not put secrets in keys. Without a key, an old ETag is
still 412 and never creates duplicate IDs. Cursor
pages are not a frozen read snapshot; if the returned revision changes, restart
pagination. No API operation rewrites watcher events or categories.

GitHub Actions is unavailable and is not used. The custom workflow was removed;
inherited workflows remain untouched and provide no validation evidence here.
Run `./scripts/validate-project-tracking.ps1 -Offline` in PowerShell from this
repository. Omit `-Offline` to fetch locked Cargo dependencies on a fresh machine.
Requires Rust/Cargo, rustfmt, Clippy and the native linker (MSVC C++ tools and
Windows SDK on Windows). The script fails on the first failed command, checks
formatting, runs domain/datastore/HTTP regressions and scoped Clippy. Equivalent
Cargo commands inside it can run directly on Linux/macOS, which remain unvalidated.
It does not upload data or create an installer. Host, Android and UI evidence are
separate from local synthetic tests.

Local baseline validation (Windows, Rust 1.99.0): five initial HTTP tests and 22 existing API
tests passed. Server/API Clippy passed with `--no-deps --lib --test projects_api
-- -D warnings`. Without `--no-deps`, pre-existing duplicate lint attributes in
aw-query block strict Clippy. Domain/datastore scoped Clippy also passed.

Current validation after schema-2 retry support: the local script passed on
Windows/Rust 1.99.0, with 105 tests passing and one existing legacy-import test
ignored. Formatting and both scoped Clippy commands passed. Retry tests cover
restart replay, key conflicts, concurrent identical requests, failed-edit key
release, schema-1 upgrade and atomic rollback on retry-record insertion failure.

Schema-3 validation: 108 tests passed, one legacy test ignored, formatting and
scoped Clippy passed on Windows/Rust 1.99.0. Added exact-term inheritance,
invalid decimals/timezones, settings HTTP validation and schema-1-to-3 migration.
The persistence regression verifies settings, descriptions and fixed terms
survive restart along with revision history.

## Manual ledger (module schema 4)

Capabilities now advertise `manual_entries` and `atomic_task_assignment`.
Snapshots add `entries` (defaults to an empty list in older audit/retry records).
`PUT /workspaces/{uuid}/bookings` uses the existing ETag and idempotency contract:

```json
{
  "new_task": null,
  "upsert": [{
    "id": "00000000-0000-0000-0000-00000000000a",
    "node_id": "00000000-0000-0000-0000-000000000001",
    "description": "Synthetic design review",
    "intervals": [{"start": "2026-10-11T10:00:00Z", "end": "2026-10-11T10:15:00Z"}],
    "sources": [{"bucket_id": "synthetic-source", "event_id": 1}],
    "deleted": false,
    "duration_ns": ""
  }],
  "delete_ids": []
}
```

An optional complete `new_task` Node must be a new, unarchived task in this
workspace, with at least one submitted entry assigned to it. Creation and all
entries commit as one revision or neither commits. New tasks inherit policies
when submitted with inherited billing/reporting. No separate task-creation call
is needed in the Assign flow. A batch permits at most 100 upserts and 100 deletes.
The snapshot limit is 50,000 entries, including deletion tombstones.

Each entry has a stable UUID, assignment node, description (max 16,384 UTF-8
bytes), up to 1,000 UTC/offset RFC3339 half-open intervals, up to 1,000 source
references and a tombstone flag. The server normalizes overlapping/touching spans,
preserves disjoint gaps, and derives exact decimal-string `duration_ns` from
nanosecond timestamps; client duration values are never trusted. Each merged
span must have a positive representable signed-64-bit nanosecond duration.
No rounding policy is applied. Any overlap between live entries in the same
workspace is rejected with 422, including offline work and cross-task overlap.
Adjacent entries are legal. Existing IDs edit/resize/reassign; split is a batch
deleting one old ID and creating two new IDs. Deletes retain the old entry and
before/after history, and deleted IDs cannot be reused.

Archived branches reject new/reassigned work. Existing work can still be edited
or deleted without unarchiving; archiving a node never erases booked entries.
Source references are nullable event IDs plus bucket IDs, not copied event data,
and have no captured-event foreign keys. Source deletion, pruning or failed reads
do not delete bookings. References indicate provenance, not proof that a later
manually moved/resized interval still matches that source. No booking mutation
calls AW bucket/event update/delete APIs or changes categories.

Schema-4 local validation (Windows/Rust 1.99.0): 116 tests passed, one existing
legacy test ignored; formatting and scoped Clippy passed. Added interval union,
gap preservation, nanosecond and DST-offset duration, split/concurrent overlap,
archive behavior, atomic task creation/rollback and identical request replay.
SQLite reopen coverage includes manual entries and missing source references.
The source-deletion HTTP regression verifies booking leaves captured events
unchanged and subsequent bucket deletion leaves booked work unchanged.
Reports/export snapshots and complete domain backup/restore remain stage 3.
