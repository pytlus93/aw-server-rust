-- Module schema 1. AW buckets/events and PRAGMA user_version are unchanged.
CREATE TABLE pt_workspace (
    id TEXT PRIMARY KEY NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL
);
CREATE TABLE pt_nodes (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES pt_workspace(id),
    parent_id TEXT REFERENCES pt_nodes(id) DEFERRABLE INITIALLY DEFERRED,
    kind TEXT NOT NULL CHECK (kind IN ('project', 'task')),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    case_id TEXT,
    archived INTEGER NOT NULL CHECK (archived IN (0, 1)),
    body TEXT NOT NULL
);
CREATE INDEX pt_nodes_parent ON pt_nodes(workspace_id, parent_id);
CREATE INDEX pt_nodes_name ON pt_nodes(workspace_id, name);
CREATE INDEX pt_nodes_case ON pt_nodes(workspace_id, case_id);
CREATE TABLE pt_activity_id_mappings (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES pt_workspace(id),
    owner_node_id TEXT NOT NULL REFERENCES pt_nodes(id) DEFERRABLE INITIALLY DEFERRED,
    valid_from TEXT NOT NULL,
    valid_until TEXT,
    activity_id TEXT NOT NULL CHECK (length(trim(activity_id)) > 0),
    body TEXT NOT NULL,
    CHECK (valid_until IS NULL OR valid_until > valid_from)
);
CREATE INDEX pt_mappings_owner_date ON pt_activity_id_mappings(owner_node_id, valid_from);
CREATE TABLE pt_domain_revisions (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES pt_workspace(id),
    revision INTEGER NOT NULL,
    before_json TEXT NOT NULL,
    after_json TEXT NOT NULL,
    recorded_at_us INTEGER NOT NULL,
    UNIQUE (workspace_id, revision)
);
