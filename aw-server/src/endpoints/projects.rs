//! Additive project API; no writes to captured events or category settings.
use aw_datastore::projects::ProjectSnapshot;
use aw_projects::{ActivityIdMapping, Hierarchy, Node};
use chrono::NaiveDate;
use rocket::http::{Header, Status};
use rocket::request::{FromRequest, Outcome};
use rocket::response::{Responder, Response};
use rocket::serde::json::Json;
use rocket::{Request, State};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{HttpErrorJson, ServerState};

pub struct Precondition(Option<String>);

pub struct RetryHeader {
    key: Option<String>,
    operation: String,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for RetryHeader {
    type Error = ();
    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        Outcome::Success(Self {
            key: request
                .headers()
                .get_one("Idempotency-Key")
                .map(str::to_owned),
            operation: format!("{} {}", request.method(), request.uri().path()),
        })
    }
}

fn prepare_retry(
    state: &ServerState,
    workspace: &str,
    precondition: &Precondition,
    header: RetryHeader,
    body: Value,
) -> Result<
    (
        Option<ProjectJson>,
        Option<aw_datastore::projects::ProjectRetry>,
    ),
    HttpErrorJson,
> {
    // Preconditions remain required even on replay. Include the exact original
    // precondition in identity; do not substitute today's workspace revision.
    let etag = precondition
        .0
        .as_deref()
        .ok_or_else(|| error(Status::PreconditionRequired, "If-Match is required"))?;
    let Some(key) = header.key else {
        return Ok((None, None));
    };
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(error(Status::BadRequest, "invalid Idempotency-Key"));
    }
    let request_hash = format!(
        "{:x}",
        Sha256::digest(json!([header.operation, etag, body]).to_string().as_bytes())
    );
    let retry = aw_datastore::projects::ProjectRetry { key, request_hash };
    let replay = state
        .datastore
        .replay_project(id(workspace)?, retry.clone())
        .map_err(HttpErrorJson::from)?;
    Ok((replay.map(reply), Some(retry)))
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for Precondition {
    type Error = ();
    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        Outcome::Success(Self(
            request.headers().get_one("If-Match").map(str::to_owned),
        ))
    }
}

impl Precondition {
    fn check(&self, revision: i64) -> Result<(), HttpErrorJson> {
        let value = self
            .0
            .as_deref()
            .ok_or_else(|| error(Status::PreconditionRequired, "If-Match is required"))?;
        let expected = format!("\"{revision}\"");
        if value != expected {
            return Err(error(
                Status::PreconditionFailed,
                "workspace revision is stale or invalid",
            ));
        }
        Ok(())
    }
}

pub struct ProjectJson {
    body: Value,
    revision: i64,
}

impl<'r> Responder<'r, 'static> for ProjectJson {
    fn respond_to(self, request: &'r Request<'_>) -> rocket::response::Result<'static> {
        Response::build_from(Json(self.body).respond_to(request)?)
            .header(Header::new("ETag", format!("\"{}\"", self.revision)))
            .header(Header::new("Cache-Control", "no-store"))
            .ok()
    }
}

fn error(status: Status, message: &str) -> HttpErrorJson {
    HttpErrorJson::new(status, message.to_owned())
}

fn id(value: &str) -> Result<Uuid, HttpErrorJson> {
    Uuid::parse_str(value).map_err(|_| error(Status::BadRequest, "invalid UUID"))
}

fn load(state: &ServerState, workspace: &str) -> Result<ProjectSnapshot, HttpErrorJson> {
    state
        .datastore
        .get_projects(id(workspace)?)
        .map_err(Into::into)
}

fn reply(snapshot: ProjectSnapshot) -> ProjectJson {
    ProjectJson {
        revision: snapshot.revision,
        body: json!(snapshot),
    }
}

fn save(
    state: &ServerState,
    snapshot: ProjectSnapshot,
    retry: Option<aw_datastore::projects::ProjectRetry>,
) -> Result<ProjectJson, HttpErrorJson> {
    // Initial bounded snapshot adapter, not an unbounded public overwrite API.
    if snapshot.nodes.len() > 10_000 || snapshot.mappings.len() > 50_000 {
        return Err(error(
            Status::UnprocessableEntity,
            "workspace resource limit exceeded",
        ));
    }
    let saved = match retry {
        Some(retry) => state.datastore.save_project_retry(snapshot, retry),
        None => state.datastore.save_projects(snapshot),
    };
    saved.map(reply).map_err(Into::into)
}

#[get("/capabilities")]
pub fn capabilities() -> Json<Value> {
    Json(json!({"api_version": 1, "schema_version": 2,
        "features": ["nodes", "archive", "dated_activity_ids", "resolved_policy"],
        "concurrency": "workspace_etag", "idempotency_keys": true,
        "manual_entries": false, "export_snapshots": false}))
}

#[get("/workspaces/<workspace>/nodes?<q>&<include_archived>&<after>&<limit>")]
pub fn nodes(
    state: &State<ServerState>,
    workspace: &str,
    q: Option<&str>,
    include_archived: Option<bool>,
    after: Option<&str>,
    limit: Option<usize>,
) -> Result<ProjectJson, HttpErrorJson> {
    let snapshot = load(state, workspace)?;
    let tree = Hierarchy::new(snapshot.nodes.clone())
        .map_err(|_| error(Status::InternalServerError, "stored hierarchy is invalid"))?;
    let cursor = after.map(id).transpose()?;
    let limit = limit.unwrap_or(100);
    if !(1..=200).contains(&limit) {
        return Err(error(Status::BadRequest, "limit must be 1 through 200"));
    }
    if q.is_some_and(|q| q.len() > 256) {
        return Err(error(Status::BadRequest, "search is too long"));
    }
    let query = q.unwrap_or("").to_lowercase();
    let mut matches: Vec<_> = snapshot
        .nodes
        .iter()
        .filter(|node| {
            cursor.is_none_or(|cursor| node.id > cursor)
                && (include_archived.unwrap_or(false)
                    || !tree.resolve(node.id).is_ok_and(|p| p.effectively_archived))
                && (node.name.to_lowercase().contains(&query)
                    || node
                        .case_id
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query))
        })
        .collect();
    matches.sort_by_key(|node| node.id);
    let next = (matches.len() > limit).then(|| matches[limit - 1].id);
    matches.truncate(limit);
    Ok(ProjectJson {
        revision: snapshot.revision,
        body: json!({"revision": snapshot.revision, "nodes": matches, "next_cursor": next}),
    })
}

#[get("/workspaces/<workspace>/nodes/tree")]
pub fn tree(state: &State<ServerState>, workspace: &str) -> Result<ProjectJson, HttpErrorJson> {
    Ok(reply(load(state, workspace)?))
}

#[put(
    "/workspaces/<workspace>/nodes/<node_id>",
    format = "json",
    data = "<node>"
)]
pub fn put_node(
    state: &State<ServerState>,
    workspace: &str,
    node_id: &str,
    precondition: Precondition,
    retry_header: RetryHeader,
    node: Json<Node>,
) -> Result<ProjectJson, HttpErrorJson> {
    let _guard = state.project_write_lock.lock().map_err(|_| {
        error(
            Status::InternalServerError,
            "project mutation lock unavailable",
        )
    })?;
    let (replay, retry) =
        prepare_retry(state, workspace, &precondition, retry_header, json!(&*node))?;
    if let Some(response) = replay {
        return Ok(response);
    }
    let mut snapshot = load(state, workspace)?;
    precondition.check(snapshot.revision)?;
    let node = node.into_inner();
    if node.id != id(node_id)? || node.workspace_id != snapshot.workspace_id {
        return Err(error(
            Status::UnprocessableEntity,
            "node identity does not match URL",
        ));
    }
    if node.name.len() > 1024 || node.case_id.as_ref().is_some_and(|id| id.len() > 256) {
        return Err(error(Status::UnprocessableEntity, "node field is too long"));
    }
    match snapshot.nodes.iter_mut().find(|old| old.id == node.id) {
        Some(old) => *old = node,
        None => snapshot.nodes.push(node),
    }
    save(state, snapshot, retry)
}

#[get("/workspaces/<workspace>/nodes/<node_id>/resolved-policy?<work_date>")]
pub fn resolved(
    state: &State<ServerState>,
    workspace: &str,
    node_id: &str,
    work_date: &str,
) -> Result<ProjectJson, HttpErrorJson> {
    let snapshot = load(state, workspace)?;
    let node_id = id(node_id)?;
    if !snapshot.nodes.iter().any(|node| node.id == node_id) {
        return Err(error(Status::NotFound, "node does not exist"));
    }
    let date = NaiveDate::parse_from_str(work_date, "%Y-%m-%d")
        .map_err(|_| error(Status::BadRequest, "work_date must be YYYY-MM-DD"))?;
    if date.to_string() != work_date {
        return Err(error(Status::BadRequest, "work_date must be YYYY-MM-DD"));
    }
    let tree = Hierarchy::new(snapshot.nodes.clone())
        .map_err(|_| error(Status::InternalServerError, "stored hierarchy is invalid"))?;
    let policy = tree
        .resolve(node_id)
        .map_err(|_| error(Status::InternalServerError, "stored policy is invalid"))?;
    let mapping = tree
        .activity_id(node_id, date, &snapshot.mappings)
        .map_err(|_| error(Status::InternalServerError, "stored mapping is invalid"))?;
    Ok(ProjectJson {
        revision: snapshot.revision,
        body: json!({"revision": snapshot.revision, "work_date": date, "policy": policy, "mapping": mapping}),
    })
}

#[get("/workspaces/<workspace>/activity-id-mappings")]
pub fn mappings(state: &State<ServerState>, workspace: &str) -> Result<ProjectJson, HttpErrorJson> {
    let snapshot = load(state, workspace)?;
    Ok(ProjectJson {
        revision: snapshot.revision,
        body: json!({"revision": snapshot.revision, "mappings": snapshot.mappings}),
    })
}

#[put(
    "/workspaces/<workspace>/activity-id-mappings/<mapping_id>",
    format = "json",
    data = "<mapping>"
)]
pub fn put_mapping(
    state: &State<ServerState>,
    workspace: &str,
    mapping_id: &str,
    precondition: Precondition,
    retry_header: RetryHeader,
    mapping: Json<ActivityIdMapping>,
) -> Result<ProjectJson, HttpErrorJson> {
    let _guard = state.project_write_lock.lock().map_err(|_| {
        error(
            Status::InternalServerError,
            "project mutation lock unavailable",
        )
    })?;
    let (replay, retry) = prepare_retry(
        state,
        workspace,
        &precondition,
        retry_header,
        json!(&*mapping),
    )?;
    if let Some(response) = replay {
        return Ok(response);
    }
    let mut snapshot = load(state, workspace)?;
    precondition.check(snapshot.revision)?;
    let mapping = mapping.into_inner();
    if mapping.id != id(mapping_id)? || mapping.activity_id.len() > 256 {
        return Err(error(
            Status::UnprocessableEntity,
            "invalid mapping identity or length",
        ));
    }
    match snapshot
        .mappings
        .iter_mut()
        .find(|old| old.id == mapping.id)
    {
        Some(old) => *old = mapping,
        None => snapshot.mappings.push(mapping),
    }
    save(state, snapshot, retry)
}

#[delete("/workspaces/<workspace>/activity-id-mappings/<mapping_id>")]
pub fn delete_mapping(
    state: &State<ServerState>,
    workspace: &str,
    mapping_id: &str,
    precondition: Precondition,
    retry_header: RetryHeader,
) -> Result<ProjectJson, HttpErrorJson> {
    let _guard = state.project_write_lock.lock().map_err(|_| {
        error(
            Status::InternalServerError,
            "project mutation lock unavailable",
        )
    })?;
    let (replay, retry) =
        prepare_retry(state, workspace, &precondition, retry_header, Value::Null)?;
    if let Some(response) = replay {
        return Ok(response);
    }
    let mut snapshot = load(state, workspace)?;
    precondition.check(snapshot.revision)?;
    let mapping_id = id(mapping_id)?;
    if !snapshot.mappings.iter().any(|m| m.id == mapping_id) {
        return Err(error(Status::NotFound, "mapping does not exist"));
    }
    snapshot.mappings.retain(|m| m.id != mapping_id);
    save(state, snapshot, retry)
}
