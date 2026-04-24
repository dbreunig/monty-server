CREATE TABLE functions (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    slug TEXT NOT NULL,
    code TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('stateless', 'bound')),
    env_id TEXT REFERENCES envs(id) ON DELETE SET NULL,
    is_public INTEGER NOT NULL DEFAULT 0,
    public_token TEXT,
    truncate_logs INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX idx_functions_user_slug ON functions(user_id, slug);
CREATE UNIQUE INDEX idx_functions_public_token
    ON functions(public_token) WHERE public_token IS NOT NULL;
CREATE INDEX idx_functions_user ON functions(user_id);
