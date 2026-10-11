-- Separate manual ledger. Source references intentionally have no event FK:
-- captured event deletion must not destroy booked work or historical evidence.
CREATE TABLE pt_entries (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES pt_workspace(id),
    node_id TEXT NOT NULL REFERENCES pt_nodes(id) DEFERRABLE INITIALLY DEFERRED,
    deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
    body TEXT NOT NULL
);
CREATE INDEX pt_entries_node ON pt_entries(workspace_id, node_id, deleted);
