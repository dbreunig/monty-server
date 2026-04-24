CREATE TABLE function_calls (
    id INTEGER PRIMARY KEY,
    function_id TEXT NOT NULL REFERENCES functions(id) ON DELETE CASCADE,
    status INTEGER NOT NULL,
    request_body TEXT,
    response_body TEXT,
    error TEXT,
    duration_ms INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_function_calls_fn ON function_calls(function_id, id);
