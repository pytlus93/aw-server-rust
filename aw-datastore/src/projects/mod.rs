//! Storage foundation for the project-tracking module. Snapshot replacement is
//! an internal transactional primitive, not a public REST bulk-overwrite API.
use aw_projects::{ActivityIdMapping, Hierarchy, Node};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::DatastoreError;

const MIGRATION: &str = include_str!("../../migrations/projects/0001_domain.sql");
const MIGRATION_2: &str = include_str!("../../migrations/projects/0002_idempotency.sql");
const MIGRATION_3: &str = include_str!("../../migrations/projects/0003_workspace_settings.sql");

#[derive(Debug, Clone)]
pub struct ProjectRetry {
    pub key: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub workspace_id: Uuid,
    pub revision: i64,
    #[serde(default)]
    pub settings: aw_projects::WorkspaceSettings,
    pub nodes: Vec<Node>,
    pub mappings: Vec<ActivityIdMapping>,
}

fn internal(error: impl std::fmt::Display) -> DatastoreError {
    DatastoreError::InternalError(format!("Project storage: {error}"))
}

/// Uses its own migration ledger without changing AW's user_version. Called
/// before the worker begins serving commands, only on writable databases.
pub(crate) fn migrate(conn: &Connection) -> Result<(), DatastoreError> {
    conn.execute_batch("SAVEPOINT pt_migration")
        .map_err(internal)?;
    let result = (|| {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS pt_schema_migrations (version INTEGER PRIMARY KEY, applied_at_us INTEGER NOT NULL, checksum TEXT NOT NULL)").map_err(internal)?;
        let rows: Vec<(i64, String)> = conn
            .prepare("SELECT version, checksum FROM pt_schema_migrations ORDER BY version")
            .map_err(internal)?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(internal)?
            .collect::<rusqlite::Result<_>>()
            .map_err(internal)?;
        let migrations = [MIGRATION, MIGRATION_2, MIGRATION_3];
        if rows.len() > migrations.len() {
            return Err(internal("unsupported project schema"));
        }
        for (index, sql) in migrations.iter().enumerate() {
            let version = index as i64 + 1;
            let checksum = format!("{:x}", Sha256::digest(sql.as_bytes()));
            if let Some(row) = rows.get(index) {
                if row != &(version, checksum) {
                    return Err(internal("unsupported or modified project schema"));
                }
            } else {
                conn.execute_batch(sql).map_err(internal)?;
                conn.execute(
                    "INSERT INTO pt_schema_migrations VALUES (?1, ?2, ?3)",
                    params![version, Utc::now().timestamp_micros(), checksum],
                )
                .map_err(internal)?;
            }
        }
        Ok(())
    })();
    finish_savepoint(conn, "pt_migration", result)
}

pub(crate) fn replay(
    conn: &Connection,
    workspace_id: Uuid,
    retry: &ProjectRetry,
) -> Result<Option<ProjectSnapshot>, DatastoreError> {
    if retry.key.is_empty()
        || retry.key.len() > 128
        || !retry.key.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(DatastoreError::InvalidProjectData(
            "invalid idempotency key".into(),
        ));
    }
    let record: Option<(String, String)> = conn.query_row(
        "SELECT request_hash, response_json FROM pt_idempotency WHERE workspace_id=?1 AND operation_key=?2",
        params![workspace_id.to_string(), retry.key], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(internal)?;
    match record {
        Some((hash, response)) if hash == retry.request_hash => {
            Ok(Some(serde_json::from_str(&response).map_err(internal)?))
        }
        Some(_) => Err(DatastoreError::ProjectIdempotencyConflict),
        None => Ok(None),
    }
}

pub(crate) fn save_with_retry(
    conn: &Connection,
    candidate: ProjectSnapshot,
    retry: ProjectRetry,
) -> Result<ProjectSnapshot, DatastoreError> {
    if let Some(response) = replay(conn, candidate.workspace_id, &retry)? {
        return Ok(response);
    }
    conn.execute_batch("SAVEPOINT pt_retry").map_err(internal)?;
    let result = (|| {
        let saved = save(conn, candidate)?;
        conn.execute(
            "INSERT INTO pt_idempotency VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                saved.workspace_id.to_string(),
                retry.key,
                retry.request_hash,
                serde_json::to_string(&saved).map_err(internal)?,
                Utc::now().timestamp_micros()
            ],
        )
        .map_err(internal)?;
        Ok(saved)
    })();
    finish_savepoint(conn, "pt_retry", result)
}

fn finish_savepoint<T>(
    conn: &Connection,
    name: &str,
    result: Result<T, DatastoreError>,
) -> Result<T, DatastoreError> {
    match result {
        Ok(value) => {
            conn.execute_batch(&format!("RELEASE {name}"))
                .map_err(internal)?;
            Ok(value)
        }
        Err(error) => {
            conn.execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"))
                .map_err(internal)?;
            Err(error)
        }
    }
}

pub(crate) fn load(
    conn: &Connection,
    workspace_id: Uuid,
) -> Result<ProjectSnapshot, DatastoreError> {
    let id = workspace_id.to_string();
    let revision = conn
        .query_row(
            "SELECT revision FROM pt_workspace WHERE id = ?1",
            [&id],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?
        .unwrap_or(0);
    let read = |table: &str| -> Result<Vec<String>, DatastoreError> {
        conn.prepare(&format!(
            "SELECT body FROM {table} WHERE workspace_id = ?1 ORDER BY id"
        ))
        .map_err(internal)?
        .query_map([&id], |row| row.get(0))
        .map_err(internal)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(internal)
    };
    let nodes = read("pt_nodes")?
        .into_iter()
        .map(|json| serde_json::from_str(&json).map_err(internal))
        .collect::<Result<Vec<Node>, _>>()?;
    let mappings = read("pt_activity_id_mappings")?
        .into_iter()
        .map(|json| serde_json::from_str(&json).map_err(internal))
        .collect::<Result<Vec<ActivityIdMapping>, _>>()?;
    let settings: Option<String> = conn
        .query_row(
            "SELECT settings_json FROM pt_workspace WHERE id=?1",
            [&id],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    let settings = settings
        .map(|json| serde_json::from_str(&json).map_err(internal))
        .transpose()?
        .unwrap_or_default();
    Ok(ProjectSnapshot {
        workspace_id,
        revision,
        settings,
        nodes,
        mappings,
    })
}

/// Compare-and-swap an entire validated candidate domain. The worker commits
/// the outer transaction before acknowledging success. Savepoints prevent a
/// failed mutation from leaking partial changes into the watcher's batch.
pub(crate) fn save(
    conn: &Connection,
    mut candidate: ProjectSnapshot,
) -> Result<ProjectSnapshot, DatastoreError> {
    candidate
        .settings
        .validate()
        .map_err(|e| DatastoreError::InvalidProjectData(e.into()))?;
    let tree = Hierarchy::new(candidate.nodes.clone())
        .map_err(|e| DatastoreError::InvalidProjectData(format!("{e:?}")))?;
    tree.validate_mappings(&candidate.mappings)
        .map_err(|e| DatastoreError::InvalidProjectData(format!("{e:?}")))?;
    if candidate
        .nodes
        .iter()
        .any(|node| node.workspace_id != candidate.workspace_id)
    {
        return Err(DatastoreError::InvalidProjectData(
            "workspace mismatch".into(),
        ));
    }
    conn.execute_batch("SAVEPOINT pt_mutation")
        .map_err(internal)?;
    let result = (|| {
        let before = load(conn, candidate.workspace_id)?;
        if candidate.revision != before.revision {
            return Err(DatastoreError::ProjectConflict {
                expected: candidate.revision,
                actual: before.revision,
            });
        }
        // No hard deletion in stage 1: callers archive instead. Historical
        // revisions and mappings must not be lost through snapshot replacement.
        if before
            .nodes
            .iter()
            .any(|old| !candidate.nodes.iter().any(|new| old.id == new.id))
        {
            return Err(DatastoreError::InvalidProjectData(
                "nodes must be archived, not deleted".into(),
            ));
        }
        candidate.revision = before
            .revision
            .checked_add(1)
            .ok_or_else(|| internal("revision overflow"))?;
        candidate.nodes.sort_by_key(|node| node.id);
        candidate.mappings.sort_by_key(|mapping| mapping.id);
        let workspace = candidate.workspace_id.to_string();
        let now = Utc::now().timestamp_micros();
        conn.execute("INSERT INTO pt_workspace (id, revision, created_at_us, updated_at_us) VALUES (?1, ?2, ?3, ?3) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision, updated_at_us=excluded.updated_at_us", params![workspace, candidate.revision, now]).map_err(internal)?;
        conn.execute(
            "UPDATE pt_workspace SET settings_json=?1 WHERE id=?2",
            params![
                serde_json::to_string(&candidate.settings).map_err(internal)?,
                workspace
            ],
        )
        .map_err(internal)?;
        // Delete mappings first; parent references are deferred until commit.
        conn.execute(
            "DELETE FROM pt_activity_id_mappings WHERE workspace_id=?1",
            [&workspace],
        )
        .map_err(internal)?;
        conn.execute("DELETE FROM pt_nodes WHERE workspace_id=?1", [&workspace])
            .map_err(internal)?;
        for node in &candidate.nodes {
            conn.execute("INSERT INTO pt_nodes (id, workspace_id, parent_id, kind, name, case_id, archived, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)", params![node.id.to_string(), workspace, node.parent_id.map(|id| id.to_string()), match node.kind { aw_projects::NodeKind::Project => "project", aw_projects::NodeKind::Task => "task" }, node.name, node.case_id, node.archived, serde_json::to_string(node).map_err(internal)?]).map_err(internal)?;
        }
        for mapping in &candidate.mappings {
            conn.execute("INSERT INTO pt_activity_id_mappings (id, workspace_id, owner_node_id, valid_from, valid_until, activity_id, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![mapping.id.to_string(), workspace, mapping.owner_node_id.to_string(), mapping.valid_from.to_string(), mapping.valid_until.map(|d| d.to_string()), mapping.activity_id, serde_json::to_string(mapping).map_err(internal)?]).map_err(internal)?;
        }
        conn.execute("INSERT INTO pt_domain_revisions (workspace_id, revision, before_json, after_json, recorded_at_us) VALUES (?1, ?2, ?3, ?4, ?5)", params![workspace, candidate.revision, serde_json::to_string(&before).map_err(internal)?, serde_json::to_string(&candidate).map_err(internal)?, now]).map_err(internal)?;
        Ok(candidate)
    })();
    finish_savepoint(conn, "pt_mutation", result)
}
