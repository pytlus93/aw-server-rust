use aw_datastore::{projects::ProjectSnapshot, Datastore, DatastoreError};
use aw_projects::{BillingMode, Node, NodeKind, ReportingPolicy};
use rusqlite::Connection;
use uuid::Uuid;

fn snapshot() -> ProjectSnapshot {
    let workspace_id = Uuid::from_u128(100);
    ProjectSnapshot {
        workspace_id,
        revision: 0,
        settings: Default::default(),
        nodes: vec![Node {
            id: Uuid::from_u128(1),
            workspace_id,
            parent_id: None,
            kind: NodeKind::Project,
            name: "Synthetic case".into(),
            description: String::new(),
            billing_terms: Default::default(),
            case_id: Some("CS0000000".into()),
            archived: false,
            billing: BillingMode::Fixed,
            reporting: ReportingPolicy::SelfNode,
        }],
        mappings: vec![],
    }
}

#[test]
fn acknowledged_save_is_visible_on_separate_connection_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aw.db");
    let ds = Datastore::new(path.to_str().unwrap().into(), false);
    let mut initial = snapshot();
    initial.settings.reporting_timezone = "Europe/Prague".into();
    initial.nodes[0].description = "Synthetic milestone".into();
    initial.nodes[0].billing_terms.currency = Some("CZK".into());
    initial.nodes[0].billing_terms.fixed_fee = Some("25000.123456".into());
    let saved = ds.save_projects(initial).unwrap();
    assert_eq!(saved.revision, 1);
    // Separate WAL reader proves the response comes after commit, before close.
    let conn = Connection::open(&path).unwrap();
    let revision: i64 = conn
        .query_row("SELECT revision FROM pt_workspace", [], |r| r.get(0))
        .unwrap();
    assert_eq!(revision, 1);
    let history: String = conn
        .query_row("SELECT after_json FROM pt_domain_revisions", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<ProjectSnapshot>(&history).unwrap(),
        saved
    );
    let violations: i64 = conn
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
    ds.close();
    let reopened = Datastore::new(path.to_str().unwrap().into(), false);
    assert_eq!(reopened.get_projects(saved.workspace_id).unwrap(), saved);
    reopened.close();
}

#[test]
fn rejects_stale_clients_and_invalid_edits_without_changing_history() {
    let ds = Datastore::new_in_memory(false);
    let initial = snapshot();
    let saved = ds.save_projects(initial.clone()).unwrap();
    assert!(matches!(
        ds.save_projects(initial),
        Err(DatastoreError::ProjectConflict {
            expected: 0,
            actual: 1
        })
    ));
    let mut invalid = saved.clone();
    invalid.nodes[0].parent_id = Some(invalid.nodes[0].id);
    assert!(matches!(
        ds.save_projects(invalid),
        Err(DatastoreError::InvalidProjectData(_))
    ));
    assert_eq!(ds.get_projects(saved.workspace_id).unwrap(), saved);
    let mut deleted = saved.clone();
    deleted.nodes.clear();
    assert!(matches!(
        ds.save_projects(deleted),
        Err(DatastoreError::InvalidProjectData(_))
    ));
    assert_eq!(ds.get_projects(saved.workspace_id).unwrap(), saved);
    ds.close();
}

#[test]
fn concurrent_clients_only_one_can_commit_same_revision() {
    let ds = Datastore::new_in_memory(false);
    let saved = ds.save_projects(snapshot()).unwrap();
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let ds = ds.clone();
            let mut candidate = saved.clone();
            candidate.nodes[0].name = format!("Synthetic edit {i}");
            std::thread::spawn(move || ds.save_projects(candidate))
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(DatastoreError::ProjectConflict { .. })))
            .count(),
        1
    );
    ds.close();
}

#[test]
fn migration_is_idempotent_and_preserves_aw_schema_version() {
    let conn = Connection::open_in_memory().unwrap();
    aw_datastore::DatastoreInstance::new(&conn, true).unwrap();
    aw_datastore::DatastoreInstance::new(&conn, true).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, aw_datastore::NEWEST_DB_VERSION as i64);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM pt_schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 3);
    conn.execute(
        "UPDATE pt_schema_migrations SET version=4 WHERE version=3",
        [],
    )
    .unwrap();
    assert!(aw_datastore::DatastoreInstance::new(&conn, true).is_err());
}

#[test]
fn read_only_legacy_open_does_not_add_project_tables() {
    let conn = Connection::open_in_memory().unwrap();
    aw_datastore::DatastoreInstance::new(&conn, true).unwrap();
    conn.execute_batch("DROP TABLE pt_idempotency; DROP TABLE pt_domain_revisions; DROP TABLE pt_activity_id_mappings; DROP TABLE pt_nodes; DROP TABLE pt_workspace; DROP TABLE pt_schema_migrations").unwrap();
    aw_datastore::DatastoreInstance::new(&conn, false).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'pt_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn mappings_and_archive_edits_persist_with_immutable_prior_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aw.db");
    let ds = Datastore::new(path.to_str().unwrap().into(), false);
    let mut candidate = snapshot();
    candidate.mappings.push(aw_projects::ActivityIdMapping {
        id: Uuid::from_u128(10),
        owner_node_id: candidate.nodes[0].id,
        valid_from: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
        valid_until: None,
        activity_id: "ACT-2026-10".into(),
    });
    let first = ds.save_projects(candidate).unwrap();
    let mut edit = first.clone();
    edit.nodes[0].archived = true;
    edit.mappings[0].activity_id = "ACT-CORRECTED".into();
    let second = ds.save_projects(edit).unwrap();
    let conn = Connection::open(&path).unwrap();
    let history: String = conn
        .query_row(
            "SELECT after_json FROM pt_domain_revisions WHERE revision=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<ProjectSnapshot>(&history).unwrap(),
        first
    );
    ds.close();
    let reopened = Datastore::new(path.to_str().unwrap().into(), false);
    assert_eq!(reopened.get_projects(second.workspace_id).unwrap(), second);
    reopened.close();
}

#[test]
fn failed_schema_creation_rolls_back_module_migration() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE pt_nodes (incompatible TEXT)", [])
        .unwrap();
    assert!(aw_datastore::DatastoreInstance::new(&conn, true).is_err());
    let count: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('pt_workspace', 'pt_schema_migrations')", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn sql_failure_rolls_back_partial_mutation_and_leaves_worker_usable() {
    let ds = Datastore::new_in_memory(false);
    let saved = ds.save_projects(snapshot()).unwrap();
    let mut other = snapshot();
    other.workspace_id = Uuid::from_u128(200);
    other.nodes[0].workspace_id = other.workspace_id;
    // Same globally stable node ID in a different workspace causes SQL failure
    // after workspace creation, so the savepoint must remove that partial row.
    assert!(ds.save_projects(other.clone()).is_err());
    assert_eq!(ds.get_projects(other.workspace_id).unwrap().revision, 0);
    assert_eq!(ds.get_projects(saved.workspace_id).unwrap(), saved);
    other.nodes[0].id = Uuid::from_u128(2);
    assert_eq!(ds.save_projects(other).unwrap().revision, 1);
    ds.close();
}
