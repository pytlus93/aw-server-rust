use aw_server::{
    config::AWConfig,
    endpoints::{self, ServerState},
};
use rocket::{
    http::{ContentType, Header, Status},
    local::blocking::Client,
};
use serde_json::{json, Value};

const WORKSPACE: &str = "00000000-0000-0000-0000-000000000064";
const ROOT: &str = "00000000-0000-0000-0000-000000000001";
const CHILD: &str = "00000000-0000-0000-0000-000000000002";
const MAPPING: &str = "00000000-0000-0000-0000-00000000000a";
const BASE: &str = "/api/0/project-tracking/v1";

fn client(config: AWConfig) -> Client {
    let state = ServerState::new(
        aw_datastore::Datastore::new_in_memory(false),
        endpoints::AssetResolver::new(None),
        "synthetic".into(),
    );
    Client::untracked(endpoints::build_rocket(state, config)).unwrap()
}

fn path(suffix: &str) -> String {
    format!("{BASE}/workspaces/{WORKSPACE}/{suffix}")
}

fn node(id: &str, parent: Option<&str>) -> Value {
    json!({"id": id, "workspace_id": WORKSPACE, "parent_id": parent,
        "kind": if parent.is_some() { "task" } else { "project" },
        "name": "Synthetic deployment", "case_id": "CS0000000", "archived": false,
        "billing": "inherited", "reporting": {"mode": "inherited"}})
}

fn put(
    client: &Client,
    suffix: &str,
    revision: Option<i64>,
    body: Value,
) -> (Status, Option<String>, Value) {
    let mut request = client
        .put(path(suffix))
        .header(Header::new("Host", "127.0.0.1:5600"))
        .header(ContentType::JSON)
        .body(body.to_string());
    if let Some(revision) = revision {
        request = request.header(Header::new("If-Match", format!("\"{revision}\"")));
    }
    let response = request.dispatch();
    let status = response.status();
    let etag = response.headers().get_one("ETag").map(str::to_owned);
    let body = response.into_json::<Value>().unwrap();
    (status, etag, body)
}

#[test]
fn node_crud_requires_workspace_etag_and_archive_filters_descendants() {
    let client = client(AWConfig::default());
    let capability = client
        .get(format!("{BASE}/capabilities"))
        .header(Header::new("Host", "127.0.0.1:5600"))
        .dispatch();
    assert_eq!(capability.status(), Status::Ok);
    assert_eq!(
        capability.into_json::<Value>().unwrap()["idempotency_keys"],
        true
    );
    assert_eq!(
        put(&client, &format!("nodes/{ROOT}"), None, node(ROOT, None)).0,
        Status::PreconditionRequired
    );
    let (status, etag, _) = put(&client, &format!("nodes/{ROOT}"), Some(0), node(ROOT, None));
    assert_eq!(status, Status::Ok);
    assert_eq!(etag.as_deref(), Some("\"1\""));
    assert_eq!(
        put(
            &client,
            &format!("nodes/{CHILD}"),
            Some(0),
            node(CHILD, Some(ROOT))
        )
        .0,
        Status::PreconditionFailed
    );
    assert_eq!(
        put(
            &client,
            &format!("nodes/{CHILD}"),
            Some(1),
            node(CHILD, Some(ROOT))
        )
        .0,
        Status::Ok
    );
    let mut archived = node(ROOT, None);
    archived["archived"] = json!(true);
    assert_eq!(
        put(&client, &format!("nodes/{ROOT}"), Some(2), archived).0,
        Status::Ok
    );
    let response = client
        .get(path("nodes"))
        .header(Header::new("Host", "127.0.0.1:5600"))
        .dispatch();
    assert_eq!(response.headers().get_one("ETag"), Some("\"3\""));
    assert_eq!(response.into_json::<Value>().unwrap()["nodes"], json!([]));
    let response = client
        .get(path("nodes?include_archived=true&limit=1"))
        .header(Header::new("Host", "127.0.0.1:5600"))
        .dispatch();
    let body = response.into_json::<Value>().unwrap();
    assert_eq!(body["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(body["next_cursor"], ROOT);
}

#[test]
fn invalid_hierarchy_and_url_identity_leave_existing_data_unchanged() {
    let client = client(AWConfig::default());
    assert_eq!(
        put(
            &client,
            &format!("nodes/{ROOT}"),
            Some(0),
            node(CHILD, None)
        )
        .0,
        Status::UnprocessableEntity
    );
    assert_eq!(
        put(&client, &format!("nodes/{ROOT}"), Some(0), node(ROOT, None)).0,
        Status::Ok
    );
    assert_eq!(
        put(
            &client,
            &format!("nodes/{CHILD}"),
            Some(1),
            node(CHILD, Some(CHILD))
        )
        .0,
        Status::UnprocessableEntity
    );
    let response = client
        .get(path("nodes/tree"))
        .header(Header::new("Host", "127.0.0.1:5600"))
        .dispatch();
    assert_eq!(response.into_json::<Value>().unwrap()["revision"], 1);
}

#[test]
fn mappings_resolve_by_work_date_and_can_be_removed_with_preconditions() {
    let client = client(AWConfig::default());
    put(&client, &format!("nodes/{ROOT}"), Some(0), node(ROOT, None));
    put(
        &client,
        &format!("nodes/{CHILD}"),
        Some(1),
        node(CHILD, Some(ROOT)),
    );
    let mapping = json!({"id": MAPPING, "owner_node_id": ROOT, "valid_from": "2026-10-01", "valid_until": "2026-11-01", "activity_id": "ACT-OCT"});
    assert_eq!(
        put(
            &client,
            &format!("activity-id-mappings/{MAPPING}"),
            Some(2),
            mapping
        )
        .0,
        Status::Ok
    );
    for (date, expected) in [
        ("2026-10-31", json!("ACT-OCT")),
        ("2026-11-01", Value::Null),
    ] {
        let response = client
            .get(path(&format!(
                "nodes/{CHILD}/resolved-policy?work_date={date}"
            )))
            .header(Header::new("Host", "127.0.0.1:5600"))
            .dispatch();
        assert_eq!(response.status(), Status::Ok);
        let body = response.into_json::<Value>().unwrap();
        assert_eq!(body["policy"]["reporting_target_id"], ROOT);
        assert_eq!(body["mapping"]["activity_id"], expected);
    }
    let response = client
        .delete(path(&format!("activity-id-mappings/{MAPPING}")))
        .header(Header::new("Host", "127.0.0.1:5600"))
        .header(Header::new("If-Match", "\"3\""))
        .dispatch();
    assert_eq!(response.status(), Status::Ok);
    assert_eq!(response.into_json::<Value>().unwrap()["revision"], 4);
}

#[test]
fn new_routes_keep_host_and_api_key_protection() {
    let mut config = AWConfig::default();
    config.auth.api_key = Some("synthetic-key".into());
    let client = client(config);
    let response = client
        .get(format!("{BASE}/capabilities"))
        .header(Header::new("Host", "127.0.0.1:5600"))
        .dispatch();
    assert_eq!(response.status(), Status::Unauthorized);
    let response = client
        .get(format!("{BASE}/capabilities"))
        .header(Header::new("Host", "127.0.0.1:5600"))
        .header(Header::new("Authorization", "Bearer synthetic-key"))
        .dispatch();
    assert_eq!(response.status(), Status::Ok);
    let response = client
        .get(format!("{BASE}/capabilities"))
        .header(Header::new("Host", "evil.example"))
        .header(Header::new("Authorization", "Bearer synthetic-key"))
        .dispatch();
    assert_eq!(response.status(), Status::BadRequest);
}

#[test]
fn browser_watcher_origin_cannot_read_project_data() {
    let client = client(AWConfig::default());
    let response = client
        .get(path("nodes/tree"))
        .header(Header::new("Host", "127.0.0.1:5600"))
        .header(Header::new("Origin", "moz-extension://synthetic-watcher"))
        .dispatch();
    assert_eq!(response.status(), Status::Forbidden);
    assert!(response
        .headers()
        .get_one("Access-Control-Allow-Origin")
        .is_none());
}

#[test]
fn uncertain_http_write_can_replay_original_response_without_duplicate_revision() {
    let client = client(AWConfig::default());
    let send = |name: &str, key: &str| {
        let mut body = node(ROOT, None);
        body["name"] = json!(name);
        client
            .put(path(&format!("nodes/{ROOT}")))
            .header(Header::new("Host", "127.0.0.1:5600"))
            .header(ContentType::JSON)
            .header(Header::new("If-Match", "\"0\""))
            .header(Header::new("Idempotency-Key", key.to_owned()))
            .body(body.to_string())
            .dispatch()
    };
    let first = send("Synthetic case", "synthetic-create");
    assert_eq!(first.status(), Status::Ok);
    let original = first.into_json::<Value>().unwrap();
    let replay = send("Synthetic case", "synthetic-create");
    assert_eq!(replay.status(), Status::Ok);
    assert_eq!(replay.headers().get_one("ETag"), Some("\"1\""));
    assert_eq!(replay.into_json::<Value>().unwrap(), original);
    assert_eq!(
        send("Different case", "synthetic-create").status(),
        Status::Conflict
    );
    assert_eq!(
        send("Synthetic case", "new-key").status(),
        Status::PreconditionFailed
    );
    assert_eq!(
        send("Synthetic case", "bad key").status(),
        Status::BadRequest
    );
}
