CREATE TABLE repl_calls (
    id INTEGER PRIMARY KEY,
    env_id TEXT NOT NULL REFERENCES envs(id) ON DELETE CASCADE,
    code TEXT NOT NULL,
    stdout TEXT NOT NULL DEFAULT '',
    stderr TEXT NOT NULL DEFAULT '',
    result TEXT,
    error TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_repl_calls_env ON repl_calls(env_id, id);
