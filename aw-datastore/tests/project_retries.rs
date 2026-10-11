use aw_datastore::{
    projects::{ProjectRetry, ProjectSnapshot},
    Datastore, DatastoreError,
};
use uuid::Uuid;

fn snapshot() -> ProjectSnapshot {
    ProjectSnapshot {
        workspace_id: Uuid::from_u128(100),
        revision: 0,
        settings: Default::default(),
        nodes: vec![],
        mappings: vec![],
        entries: vec![],
    }
}

fn retry(hash: &str) -> ProjectRetry {
    ProjectRetry {
        key: "synthetic-operation".into(),
        request_hash: hash.into(),
    }
}

#[test]
fn lost_response_replay_survives_restart_and_rejects_changed_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aw.db");
    let ds = Datastore::new(path.to_str().unwrap().into(), false);
    let candidate = snapshot();
    let first = ds
        .save_project_retry(candidate.clone(), retry("request-a"))
        .unwrap();
    assert_eq!(
        ds.save_project_retry(candidate.clone(), retry("request-a"))
            .unwrap(),
        first
    );
    assert!(matches!(
        ds.save_project_retry(candidate.clone(), retry("request-b")),
        Err(DatastoreError::ProjectIdempotencyConflict)
    ));
    ds.close();
    let ds = Datastore::new(path.to_str().unwrap().into(), false);
    assert_eq!(
        ds.save_project_retry(candidate, retry("request-a"))
            .unwrap(),
        first
    );
    let conn = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM pt_domain_revisions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    ds.close();
}

#[test]
fn failed_edit_does_not_reserve_key_and_concurrent_retry_commits_once() {
    let ds = Datastore::new_in_memory(false);
    let mut invalid = snapshot();
    invalid.revision = 99;
    assert!(ds.save_project_retry(invalid, retry("request-a")).is_err());
    assert!(ds
        .replay_project(snapshot().workspace_id, retry("request-a"))
        .unwrap()
        .is_none());
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let ds = ds.clone();
            std::thread::spawn(move || {
                ds.save_project_retry(snapshot(), retry("request-a"))
                    .unwrap()
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results[0], results[1]);
    assert_eq!(results[0].revision, 1);
    ds.close();
}

#[test]
fn schema_one_upgrades_and_retry_insert_failure_rolls_back_entire_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aw.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    aw_datastore::DatastoreInstance::new(&conn, true).unwrap();
    conn.execute_batch(
        "ALTER TABLE pt_workspace DROP COLUMN settings_json; DROP TABLE pt_entries; DROP TABLE pt_idempotency; DELETE FROM pt_schema_migrations WHERE version>=2;",
    )
    .unwrap();
    aw_datastore::DatastoreInstance::new(&conn, true).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM pt_schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 4);
    conn.execute_batch("CREATE TRIGGER fail_retry BEFORE INSERT ON pt_idempotency BEGIN SELECT RAISE(ABORT, 'synthetic storage failure'); END;").unwrap();
    drop(conn);
    let ds = Datastore::new(path.to_str().unwrap().into(), false);
    assert!(ds
        .save_project_retry(snapshot(), retry("request-a"))
        .is_err());
    assert_eq!(
        ds.get_projects(snapshot().workspace_id).unwrap().revision,
        0
    );
    assert!(ds
        .replay_project(snapshot().workspace_id, retry("request-a"))
        .unwrap()
        .is_none());
    ds.close();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM pt_domain_revisions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
