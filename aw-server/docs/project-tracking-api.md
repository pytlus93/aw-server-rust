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
charges. These DTOs currently contain no monetary terms.

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
