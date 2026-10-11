CREATE TABLE pt_idempotency (
    workspace_id TEXT NOT NULL REFERENCES pt_workspace(id),
    operation_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    response_json TEXT NOT NULL,
    committed_at_us INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, operation_key)
);
