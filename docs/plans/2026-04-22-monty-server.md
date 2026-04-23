# Monty Server Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a self-hostable Rust web server that exposes the Monty Python interpreter as a multi-tenant service with GitHub OAuth, API keys, and persistent stateful execution environments backed by Monty snapshots.

**Architecture:** Single Axum binary + SQLite + local disk. Env state is persisted as Monty snapshot bytes on disk; each execution loads the snapshot, mounts the env's files dir into the sandbox, runs, dumps, writes back atomically. Per-env `tokio::Mutex` serializes concurrent ops on the same env. Server-rendered HTML with HTMX — no SPA.

**Tech Stack:** Rust, Axum, sqlx + SQLite, `oauth2` crate (GitHub), `tower-sessions` (cookies), `argon2` (token hashing), Askama templates, HTMX. Monty linked via path dep at `monty/crates/monty`.

**Reference:** See `docs/plans/2026-04-22-monty-server-design.md` for the full architectural decision record.

---

## Repo layout (end state)

```
monty-server/
  Cargo.toml                    # single crate, not a workspace (avoids nested-workspace issues with monty/)
  src/
    main.rs
    config.rs
    db.rs                       # sqlx pool setup
    error.rs                    # AppError + IntoResponse
    migrations/                 # sqlx migrations
    auth/
      mod.rs
      oauth.rs                  # GitHub OAuth flow
      session.rs                # tower-sessions glue
      api_key.rs                # hashing, verification
      middleware.rs             # Bearer + Session extractors
    models/
      mod.rs
      user.rs
      api_key.rs
      env.rs
    envs/
      mod.rs
      storage.rs                # filesystem layout, path security
      executor.rs               # EnvExecutor trait + MontyExecutor impl
      locks.rs                  # DashMap<EnvId, Arc<Mutex<()>>>
    routes/
      mod.rs
      api.rs                    # /api/v1/*
      web.rs                    # UI routes
    templates/                  # Askama .html files
  tests/
    common/mod.rs               # TestApp harness
    health.rs
    auth.rs
    api_keys.rs
    envs.rs
    execute.rs
    files.rs
  migrations/                   # sqlx CLI migrations
  Dockerfile
  fly.toml
  docs/plans/...
```

---

## Phase 0: Bootstrap

### Task 1: Cargo project + /health endpoint

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `tests/common/mod.rs`, `tests/health.rs`, `.gitignore`

**Step 1: Write failing integration test**

`tests/common/mod.rs`:
```rust
use axum::Router;
use monty_server::app;

pub struct TestApp {
    pub router: Router,
}

impl TestApp {
    pub async fn new() -> Self {
        Self { router: app().await }
    }
}
```

`tests/health.rs`:
```rust
mod common;
use axum::{body::Body, http::Request};
use common::TestApp;
use tower::ServiceExt;

#[tokio::test]
async fn health_returns_ok() {
    let app = TestApp::new().await;
    let res = app.router
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}
```

**Step 2: Run — expect fail (crate `monty_server` doesn't exist)**

Run: `cargo test --test health`

**Step 3: Create `Cargo.toml`**

```toml
[package]
name = "monty-server"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "monty-server"
path = "src/main.rs"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["full"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["trace", "fs"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
thiserror = "2"

[dev-dependencies]
tower = { version = "0.5", features = ["util"] }
```

Create `src/lib.rs`:
```rust
use axum::{routing::get, Router};

pub async fn app() -> Router {
    Router::new().route("/health", get(|| async { "ok" }))
}
```

Create `src/main.rs`:
```rust
use monty_server::app;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let app = app().await;
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
```

Create `.gitignore`: `target/`, `/data/`, `*.db`, `*.db-*`, `.env`

**Step 4: Run — expect pass**

Run: `cargo test --test health`

**Step 5: Commit**

```bash
git add Cargo.toml src/ tests/ .gitignore
git commit -m "scaffold: axum crate with /health endpoint and integration test harness"
```

---

### Task 2: Config from env vars

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs`, `src/main.rs`

**Step 1: Write failing test** in `src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn loads_from_env() {
        env::set_var("DATA_DIR", "/tmp/monty-test");
        env::set_var("GITHUB_CLIENT_ID", "cid");
        env::set_var("GITHUB_CLIENT_SECRET", "csec");
        env::set_var("SESSION_SECRET", "a".repeat(64));
        env::set_var("BASE_URL", "http://localhost:3000");
        let c = Config::from_env().unwrap();
        assert_eq!(c.data_dir.to_str().unwrap(), "/tmp/monty-test");
        assert_eq!(c.github_client_id, "cid");
    }
}
```

**Step 2: Run — expect fail**

**Step 3: Implement** `Config { data_dir: PathBuf, github_client_id, github_client_secret, session_secret: [u8; 64], base_url }` with `from_env()` using `std::env::var`. Session secret must be ≥ 64 bytes or return error.

**Step 4: Run — expect pass**

**Step 5: Wire `Config` through `app()` signature and `main.rs`. Update `TestApp::new()` to build a test `Config` with a tmpdir. Run `cargo test`.**

**Step 6: Commit** `feat: configurable Config loaded from env vars`

---

### Task 3: SQLite + sqlx migrations

**Files:**
- Add to `Cargo.toml`: `sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "migrate", "chrono", "uuid", "macros"] }`, `uuid = { version = "1", features = ["v4", "serde"] }`, `chrono = { version = "0.4", features = ["serde"] }`
- Create: `src/db.rs`, `migrations/0001_init.sql`

**Step 1: Write failing test** in `tests/db.rs`:

```rust
#[tokio::test]
async fn migrations_run_and_users_table_exists() {
    let pool = monty_server::db::connect(":memory:").await.unwrap();
    monty_server::db::migrate(&pool).await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users").fetch_one(&pool).await.unwrap();
    assert_eq!(count.0, 0);
}
```

**Step 2: Run — expect fail**

**Step 3: Create migration `migrations/0001_init.sql`:**

```sql
CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    github_id INTEGER NOT NULL UNIQUE,
    github_login TEXT NOT NULL,
    email TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

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
```

Create `src/db.rs` with `connect(url)` → `SqlitePool` (enable foreign keys via `PRAGMA foreign_keys = ON;`, WAL mode) and `migrate(&pool)` using `sqlx::migrate!("./migrations")`.

**Step 4: Run — expect pass**

**Step 5: Wire `pool` into `AppState` (struct passed as Axum `State`). Update `TestApp::new()` to use `":memory:"`. Re-run all tests.**

**Step 6: Commit** `feat: sqlite + sqlx migrations with users/api_keys/envs schema`

---

## Phase 1: Auth

### Task 4: User storage

**Files:**
- Create: `src/models/user.rs`
- Modify: `src/models/mod.rs`

**Step 1: Write failing test** in `tests/users.rs`:

```rust
#[tokio::test]
async fn upsert_user_creates_and_then_updates() {
    let pool = test_pool().await;
    let u1 = User::upsert(&pool, 42, "octocat", Some("o@example.com")).await.unwrap();
    let u2 = User::upsert(&pool, 42, "octocat-renamed", None).await.unwrap();
    assert_eq!(u1.id, u2.id);
    assert_eq!(u2.github_login, "octocat-renamed");
}
```

**Step 2: Run — expect fail**

**Step 3: Implement** `struct User { id: i64, github_id: i64, github_login: String, email: Option<String>, created_at: DateTime<Utc> }` with `upsert` using `INSERT ... ON CONFLICT(github_id) DO UPDATE SET ...`. Also `find_by_id`.

**Step 4: Run — expect pass**

**Step 5: Commit** `feat: User model with upsert`

---

### Task 5: API key model with hashing

**Files:**
- Add to `Cargo.toml`: `argon2 = "0.5"`, `rand = "0.8"`, `base64 = "0.22"`
- Create: `src/models/api_key.rs`

**Step 1: Write failing test**:

```rust
#[tokio::test]
async fn create_returns_plaintext_once_and_hash_verifies() {
    let pool = test_pool().await;
    let user = User::upsert(&pool, 1, "u", None).await.unwrap();

    let (key, plaintext) = ApiKey::create(&pool, user.id, "laptop").await.unwrap();
    assert!(plaintext.starts_with("mk_"));
    assert_eq!(plaintext.len(), 3 + 32); // "mk_" + 32 url-safe chars

    // Can verify plaintext against stored hash
    let found = ApiKey::verify(&pool, &plaintext).await.unwrap().unwrap();
    assert_eq!(found.user_id, user.id);

    // Wrong token doesn't verify
    assert!(ApiKey::verify(&pool, "mk_wrongwrongwrong").await.unwrap().is_none());
}
```

**Step 2: Run — expect fail**

**Step 3: Implement**

Token format: `"mk_" + base64url(rand::random::<[u8; 24]>())` → 32 url-safe chars after prefix. Store `prefix = &plaintext[..11]` (e.g., `mk_abc12345`), `token_hash = argon2id(plaintext)`. `verify` looks up candidates by prefix (usually 1), runs argon2 verify.

```rust
pub struct ApiKey {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl ApiKey {
    pub async fn create(pool: &SqlitePool, user_id: i64, name: &str) -> Result<(Self, String)> { ... }
    pub async fn verify(pool: &SqlitePool, plaintext: &str) -> Result<Option<Self>> { ... }
    pub async fn list_for_user(pool: &SqlitePool, user_id: i64) -> Result<Vec<Self>> { ... }
    pub async fn delete(pool: &SqlitePool, user_id: i64, id: i64) -> Result<bool> { ... }
}
```

**Step 4: Run — expect pass**

**Step 5: Commit** `feat: API key creation, verification, and lifecycle`

---

### Task 6: GitHub OAuth flow + sessions

**Files:**
- Add to `Cargo.toml`: `oauth2 = "4.4"`, `tower-sessions = "0.13"`, `tower-sessions-sqlx-store = { version = "0.14", features = ["sqlite"] }`, `reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }`
- Create: `src/auth/oauth.rs`, `src/auth/session.rs`, `src/auth/mod.rs`
- Modify: `src/lib.rs` to install session layer and mount routes

**Step 1: Write failing test** in `tests/auth.rs`:

```rust
#[tokio::test]
async fn login_redirects_to_github_with_state_cookie() {
    let app = TestApp::new().await;
    let res = app.get("/auth/login").await;
    assert_eq!(res.status(), 302);
    let loc = res.headers().get("location").unwrap().to_str().unwrap();
    assert!(loc.starts_with("https://github.com/login/oauth/authorize?"));
    assert!(loc.contains("state="));
    // state is persisted in session for CSRF check
    assert!(res.headers().get("set-cookie").is_some());
}
```

**Step 2: Run — expect fail**

**Step 3: Implement**

- `build_oauth_client(cfg) -> BasicClient` — GitHub authorize URL, token URL, redirect URL = `{base_url}/auth/callback`.
- `GET /auth/login`: generate `(auth_url, csrf_state)` via `client.authorize_url`. Store `csrf_state` in session. 302 to `auth_url`. Request scope `read:user user:email`.
- `GET /auth/callback?code=&state=`: verify state matches session, exchange code for token via `client.exchange_code`. Call GitHub `GET /user` and `GET /user/emails` with bearer token (via reqwest). Upsert user, store `user_id` in session. Redirect to `/dashboard`.
- `POST /auth/logout`: clear session, redirect to `/`.

Install `SessionManagerLayer::new(SqliteStore::new(pool.clone()))` with 7-day expiry, signed cookie using `cfg.session_secret`.

**Step 4: Run — expect pass (on the redirect test)**

**Step 5: Add a callback test that mocks the GitHub HTTP calls via `wiremock`** (add `wiremock = "0.6"` to dev-deps). Inject the GitHub API base URL as a `Config` field so tests can point it at the mock server. The callback test:
- Pre-seeds the session with a known state
- Starts a wiremock mounting `/login/oauth/access_token` → `{"access_token": "gho_x"}`, `/user` → `{"id": 42, "login": "octocat"}`, `/user/emails` → `[{"email":"o@example.com","primary":true}]`
- Hits `/auth/callback?code=x&state=known`
- Asserts 302 to `/dashboard` and a user row exists for github_id=42

**Step 6: Commit** `feat: github oauth sign-in with session store`

---

### Task 7: Auth extractors (session + bearer) + AuthUser

**Files:**
- Create: `src/auth/middleware.rs`

**Step 1: Write failing test** in `tests/auth.rs`:

```rust
#[tokio::test]
async fn bearer_token_auth_accepts_valid_rejects_invalid() {
    let app = TestApp::new().await;
    let user = app.create_test_user().await;
    let (_, token) = app.create_api_key(user.id).await;

    // Valid
    let r = app.get_with_bearer("/api/v1/envs", &token).await;
    assert_eq!(r.status(), 200);

    // Invalid
    let r = app.get_with_bearer("/api/v1/envs", "mk_nope").await;
    assert_eq!(r.status(), 401);

    // Missing
    let r = app.get("/api/v1/envs").await;
    assert_eq!(r.status(), 401);
}
```

(Add a stub `/api/v1/envs` returning `[]` if needed.)

**Step 2: Run — expect fail**

**Step 3: Implement `AuthUser(pub User)` extractor.**

Two `FromRequestParts` impls via a single extractor that:
- For paths starting with `/api/v1/`: read `Authorization: Bearer <t>`, call `ApiKey::verify`, update `last_used_at`, load `User`. On failure → 401.
- Else: read session, get `user_id`, load `User`. On failure → redirect to `/auth/login` (for UI) or 401 (for JSON routes). Use two separate extractors (`SessionUser`, `BearerUser`) rather than branching inside one — cleaner.

**Step 4: Run — expect pass**

**Step 5: Commit** `feat: session + bearer auth extractors`

---

## Phase 2: API keys UI

### Task 8: API key management routes

**Files:**
- Create: `src/routes/web.rs` (start adding handlers), `src/templates/dashboard.html`, `src/templates/base.html`

**Step 1: Write failing test**:

```rust
#[tokio::test]
async fn api_key_create_list_delete_round_trip() {
    let app = TestApp::new().await;
    let user = app.create_test_user().await;
    let session = app.sign_in_as(&user).await;  // returns a cookie

    // Create
    let res = app.post_form("/api-keys", &[("name", "laptop")], &session).await;
    assert_eq!(res.status(), 200);  // HTMX returns a fragment showing the one-time plaintext
    let body = body_string(res).await;
    assert!(body.contains("mk_"), "must display token once");

    // List
    let res = app.get_with_session("/dashboard", &session).await;
    let body = body_string(res).await;
    assert!(body.contains("laptop"));
    assert!(!body.contains(&body[..]));  // plaintext must NOT appear again

    // Delete
    let keys = ApiKey::list_for_user(&app.pool, user.id).await.unwrap();
    let res = app.post_form(&format!("/api-keys/{}/delete", keys[0].id), &[], &session).await;
    assert_eq!(res.status(), 200);
    assert_eq!(ApiKey::list_for_user(&app.pool, user.id).await.unwrap().len(), 0);
}
```

**Step 2: Run — expect fail**

**Step 3: Implement**

Add `askama = { version = "0.12", features = ["with-axum"] }` + `askama_axum = "0.4"`.

Handlers:
- `GET /dashboard` — render `dashboard.html` with `{user, api_keys, envs}`
- `POST /api-keys` — form `{name}` → create, return HTMX fragment with the one-time plaintext + "copy now, you won't see this again"
- `POST /api-keys/:id/delete` — delete, return HTMX fragment (updated list)

**Step 4: Run — expect pass**

**Step 5: Commit** `feat: dashboard with API key create/list/delete via htmx`

---

## Phase 3: Envs (metadata + filesystem + locks)

### Task 9: Env filesystem storage

**Files:**
- Create: `src/envs/storage.rs`

**Step 1: Write failing test** in `tests/env_storage.rs`:

```rust
#[tokio::test]
async fn create_env_dir_creates_files_dir_and_empty_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    let id = "01HXYZ...".to_string();

    storage.create(&id).await.unwrap();

    assert!(tmp.path().join("envs").join(&id).join("files").is_dir());
    assert!(tmp.path().join("envs").join(&id).join("snapshot.bin").is_file());
}

#[tokio::test]
async fn delete_env_removes_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    let id = "abc".to_string();
    storage.create(&id).await.unwrap();
    storage.delete(&id).await.unwrap();
    assert!(!tmp.path().join("envs").join(&id).exists());
}

#[tokio::test]
async fn write_snapshot_is_atomic() {
    // Write via tempfile + rename; partial write must not corrupt existing snapshot
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    storage.create("e").await.unwrap();
    storage.write_snapshot("e", b"v1").await.unwrap();
    storage.write_snapshot("e", b"v2_longer").await.unwrap();
    assert_eq!(storage.read_snapshot("e").await.unwrap(), b"v2_longer");
}
```

**Step 2: Run — expect fail**

**Step 3: Implement**

```rust
pub struct EnvStorage { root: PathBuf }

impl EnvStorage {
    pub fn new(data_dir: &Path) -> Self { ... }
    pub async fn create(&self, id: &str) -> Result<()>;
    pub async fn delete(&self, id: &str) -> Result<()>;
    pub async fn read_snapshot(&self, id: &str) -> Result<Vec<u8>>;
    pub async fn write_snapshot(&self, id: &str, bytes: &[u8]) -> Result<()>;
    pub fn files_dir(&self, id: &str) -> PathBuf;
}
```

Initial snapshot on create: write the result of `MontyRun::new("", "script.py", vec![]).dump()` (or simply empty bytes as a sentinel; if empty, executor knows to build a fresh runner). Keep this decision minimal — flag it as "executor initializes on first use if snapshot is empty".

Atomic write: write to `snapshot.bin.tmp`, fsync, rename.

**Security:** `id` must match `^[a-z0-9-]{8,}$` — enforced before any `Path::join` call. Never take user input directly into `files_dir`.

**Step 4: Run — expect pass**

**Step 5: Commit** `feat: env filesystem storage with atomic snapshot writes`

---

### Task 10: Env model + CRUD API

**Files:**
- Create: `src/models/env.rs`, `src/routes/api.rs`

**Step 1: Write failing tests** in `tests/envs.rs`:

```rust
#[tokio::test]
async fn create_env_via_api() {
    let app = TestApp::new().await;
    let (user, token) = app.make_user_with_token().await;
    let res = app.post_json_with_bearer("/api/v1/envs", json!({"name": "scratch"}), &token).await;
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = body_json(res).await;
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["name"], "scratch");
    // Directory exists on disk
    assert!(app.data_dir.join("envs").join(&id).join("files").is_dir());
}

#[tokio::test]
async fn cannot_see_other_users_envs() { ... }

#[tokio::test]
async fn delete_env_removes_row_and_directory() { ... }
```

**Step 2: Run — expect fail**

**Step 3: Implement**

Env model: `id: String (uuidv4 hex)`, `user_id`, `name`, timestamps. Methods: `create`, `list_for_user`, `find_for_user(user_id, id)`, `delete_for_user`.

API handlers in `src/routes/api.rs`:
- `POST /api/v1/envs` `{name}` — generate UUID, call `EnvStorage::create(id)`, insert row, return `{id, name, created_at}`.
- `GET /api/v1/envs` — list caller's envs.
- `DELETE /api/v1/envs/:id` — verify ownership, call storage.delete, delete row.

Mount under an `api_router()` with the `BearerUser` extractor attached via a layer.

**Step 4: Run — expect pass**

**Step 5: Commit** `feat: env CRUD API with per-user isolation`

---

### Task 11: Per-env lock registry

**Files:**
- Add to `Cargo.toml`: `dashmap = "6"`
- Create: `src/envs/locks.rs`

**Step 1: Write failing test** in `tests/env_locks.rs`:

```rust
#[tokio::test]
async fn concurrent_ops_on_same_env_are_serialized() {
    let locks = LockRegistry::new();
    let counter = Arc::new(AtomicI32::new(0));
    let max_seen = Arc::new(AtomicI32::new(0));

    let tasks: Vec<_> = (0..20).map(|_| {
        let locks = locks.clone();
        let counter = counter.clone();
        let max_seen = max_seen.clone();
        tokio::spawn(async move {
            let _guard = locks.acquire("env1").await;
            let now = counter.fetch_add(1, SeqCst) + 1;
            max_seen.fetch_max(now, SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
            counter.fetch_sub(1, SeqCst);
        })
    }).collect();
    for t in tasks { t.await.unwrap(); }
    assert_eq!(max_seen.load(SeqCst), 1, "never more than 1 holder");
}

#[tokio::test]
async fn different_envs_do_not_block() {
    // 10 tasks on 10 different env ids all hold at once
}

#[tokio::test]
async fn release_decrements_refcount_and_evicts_when_zero() {
    let locks = LockRegistry::new();
    {
        let _g = locks.acquire("e").await;
        assert_eq!(locks.len(), 1);
    }
    assert_eq!(locks.len(), 0, "entry evicted when last holder drops");
}
```

**Step 2: Run — expect fail**

**Step 3: Implement**

```rust
#[derive(Clone)]
pub struct LockRegistry {
    inner: Arc<DashMap<String, Arc<RefCountedMutex>>>,
}

struct RefCountedMutex {
    mutex: Mutex<()>,
    holders: AtomicUsize,
}

pub struct LockGuard<'a> {
    _permit: OwnedMutexGuard<()>,
    registry: LockRegistry,
    key: String,
}

impl LockRegistry {
    pub async fn acquire(&self, id: &str) -> LockGuard { ... }
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        // decrement holders; if 0, remove from map
    }
}
```

The eviction-on-zero prevents unbounded growth of the map without requiring a background reaper. Use `entry(key).or_insert_with(...)` + `holders.fetch_add`; on drop, `fetch_sub(1) == 1` means we're the last → `remove_if` on the DashMap entry.

**Step 4: Run — expect pass**

**Step 5: Commit** `feat: per-env lock registry with automatic eviction`

---

## Phase 4: Monty integration

### Task 12: EnvExecutor trait + Monty-backed impl

**Files:**
- Add to `Cargo.toml`: `monty = { path = "monty/crates/monty" }`
- Create: `src/envs/executor.rs`

**Note on Cargo workspaces:** The top-level `Cargo.toml` is a single-crate manifest (no `[workspace]` section), and `monty/Cargo.toml` already defines its own workspace. This avoids the nested-workspace error Cargo raises when an outer workspace's globs would capture an inner one. If you hit `current package believes it's in a workspace when it's not`, add `workspace = { }` (empty) to disambiguate or explicitly `exclude = ["monty"]` in a workspace block.

**Step 1: Write failing test** in `tests/executor.rs`:

```rust
#[tokio::test]
async fn stateful_execution_preserves_variables_across_calls() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    let locks = LockRegistry::new();
    let exec = MontyExecutor::new(storage.clone(), locks);
    storage.create("e").await.unwrap();

    let out1 = exec.execute("e", "x = 40 + 2").await.unwrap();
    assert_eq!(out1.stderr, "");

    let out2 = exec.execute("e", "print(x)").await.unwrap();
    assert_eq!(out2.stdout.trim(), "42");
}

#[tokio::test]
async fn one_off_does_not_persist() {
    let exec = ...;
    storage.create("e").await.unwrap();
    exec.execute_one_off("x = 99").await.unwrap();
    let out = exec.execute("e", "print('x' in dir())").await.unwrap();
    assert_eq!(out.stdout.trim(), "False");
}

#[tokio::test]
async fn sandbox_cannot_read_outside_files_dir() {
    // Place a secret file at data_dir/secret.txt
    // Try to read ../secret.txt via code — must fail
}

#[tokio::test]
async fn sandbox_can_read_write_its_files_dir() {
    exec.execute("e", "open('/mnt/hello.txt', 'w').write('hi')").await.unwrap();
    let contents = std::fs::read_to_string(storage.files_dir("e").join("hello.txt")).unwrap();
    assert_eq!(contents, "hi");
}
```

**Step 2: Run — expect fail**

**Step 3: Implement**

```rust
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub result: Option<serde_json::Value>,   // repr of the last expression
    pub error: Option<String>,               // Python traceback, if any
}

#[async_trait::async_trait]
pub trait EnvExecutor: Send + Sync + 'static {
    async fn execute(&self, env_id: &str, code: &str) -> Result<ExecOutput>;
    async fn execute_one_off(&self, code: &str) -> Result<ExecOutput>;
}

pub struct MontyExecutor { storage: EnvStorage, locks: LockRegistry }
```

For stateful execution:
1. `let _guard = locks.acquire(env_id).await;`
2. Read snapshot bytes. If empty, create a fresh `MontyRun::new(...)` with the caller's code; else `MontyRun::load(&bytes)?` and then a second step to run the code in the loaded context. **Open question for the implementer:** the current Monty snapshot API dumps *parsed code*, not live globals — confirm what the "stateful env" abstraction looks like in Monty's current API. If snapshots don't yet carry post-execution globals, implement stateful mode by concatenating a script buffer (every execute() appends to a persisted `.py` file and re-runs it with a resource cap). Document which route was taken in the commit message.
3. Mount `storage.files_dir(env_id)` at virtual path `/mnt` via Monty's `MountTable` (ReadWrite). Verify exact API name against `monty/crates/monty/src/fs/` before wiring.
4. Run with a resource tracker: default limits — 128 MB, 30 s wallclock, 10M allocations. Configurable via `Config`.
5. Capture stdout/stderr via Monty's `PrintWriter` — collect into `Vec<u8>`, decode as UTF-8 (lossy).
6. Dump updated state, write atomically back via `storage.write_snapshot`.

Monty's Rust API is synchronous; wrap the execution step in `tokio::task::spawn_blocking` so long-running interpreter work doesn't block the async runtime.

Error path: on Monty runtime error, do **not** write back the snapshot (preserve last-good state). Return the traceback as `ExecOutput.error`.

**Step 4: Run — expect pass (all four tests)**

**Step 5: Commit** `feat: monty-backed env executor with persistent snapshots and sandboxed filesystem mounts`

---

### Task 13: Execute endpoints

**Files:**
- Modify: `src/routes/api.rs`

**Step 1: Write failing test**:

```rust
#[tokio::test]
async fn execute_endpoint_runs_code_in_env() {
    let app = TestApp::new().await;
    let (user, token) = app.make_user_with_token().await;
    let env = app.create_env(&token, "scratch").await;

    let r = app.post_json_with_bearer(
        &format!("/api/v1/envs/{}/execute", env.id),
        json!({"code": "2 + 2"}),
        &token,
    ).await;
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = body_json(r).await;
    assert_eq!(body["result"], json!("4"));
}

#[tokio::test]
async fn one_off_execute_endpoint() {
    let app = TestApp::new().await;
    let (_, token) = app.make_user_with_token().await;
    let r = app.post_json_with_bearer("/api/v1/execute", json!({"code": "1 + 1"}), &token).await;
    assert_eq!(r.status(), 200);
    assert_eq!(body_json(r).await["result"], json!("2"));
}
```

**Step 2: Run — expect fail**

**Step 3: Implement**

- `POST /api/v1/envs/:id/execute` `{code}` — verify ownership, call `executor.execute(id, code)`, return `ExecOutput` as JSON.
- `POST /api/v1/execute` `{code}` — call `executor.execute_one_off(code)`.

**Step 4: Run — expect pass**

**Step 5: Commit** `feat: execute endpoints for stateful and one-off code`

---

## Phase 5: File management

### Task 14: File list / upload / download / delete endpoints

**Files:**
- Modify: `src/envs/storage.rs` (add file ops), `src/routes/api.rs`
- Add to `Cargo.toml`: `axum = { version = "0.7", features = ["multipart"] }`

**Step 1: Write failing tests** — four tests:
- list empty, then after upload
- upload a file, GET it back byte-identical
- delete a file
- **path traversal attempt must fail** — `GET /api/v1/envs/:id/files/../../../etc/passwd` → 400, and `..` in upload filename is rejected

**Step 2: Run — expect fail**

**Step 3: Implement**

Add to `EnvStorage`:
```rust
pub async fn list_files(&self, id: &str) -> Result<Vec<FileEntry>>;
pub async fn write_file(&self, id: &str, name: &str, bytes: &[u8]) -> Result<()>;
pub async fn read_file(&self, id: &str, name: &str) -> Result<Vec<u8>>;
pub async fn delete_file(&self, id: &str, name: &str) -> Result<bool>;
```

Each takes the raw filename. Before joining, validate with a strict regex (`^[A-Za-z0-9._-]+(\/[A-Za-z0-9._-]+)*$`), reject `..` components, then canonicalize the resulting path and assert it starts with `files_dir(id).canonicalize()`. **This is the security boundary outside Monty — treat it with the same care as `path_security.rs` in Monty itself.**

File uploads: take the env lock (mirrors execution's lock — prevents upload racing with a running execution).

**Step 4: Run — expect pass**

**Step 5: Commit** `feat: file list/upload/download/delete with path traversal protection`

---

## Phase 6: Web UI — env browser & REPL

### Task 15: Env list + detail view (static sections)

**Files:**
- Create: `src/templates/env_list.html`, `src/templates/env_detail.html`
- Modify: `src/routes/web.rs`

**Step 1: Write failing test** — a smoke test that `/envs/:id` for the owner renders 200 and contains the env name and the list of files.

**Step 2: Implement**

- `GET /envs` (redirect from dashboard already) — listing
- `GET /envs/:id` — detail page: file list, upload form, REPL textarea, state panel

**Step 3: Run — expect pass**

**Step 4: Commit** `feat: env detail page with file browser`

---

### Task 16: HTMX REPL + state view

**Files:**
- Modify: `src/routes/web.rs`, templates

**Step 1: Write failing test** — POST `/envs/:id/repl` with a code fragment returns an HTMX fragment containing the output. A second POST reads a variable set in the first.

**Step 2: Implement**

- `POST /envs/:id/repl` (session-auth) `{code}` — calls `executor.execute(id, code)`. Returns an HTMX partial that gets appended to the transcript.
- `GET /envs/:id/state` — runs a one-off fragment `{k: repr(v) for k, v in globals().items() if not k.startswith('_')}` against the env and renders a two-column table. Re-fetched by HTMX on interval or after each REPL submission.

**Step 3: Run — expect pass**

**Step 4: Commit** `feat: htmx-driven repl and live state view`

---

### Task 17: File upload UI

**Files:** templates + `src/routes/web.rs`

Simple multipart form that POSTs to the existing upload handler, returning an HTMX fragment re-rendering the file list. Include a file-size guard (reject > 50 MB by default; configurable).

Commit: `feat: file upload ui`

---

## Phase 7: Deploy

### Task 18: Dockerfile

**Files:** `Dockerfile`

Multi-stage:
```dockerfile
FROM rust:1 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin monty-server

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/monty-server /monty-server
COPY --from=builder /app/migrations /migrations
ENV DATA_DIR=/data
EXPOSE 3000
CMD ["/monty-server"]
```

Build locally: `docker build -t monty-server .` and smoke-test against a tmp volume.

Commit: `build: multi-stage distroless dockerfile`

### Task 19: fly.toml

**Files:** `fly.toml`

Single-machine config with a 10 GB volume mounted at `/data`. Document required secrets (`GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, `SESSION_SECRET`) and how to set them.

Commit: `build: fly.toml with persistent volume`

---

## Review / polish (not separate tasks — do throughout)

- **Error model:** Single `AppError` enum with `IntoResponse`. JSON routes return JSON errors; UI routes return an HTML error page.
- **Logging:** `tracing` spans per request, log user_id + env_id on execution.
- **Rate limits:** `tower_governor` on `/auth/login` and `/api/v1/execute*`. Default: 10/min per IP on auth, 60/min per API key on execute.
- **Size caps:** request body 5 MB (code), file upload 50 MB, env files total 500 MB.

---

## Open questions flagged during design (decide as you go)

1. **Monty snapshot semantics for globals** (Task 12) — confirm whether current Monty dumps globals across `run()` calls or whether stateful-env mode requires the "append to script buffer" workaround. If the latter, consider contributing a `globals()` snapshot API upstream.
2. **Exact Monty resource-tracker names** — the design references `NoLimitTracker`; the real impl should pick a bounded tracker. Verify against `monty/crates/monty/src/` and pick the right type.
3. **Max concurrent envs per user** — no limit in MVP; add one if abuse becomes real.
