CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    github_id INTEGER UNIQUE,
    github_login TEXT,
    email TEXT,
    password_hash TEXT,
    is_admin INTEGER NOT NULL DEFAULT 0,
    approved_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
-- Local (password) accounts must have unique emails. GitHub users may share
-- emails with local users; their identity is the github_id.
CREATE UNIQUE INDEX idx_users_local_email ON users(email) WHERE password_hash IS NOT NULL;

CREATE TABLE api_keys (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    prefix TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at TEXT
);
CREATE INDEX idx_api_keys_user ON api_keys(user_id);
CREATE INDEX idx_api_keys_prefix ON api_keys(prefix);

CREATE TABLE envs (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_envs_user ON envs(user_id);
