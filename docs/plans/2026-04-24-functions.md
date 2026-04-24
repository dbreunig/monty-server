# Functions Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a third first-class object — `functions` — that exposes a stored Python `def handler(...)` as an authed or public HTTP endpoint, running either stateless or bound to an existing env.

**Architecture:** New `functions` and `function_calls` tables. HTTP surface is two call routes (`POST /fn/:slug` authed, `POST /fn/p/:public_token` public) plus CRUD under `/api/v1/functions`. Execution reuses `MontyRepl` + the per-env lock registry. A Rust-side textual parser reads `def handler(...)` to build a schema for request validation. UI mirrors the existing env dashboard + detail patterns.

**Tech Stack:** Rust, Axum, sqlx (SQLite), askama (HTMX templates), Monty interpreter, existing `EnvStorage` / `LockRegistry` infrastructure.

**Signature introspection — decided upfront.** `monty::Signature` is `pub(crate)` and not reachable from monty-server; Monty's typeshed does not expose `inspect`. v1 introspection is a hand-rolled Rust parser over the `def handler(...)` source line, supporting annotations `str | int | float | bool | list | dict | None | Any`. Unknown or missing annotations fall back to `any` (accept anything). Defaults from the source → parameter is optional.

**Relevant files already in place (read these before starting):**
- `src/models/env.rs`, `src/models/api_key.rs` — model patterns to copy
- `src/routes/api.rs`, `src/routes/web.rs` — route patterns
- `src/envs/executor.rs` — `MontyRepl` feed/dump flow
- `src/envs/locks.rs` — env lock registry
- `migrations/0001_init.sql`, `migrations/0002_repl_calls.sql` — migration patterns
- `tests/common/` and `tests/env_ui.rs` — test harness (`TestApp::new()`, `sign_in`)
- `templates/_env_list.html`, `templates/env_detail.html` — UI to mirror
- `docs/plans/2026-04-24-functions-design.md` — the design this plan implements

---

## Task 1: Migration for `functions` table

**Files:**
- Create: `migrations/0003_functions.sql`

**Step 1: Write the migration**

```sql
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
```

**Step 2: Verify it runs**

Run: `cargo test --test env_ui -- root_redirects_to_login_when_signed_out`
Expected: PASS (test harness runs migrations on every test; a broken migration would fail here).

**Step 3: Commit**

```bash
git add migrations/0003_functions.sql
git commit -m "feat(functions): add functions table"
```

---

## Task 2: Migration for `function_calls` table

**Files:**
- Create: `migrations/0004_function_calls.sql`

**Step 1: Write the migration**

```sql
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
```

**Step 2: Verify it runs**

Run: `cargo test --test env_ui -- root_redirects_to_login_when_signed_out`
Expected: PASS.

**Step 3: Commit**

```bash
git add migrations/0004_function_calls.sql
git commit -m "feat(functions): add function_calls table"
```

---

## Task 3: Signature parser — failing test

**Files:**
- Create: `src/functions/mod.rs`
- Create: `src/functions/signature.rs`
- Modify: `src/lib.rs` (add `pub mod functions;`)

**Step 1: Stub the module + add to lib**

`src/functions/mod.rs`:

```rust
pub mod signature;
```

Add to `src/lib.rs` after `pub mod envs;`:

```rust
pub mod functions;
```

**Step 2: Write the failing test**

`src/functions/signature.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    pub name: String,
    pub type_hint: TypeHint,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeHint {
    Str,
    Int,
    Float,
    Bool,
    List,
    Dict,
    None_,
    Any,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HandlerSignature {
    pub params: Vec<ParamSpec>,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("code must define `def handler(...)`")]
    NoHandler,
    #[error("malformed handler signature: {0}")]
    Malformed(String),
}

pub fn parse_handler(_code: &str) -> Result<HandlerSignature, ParseError> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_typed_signature() {
        let code = "def handler(name: str, count: int) -> dict:\n    return {}\n";
        let sig = parse_handler(code).unwrap();
        assert_eq!(
            sig.params,
            vec![
                ParamSpec { name: "name".into(), type_hint: TypeHint::Str, required: true },
                ParamSpec { name: "count".into(), type_hint: TypeHint::Int, required: true },
            ],
        );
    }
}
```

**Step 3: Run test to verify it fails**

Run: `cargo test --lib functions::signature::tests::parses_simple_typed_signature`
Expected: FAIL (`unimplemented!()` panics).

**Step 4: Commit**

```bash
git add src/lib.rs src/functions/mod.rs src/functions/signature.rs
git commit -m "test(functions): scaffold signature parser with failing test"
```

---

## Task 4: Signature parser — minimal implementation

**Files:**
- Modify: `src/functions/signature.rs`
- Modify: `Cargo.toml` — add `thiserror = "1"` if not already present (check first with `grep thiserror Cargo.toml`)

**Step 1: Implement `parse_handler`**

Replace the `unimplemented!()` body with a hand-rolled parser. Strategy: find the line `def handler(`, collect until the matching `)`, then split parameters at top-level commas.

```rust
pub fn parse_handler(code: &str) -> Result<HandlerSignature, ParseError> {
    let start = find_handler_def(code).ok_or(ParseError::NoHandler)?;
    let (inside, _rest) = split_parens(&code[start..])
        .ok_or_else(|| ParseError::Malformed("unclosed `(`".into()))?;
    let params = split_top_level_commas(inside)
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .map(parse_param)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HandlerSignature { params })
}

fn find_handler_def(code: &str) -> Option<usize> {
    // Locate `def handler(` at any indentation / anywhere in the file.
    for (i, line) in code.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("def handler(") || trimmed.starts_with("async def handler(") {
            let line_start = code.lines().take(i).map(|l| l.len() + 1).sum::<usize>();
            let offset_in_line = line.find("handler(")?;
            return Some(line_start + offset_in_line + "handler".len());
        }
    }
    None
}

fn split_parens(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&s[1..i], &s[i + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn parse_param(raw: &str) -> Result<ParamSpec, ParseError> {
    let t = raw.trim();
    if t.starts_with('*') {
        return Err(ParseError::Malformed(
            "*args / **kwargs not supported in v1".into(),
        ));
    }
    // `name[: type][= default]`
    let (head, default) = match t.split_once('=') {
        Some((h, d)) => (h.trim(), Some(d.trim())),
        None => (t, None),
    };
    let (name, type_hint) = match head.split_once(':') {
        Some((n, ty)) => (n.trim().to_string(), parse_type(ty.trim())),
        None => (head.to_string(), TypeHint::Any),
    };
    if name.is_empty() {
        return Err(ParseError::Malformed("empty parameter name".into()));
    }
    Ok(ParamSpec {
        name,
        type_hint,
        required: default.is_none(),
    })
}

fn parse_type(s: &str) -> TypeHint {
    match s {
        "str" => TypeHint::Str,
        "int" => TypeHint::Int,
        "float" => TypeHint::Float,
        "bool" => TypeHint::Bool,
        "list" => TypeHint::List,
        "dict" => TypeHint::Dict,
        "None" => TypeHint::None_,
        _ => TypeHint::Any,
    }
}
```

**Step 2: Run the existing test**

Run: `cargo test --lib functions::signature::tests::parses_simple_typed_signature`
Expected: PASS.

**Step 3: Add more tests covering edge cases**

Append to the `tests` module:

```rust
#[test]
fn no_handler_means_error() {
    let code = "def other(): pass\n";
    assert!(matches!(parse_handler(code), Err(ParseError::NoHandler)));
}

#[test]
fn default_makes_param_optional() {
    let code = "def handler(x: int = 5): pass\n";
    let sig = parse_handler(code).unwrap();
    assert!(!sig.params[0].required);
}

#[test]
fn unknown_type_falls_back_to_any() {
    let code = "def handler(x: SomeModel): pass\n";
    let sig = parse_handler(code).unwrap();
    assert_eq!(sig.params[0].type_hint, TypeHint::Any);
}

#[test]
fn missing_annotation_is_any() {
    let code = "def handler(x): pass\n";
    let sig = parse_handler(code).unwrap();
    assert_eq!(sig.params[0].type_hint, TypeHint::Any);
}

#[test]
fn rejects_varargs() {
    let code = "def handler(*args): pass\n";
    assert!(matches!(parse_handler(code), Err(ParseError::Malformed(_))));
}

#[test]
fn zero_params_ok() {
    let code = "def handler():\n    return 1\n";
    let sig = parse_handler(code).unwrap();
    assert!(sig.params.is_empty());
}
```

**Step 4: Run all tests**

Run: `cargo test --lib functions::signature`
Expected: all PASS.

**Step 5: Commit**

```bash
git add src/functions/signature.rs Cargo.toml
git commit -m "feat(functions): implement handler signature parser"
```

---

## Task 5: Request validator — failing test

**Files:**
- Create: `src/functions/validate.rs`
- Modify: `src/functions/mod.rs` — add `pub mod validate;`

**Step 1: Write the failing test**

`src/functions/validate.rs`:

```rust
use crate::functions::signature::{HandlerSignature, ParamSpec, TypeHint};
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Serialize)]
pub struct SchemaError {
    pub error: &'static str,
    pub expected: Map<String, Value>,
    pub missing: Vec<String>,
    pub unexpected: Vec<String>,
    pub wrong_type: Vec<String>,
}

pub fn validate(
    sig: &HandlerSignature,
    body: &Value,
) -> Result<Map<String, Value>, SchemaError> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sig(params: Vec<(&str, TypeHint, bool)>) -> HandlerSignature {
        HandlerSignature {
            params: params
                .into_iter()
                .map(|(n, t, r)| ParamSpec { name: n.into(), type_hint: t, required: r })
                .collect(),
        }
    }

    #[test]
    fn accepts_matching_body() {
        let s = sig(vec![("name", TypeHint::Str, true), ("count", TypeHint::Int, true)]);
        let body = json!({ "name": "hi", "count": 3 });
        let kw = validate(&s, &body).unwrap();
        assert_eq!(kw["name"], json!("hi"));
        assert_eq!(kw["count"], json!(3));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib functions::validate::tests::accepts_matching_body`
Expected: FAIL.

**Step 3: Commit**

```bash
git add src/functions/validate.rs src/functions/mod.rs
git commit -m "test(functions): scaffold request validator"
```

---

## Task 6: Request validator — implementation

**Files:**
- Modify: `src/functions/validate.rs`

**Step 1: Implement `validate`**

```rust
pub fn validate(
    sig: &HandlerSignature,
    body: &Value,
) -> Result<Map<String, Value>, SchemaError> {
    let expected_map: Map<String, Value> = sig
        .params
        .iter()
        .map(|p| (p.name.clone(), Value::String(type_hint_name(&p.type_hint).into())))
        .collect();

    let obj = match body {
        Value::Object(m) => m,
        _ => {
            return Err(SchemaError {
                error: "invalid input",
                expected: expected_map,
                missing: sig.params.iter().filter(|p| p.required).map(|p| p.name.clone()).collect(),
                unexpected: vec![],
                wrong_type: vec![],
            });
        }
    };

    let mut missing = Vec::new();
    let mut wrong_type = Vec::new();
    let mut kwargs: Map<String, Value> = Map::new();

    for p in &sig.params {
        match obj.get(&p.name) {
            Some(v) => {
                if !matches_type(&p.type_hint, v) {
                    wrong_type.push(p.name.clone());
                } else {
                    kwargs.insert(p.name.clone(), v.clone());
                }
            }
            None if p.required => missing.push(p.name.clone()),
            None => {} // optional with default — let Python apply the default
        }
    }

    let param_names: std::collections::HashSet<&str> =
        sig.params.iter().map(|p| p.name.as_str()).collect();
    let unexpected: Vec<String> = obj
        .keys()
        .filter(|k| !param_names.contains(k.as_str()))
        .cloned()
        .collect();

    if !missing.is_empty() || !wrong_type.is_empty() || !unexpected.is_empty() {
        return Err(SchemaError {
            error: "invalid input",
            expected: expected_map,
            missing,
            unexpected,
            wrong_type,
        });
    }

    Ok(kwargs)
}

fn type_hint_name(t: &TypeHint) -> &'static str {
    match t {
        TypeHint::Str => "str",
        TypeHint::Int => "int",
        TypeHint::Float => "float",
        TypeHint::Bool => "bool",
        TypeHint::List => "list",
        TypeHint::Dict => "dict",
        TypeHint::None_ => "None",
        TypeHint::Any => "any",
    }
}

fn matches_type(t: &TypeHint, v: &Value) -> bool {
    match (t, v) {
        (TypeHint::Any, _) => true,
        (TypeHint::Str, Value::String(_)) => true,
        (TypeHint::Int, Value::Number(n)) => n.is_i64() || n.is_u64(),
        (TypeHint::Float, Value::Number(_)) => true,
        (TypeHint::Bool, Value::Bool(_)) => true,
        (TypeHint::List, Value::Array(_)) => true,
        (TypeHint::Dict, Value::Object(_)) => true,
        (TypeHint::None_, Value::Null) => true,
        _ => false,
    }
}
```

**Step 2: Add more tests**

Append:

```rust
#[test]
fn flags_missing_required_field() {
    let s = sig(vec![("x", TypeHint::Str, true)]);
    let err = validate(&s, &json!({})).unwrap_err();
    assert_eq!(err.missing, vec!["x"]);
}

#[test]
fn flags_wrong_type() {
    let s = sig(vec![("x", TypeHint::Int, true)]);
    let err = validate(&s, &json!({ "x": "oops" })).unwrap_err();
    assert_eq!(err.wrong_type, vec!["x"]);
}

#[test]
fn flags_unexpected_keys() {
    let s = sig(vec![("x", TypeHint::Int, true)]);
    let err = validate(&s, &json!({ "x": 1, "extra": true })).unwrap_err();
    assert_eq!(err.unexpected, vec!["extra"]);
}

#[test]
fn optional_param_may_be_omitted() {
    let s = sig(vec![("x", TypeHint::Int, false)]);
    let kw = validate(&s, &json!({})).unwrap();
    assert!(kw.is_empty());
}

#[test]
fn any_type_accepts_anything() {
    let s = sig(vec![("x", TypeHint::Any, true)]);
    assert!(validate(&s, &json!({ "x": [1, 2] })).is_ok());
    assert!(validate(&s, &json!({ "x": null })).is_ok());
}
```

**Step 3: Run the tests**

Run: `cargo test --lib functions::validate`
Expected: all PASS.

**Step 4: Commit**

```bash
git add src/functions/validate.rs
git commit -m "feat(functions): implement JSON body validation against handler signature"
```

---

## Task 7: `Function` model CRUD

**Files:**
- Create: `src/models/function.rs`
- Modify: `src/models/mod.rs` — add `pub mod function; pub use function::Function;`

**Step 1: Write the module**

Copy the shape of `src/models/env.rs` and `src/models/api_key.rs`. Include: `create`, `find_for_user`, `find_by_slug_for_user`, `find_by_public_token`, `list_for_user`, `update_code`, `set_public`, `rotate_public_token`, `delete_for_user`, `touch`.

Key points:
- `id` is a UUID simple string.
- Public token is 32 url-safe-no-pad base64 bytes from `rand::thread_rng()`.
- Slug validation: regex `^[a-z0-9][a-z0-9_-]{0,62}$` (enforced in a free function, invoked at the route layer before insert).
- `set_public(pool, id, is_public)` mints a token when flipping from 0→1 and clears it when flipping 1→0.
- `rotate_public_token` generates a fresh token only if `is_public = 1`.

Implement `create` to reject invalid slugs and surface unique-constraint errors as a typed error (`anyhow` is fine; the route layer maps to 409).

**Step 2: Add a unit-ish test that exercises CRUD end-to-end via sqlx**

`src/models/function.rs` (at the bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::User;

    async fn pool() -> sqlx::SqlitePool {
        let pool = db::connect("sqlite::memory:").await.unwrap();
        db::migrate(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn create_and_fetch() {
        let pool = pool().await;
        let user = User::upsert_github(&pool, 42, "u", None).await.unwrap();
        let f = Function::create(
            &pool, user.id, "hello",
            "def handler(): return 1",
            "stateless", None, false, true,
        )
        .await
        .unwrap();
        let fetched = Function::find_by_slug_for_user(&pool, user.id, "hello")
            .await.unwrap().unwrap();
        assert_eq!(fetched.id, f.id);
        assert_eq!(fetched.code, "def handler(): return 1");
    }

    #[tokio::test]
    async fn publishing_mints_token_unpublishing_clears_it() {
        let pool = pool().await;
        let user = User::upsert_github(&pool, 1, "u", None).await.unwrap();
        let f = Function::create(&pool, user.id, "pub", "def handler(): ...", "stateless", None, false, true)
            .await.unwrap();
        Function::set_public(&pool, &f.id, true).await.unwrap();
        let f2 = Function::find_for_user(&pool, user.id, &f.id).await.unwrap().unwrap();
        assert!(f2.is_public);
        assert!(f2.public_token.is_some());
        let tok = f2.public_token.clone().unwrap();
        Function::set_public(&pool, &f.id, false).await.unwrap();
        let f3 = Function::find_for_user(&pool, user.id, &f.id).await.unwrap().unwrap();
        assert!(!f3.is_public);
        assert!(f3.public_token.is_none());
        // New token after re-publish.
        Function::set_public(&pool, &f.id, true).await.unwrap();
        let f4 = Function::find_for_user(&pool, user.id, &f.id).await.unwrap().unwrap();
        assert_ne!(f4.public_token.unwrap(), tok);
    }
}
```

**Step 3: Run the tests**

Run: `cargo test --lib models::function::tests`
Expected: all PASS.

**Step 4: Commit**

```bash
git add src/models/function.rs src/models/mod.rs
git commit -m "feat(functions): add Function model with CRUD and public-token mint/rotate"
```

---

## Task 8: `FunctionCall` model

**Files:**
- Create: `src/models/function_call.rs`
- Modify: `src/models/mod.rs` — add `pub mod function_call; pub use function_call::FunctionCall;`

**Step 1: Implement**

```rust
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

const TRUNCATE_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FunctionCall {
    pub id: i64,
    pub function_id: String,
    pub status: i64,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub error: Option<String>,
    pub duration_ms: i64,
    pub created_at: DateTime<Utc>,
}

impl FunctionCall {
    pub async fn insert(
        pool: &SqlitePool,
        function_id: &str,
        status: u16,
        request: Option<&str>,
        response: Option<&str>,
        error: Option<&str>,
        duration_ms: u64,
        truncate: bool,
    ) -> Result<Self> {
        let req = request.map(|s| maybe_truncate(s, truncate));
        let resp = response.map(|s| maybe_truncate(s, truncate));
        let row = sqlx::query_as::<_, FunctionCall>(
            r#"
            INSERT INTO function_calls
                (function_id, status, request_body, response_body, error, duration_ms)
            VALUES (?, ?, ?, ?, ?, ?)
            RETURNING id, function_id, status, request_body, response_body, error, duration_ms, created_at
            "#,
        )
        .bind(function_id)
        .bind(status as i64)
        .bind(req.as_deref())
        .bind(resp.as_deref())
        .bind(error)
        .bind(duration_ms as i64)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    pub async fn list_recent(pool: &SqlitePool, function_id: &str, limit: i64) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, FunctionCall>(
            r#"
            SELECT id, function_id, status, request_body, response_body, error, duration_ms, created_at
            FROM function_calls WHERE function_id = ?
            ORDER BY id DESC LIMIT ?
            "#,
        )
        .bind(function_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn prune(pool: &SqlitePool, function_id: &str, keep: i64) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM function_calls
            WHERE function_id = ?
              AND id NOT IN (
                SELECT id FROM function_calls
                WHERE function_id = ?
                ORDER BY id DESC LIMIT ?
              )
            "#,
        )
        .bind(function_id)
        .bind(function_id)
        .bind(keep)
        .execute(pool)
        .await?;
        Ok(())
    }
}

fn maybe_truncate(s: &str, truncate: bool) -> String {
    if !truncate || s.len() <= TRUNCATE_BYTES {
        s.to_string()
    } else {
        let mut end = TRUNCATE_BYTES;
        while !s.is_char_boundary(end) { end -= 1; }
        format!("{}...[truncated]", &s[..end])
    }
}
```

**Step 2: Add a short test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncates_when_flag_set() {
        let s = "a".repeat(10_000);
        let t = maybe_truncate(&s, true);
        assert!(t.ends_with("...[truncated]"));
        assert!(t.len() < s.len());
    }

    #[test]
    fn no_truncate_when_flag_clear() {
        let s = "a".repeat(10_000);
        assert_eq!(maybe_truncate(&s, false), s);
    }
}
```

**Step 3: Run**

Run: `cargo test --lib models::function_call`
Expected: PASS.

**Step 4: Commit**

```bash
git add src/models/function_call.rs src/models/mod.rs
git commit -m "feat(functions): add FunctionCall model with truncation + pruning"
```

---

## Task 9: Function executor — stateless path

**Files:**
- Create: `src/functions/executor.rs`
- Modify: `src/functions/mod.rs` — add `pub mod executor;`

**Step 1: Define the trait + outcome types**

```rust
use anyhow::{Context, Result};
use monty::{
    ExtFunctionResult, LimitedTracker, MontyObject, MontyRepl, PrintStream, PrintWriter,
    ReplProgress, ReplStartError,
};
use monty::fs::{MountMode, MountTable};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::time::Instant;

use crate::envs::executor::ExecLimits;
use crate::envs::locks::LockRegistry;
use crate::envs::storage::EnvStorage;
use crate::functions::signature::HandlerSignature;

#[derive(Debug, Serialize)]
pub struct CallOutcome {
    pub status: u16,
    pub response_json: Option<String>, // serialized JSON of handler's return value
    pub error: Option<String>,         // handler/engine error message
    pub duration_ms: u64,
}

pub struct FunctionExecutor {
    pub storage: EnvStorage,
    pub locks: LockRegistry,
    pub limits: ExecLimits,
}

impl FunctionExecutor {
    pub fn new(storage: EnvStorage, locks: LockRegistry) -> Self {
        Self { storage, locks, limits: ExecLimits::default() }
    }

    pub async fn call_stateless(
        &self,
        code: &str,
        _sig: &HandlerSignature,
        kwargs: Map<String, Value>,
    ) -> Result<CallOutcome> {
        let limits = self.limits.clone();
        let code = code.to_string();
        let started = Instant::now();
        let outcome = tokio::task::spawn_blocking(move || {
            let repl = MontyRepl::new("<fn>", limits.tracker());
            run_handler(repl, &code, &kwargs, &PathBuf::new(), &limits)
        })
        .await
        .context("spawn_blocking panicked")??;
        Ok(CallOutcome { duration_ms: started.elapsed().as_millis() as u64, ..outcome })
    }
}

fn run_handler(
    repl: MontyRepl<LimitedTracker>,
    code: &str,
    kwargs: &Map<String, Value>,
    files_dir: &PathBuf,
    limits: &ExecLimits,
) -> Result<CallOutcome> {
    // 1. Feed user code (defines handler + helpers).
    // 2. Inject __mk_kwargs as a JSON string constant, then run:
    //      import json
    //      __result = handler(**json.loads(__mk_kwargs_json))
    //      __result  # trailing expression so Monty returns it
    //    Return value is captured by MontyObject → format → serde_json.
    // Keep the implementation minimal — one feed_start call per stage is fine.
    //
    // The trampoline below uses a raw-string literal with the serialized kwargs
    // escaped for single-quote Python so we avoid quoting pitfalls; if any
    // escape issue appears, switch to base64 + base64.b64decode in Python.
    let kwargs_json = serde_json::to_string(kwargs).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(kwargs_json.as_bytes());
    let combined = format!(
        "{code}\n\
         import json, base64\n\
         __mk_kwargs = json.loads(base64.b64decode('{b64}').decode('utf-8'))\n\
         __result = handler(**__mk_kwargs)\n\
         __result\n"
    );
    let mut mount_table = MountTable::new();
    if !files_dir.as_os_str().is_empty() {
        mount_table
            .mount("/mnt", files_dir, MountMode::ReadWrite, limits.max_write_bytes)
            .context("mount files dir")?;
    }
    let mut streams: Vec<(PrintStream, String)> = Vec::new();
    let writer = PrintWriter::CollectStreams(&mut streams);
    let progress = match repl.feed_start(&combined, Vec::new(), writer) {
        Ok(p) => p,
        Err(err) => {
            let ReplStartError { repl: _, error } = *err;
            return Ok(CallOutcome {
                status: 500,
                response_json: None,
                error: Some(format!("{error}")),
                duration_ms: 0,
            });
        }
    };
    drive(progress, &mut mount_table)
}

fn drive(
    mut progress: ReplProgress<LimitedTracker>,
    mount_table: &mut MountTable,
) -> Result<CallOutcome> {
    loop {
        progress = match progress {
            ReplProgress::Complete { repl: _, value } => {
                let json = monty_object_to_json(&value)?;
                return Ok(CallOutcome {
                    status: 200,
                    response_json: Some(json),
                    error: None,
                    duration_ms: 0,
                });
            }
            ReplProgress::OsCall(call) => {
                let mut sink: Vec<(PrintStream, String)> = Vec::new();
                let writer = PrintWriter::CollectStreams(&mut sink);
                let result = match mount_table.handle_os_call(call.function, &call.args, &call.kwargs) {
                    Some(Ok(obj)) => ExtFunctionResult::Return(obj),
                    Some(Err(e)) => ExtFunctionResult::Error(e.into_exception()),
                    None => ExtFunctionResult::NotFound(format!("{:?}", call.function)),
                };
                match call.resume(result, writer) {
                    Ok(p) => p,
                    Err(err) => {
                        let ReplStartError { repl: _, error } = *err;
                        return Ok(CallOutcome {
                            status: 500,
                            response_json: None,
                            error: Some(format!("{error}")),
                            duration_ms: 0,
                        });
                    }
                }
            }
            ReplProgress::FunctionCall(call) => {
                let mut sink: Vec<(PrintStream, String)> = Vec::new();
                let writer = PrintWriter::CollectStreams(&mut sink);
                let name = call.function_name.clone();
                match call.resume(ExtFunctionResult::NotFound(name), writer) {
                    Ok(p) => p,
                    Err(err) => {
                        let ReplStartError { repl: _, error } = *err;
                        return Ok(CallOutcome {
                            status: 500,
                            response_json: None,
                            error: Some(format!("{error}")),
                            duration_ms: 0,
                        });
                    }
                }
            }
            ReplProgress::NameLookup(_) | ReplProgress::ResolveFutures(_) => {
                return Ok(CallOutcome {
                    status: 500,
                    response_json: None,
                    error: Some("unsupported progress state".into()),
                    duration_ms: 0,
                });
            }
        };
    }
}

fn monty_object_to_json(_obj: &MontyObject) -> Result<String> {
    // Placeholder: serialize via `format!("{obj:?}")` for v1 and let the caller
    // ship that as the response body. A proper `MontyObject → serde_json::Value`
    // conversion is a follow-up; the Debug output is enough to unblock the UX,
    // and we document this limitation in the detail page.
    Ok(format!("{_obj:?}"))
}
```

NOTE: the `monty_object_to_json` placeholder. This is a known gap — see Task 9b below to check whether Monty exposes a JSON converter, and fall back to `Debug` only if not.

**Step 2: Build**

Run: `cargo build`
Expected: compiles. If `base64` isn't a direct dep, add it: `cargo add base64`.

**Step 3: Commit**

```bash
git add src/functions/executor.rs src/functions/mod.rs Cargo.toml Cargo.lock
git commit -m "feat(functions): stateless executor wiring MontyRepl + kwargs injection"
```

---

## Task 9b: Verify / fix the MontyObject → JSON conversion

**Files:**
- Possibly modify: `src/functions/executor.rs`

**Step 1: Check Monty for JSON support**

Run: `grep -rn "json\|to_json\|Json" monty/crates/monty/src --include="*.rs" | head -20`

If Monty exposes `to_json` / similar on `MontyObject`, replace `monty_object_to_json` accordingly. If not, the implementation stays on `Debug` output. Look at `monty/crates/monty/tests/json_serde.rs` for a hint — that test file implies some JSON facility exists.

**Step 2: If a real converter exists, swap it in and add a test**

If the converter takes a `&MontyObject` and returns `Result<String, _>`, use it. Add a test:

```rust
#[tokio::test]
async fn stateless_round_trips_json() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    let exec = FunctionExecutor::new(storage, LockRegistry::new());
    let code = "def handler(x: int) -> int:\n    return x * 2\n";
    let sig = crate::functions::signature::parse_handler(code).unwrap();
    let mut kw = serde_json::Map::new();
    kw.insert("x".into(), serde_json::json!(21));
    let out = exec.call_stateless(code, &sig, kw).await.unwrap();
    assert_eq!(out.status, 200);
    assert_eq!(out.response_json.unwrap(), "42");
}
```

**Step 3: Run**

Run: `cargo test --lib functions::executor`
Expected: PASS.

**Step 4: Commit**

```bash
git add src/functions/executor.rs
git commit -m "feat(functions): wire MontyObject → JSON for handler return values"
```

---

## Task 10: Function executor — bound-to-env path

**Files:**
- Modify: `src/functions/executor.rs`

**Step 1: Add `call_bound`**

```rust
impl FunctionExecutor {
    pub async fn call_bound(
        &self,
        env_id: &str,
        code: &str,
        _sig: &HandlerSignature,
        kwargs: Map<String, Value>,
    ) -> Result<CallOutcome> {
        let _guard = self.locks.acquire(env_id).await;
        let bytes = self.storage.read_snapshot(env_id).await.context("read snapshot")?;
        let files_dir = self.storage.files_dir(env_id)?;
        let limits = self.limits.clone();
        let code = code.to_string();
        let started = Instant::now();
        let outcome = tokio::task::spawn_blocking(move || {
            let repl = if bytes.is_empty() {
                MontyRepl::new("<fn-bound>", limits.tracker())
            } else {
                let mut loaded: MontyRepl<LimitedTracker> =
                    MontyRepl::load(&bytes).context("load snapshot")?;
                *loaded.tracker_mut() = limits.tracker();
                loaded
            };
            // Note: we do NOT dump the REPL after running — env is read-only template.
            run_handler(repl, &code, &kwargs, &files_dir, &limits)
        })
        .await
        .context("spawn_blocking panicked")??;
        Ok(CallOutcome { duration_ms: started.elapsed().as_millis() as u64, ..outcome })
    }
}
```

**Step 2: Add an integration-ish test**

Create: `tests/functions_executor.rs` (uses the `TestApp` harness so the env has a snapshot on disk):

```rust
mod common;
use common::TestApp;
use monty_server::envs::executor::EnvExecutor;
use monty_server::envs::storage::EnvStorage;
use monty_server::envs::locks::LockRegistry;
use monty_server::functions::executor::FunctionExecutor;
use monty_server::functions::signature::parse_handler;
use monty_server::models::{Env, User};
use serde_json::{json, Map};

#[tokio::test]
async fn bound_function_sees_env_state_but_does_not_mutate_it() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let env = Env::create(&app.pool, user.id, "e").await.unwrap();
    let storage = EnvStorage::new(app.tmp.path());
    storage.create(&env.id).await.unwrap();

    // Seed env heap with `value = 100`.
    let exec = monty_server::envs::executor::MontyExecutor::new(storage.clone(), LockRegistry::new());
    exec.execute(&env.id, "value = 100").await.unwrap();

    let fn_exec = FunctionExecutor::new(storage.clone(), LockRegistry::new());
    let code = "def handler() -> int:\n    global value\n    value = value + 1\n    return value\n";
    let sig = parse_handler(code).unwrap();

    let out1 = fn_exec.call_bound(&env.id, code, &sig, Map::new()).await.unwrap();
    let out2 = fn_exec.call_bound(&env.id, code, &sig, Map::new()).await.unwrap();
    // Both calls see value=100 → both return 101 (no mutation persists).
    assert_eq!(out1.response_json.as_deref(), Some("101"));
    assert_eq!(out2.response_json.as_deref(), Some("101"));

    // And the env REPL still sees value=100.
    let out3 = exec.execute(&env.id, "value").await.unwrap();
    assert!(out3.result.unwrap().contains("100"));
}
```

**Step 3: Run**

Run: `cargo test --test functions_executor`
Expected: PASS. If Monty's `Debug` serialization of an int isn't literally `"101"`, adjust the assert to match the format (`format!("{obj:?}")`).

**Step 4: Commit**

```bash
git add src/functions/executor.rs tests/functions_executor.rs
git commit -m "feat(functions): bound-to-env executor path (read-only env state)"
```

---

## Task 11: Wire `FunctionExecutor` into `AppState`

**Files:**
- Modify: `src/lib.rs`

**Step 1: Add field + construction**

Extend `AppState` with `pub fn_executor: Arc<FunctionExecutor>` and construct it in `build`, sharing the same `EnvStorage` and `LockRegistry` as the `MontyExecutor`. (Reusing the **same** `LockRegistry` is critical — bound function calls must contend for the same lock as REPL calls.)

```rust
let locks = LockRegistry::new();
let executor = Arc::new(MontyExecutor::new(env_storage.clone(), locks.clone()));
let fn_executor = Arc::new(FunctionExecutor::new(env_storage.clone(), locks));
```

Update `AppState` struct and constructor accordingly.

**Step 2: Build**

Run: `cargo build`
Expected: compiles.

**Step 3: Commit**

```bash
git add src/lib.rs
git commit -m "feat(functions): wire FunctionExecutor into AppState"
```

---

## Task 12: CRUD API routes under `/api/v1/functions`

**Files:**
- Modify: `src/routes/api.rs`

**Step 1: Add routes + handlers**

Add to the router in `api.rs`:

```rust
.route("/api/v1/functions", get(list_functions).post(create_function))
.route("/api/v1/functions/:id", get(get_function).patch(update_function).delete(delete_function))
.route("/api/v1/functions/:id/rotate", post(rotate_public_token))
```

Handlers:
- `list_functions` — `Function::list_for_user`.
- `create_function` — body `{ slug, code, mode, env_id?, is_public?, truncate_logs? }`. Validate slug. If `mode == "bound"`, require `env_id` and verify `Env::find_for_user` returns Some. Parse the code with `parse_handler` — reject with 400 on `NoHandler` / `Malformed`. Insert. If `is_public` is true in the body, call `Function::set_public(..., true)` after insert.
- `get_function` — scoped to the authed user.
- `update_function` — partial update; if `code` changes, re-parse and reject on parse error; if `is_public` toggled, mint/clear token via `set_public`.
- `delete_function` — `Function::delete_for_user`.
- `rotate_public_token` — 409 if not public.

**Step 2: Integration test** — `tests/function_crud.rs`

Cover: create, list, fetch, patch code, toggle public (returns a token), delete. Slug collision → 409. Bound mode without env_id → 400. Invalid code → 400.

Follow the `tests/env_ui.rs` pattern for issuing requests with a bearer token (use `app.create_api_key(user.id)` if the harness has it — otherwise create an `ApiKey` row directly and mint the plaintext in the test).

**Step 3: Run**

Run: `cargo test --test function_crud`
Expected: PASS.

**Step 4: Commit**

```bash
git add src/routes/api.rs tests/function_crud.rs
git commit -m "feat(functions): CRUD API routes under /api/v1/functions"
```

---

## Task 13: Call routes `/fn/:slug` and `/fn/p/:public_token`

**Files:**
- Create: `src/routes/functions.rs`
- Modify: `src/routes/mod.rs` — `pub mod functions;`
- Modify: `src/lib.rs` — `.merge(routes::functions::router())`

**Step 1: Implement the router**

```rust
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/fn/:slug", post(call_authed))
        .route("/fn/p/:public_token", post(call_public))
}

async fn call_authed(
    State(state): State<AppState>,
    BearerUser(user): BearerUser,
    Path(slug): Path<String>,
    body: Bytes,
) -> Response {
    let function = match Function::find_by_slug_for_user(&state.pool, user.id, &slug).await {
        Ok(Some(f)) => f,
        Ok(None) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => return internal(e),
    };
    dispatch(state, function, body).await
}

async fn call_public(
    State(state): State<AppState>,
    Path(token): Path<String>,
    body: Bytes,
) -> Response {
    let function = match Function::find_by_public_token(&state.pool, &token).await {
        Ok(Some(f)) if f.is_public => f,
        Ok(_) => return (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => return internal(e),
    };
    dispatch(state, function, body).await
}
```

`dispatch` does:
1. Parse `body` as JSON; on malformed → log (with `FunctionCall::insert`), return 400 `{"error":"invalid json"}`.
2. `parse_handler(&function.code)` — any error here shouldn't happen (rejected at save), but on failure log + 500.
3. `validate(&sig, &body_json)` — on `Err(schema)`, log + return 400 with the `SchemaError` as the body.
4. Call `fn_executor.call_stateless` or `.call_bound` depending on `function.mode`.
5. On `CallOutcome { status: 200, response_json: Some(j), .. }` → return 200 with `j` as `application/json`.
6. On error outcome → map to 500/504/507 as appropriate; log to `function_calls`.
7. `FunctionCall::prune(pool, id, 500)` after every insert (cheap; SQLite handles this fine for small N).

**Step 2: Integration tests** — `tests/function_calls.rs`

Cover:
- Stateless function with matching body → 200 + return value JSON.
- Missing required field → 400 with `missing` populated.
- Wrong type → 400 with `wrong_type`.
- Unexpected key → 400 with `unexpected`.
- Malformed JSON body → 400 with `invalid json`.
- Private function without bearer → 401.
- After `Function::set_public(true)`, the minted token → 200.
- After `Function::set_public(false)`, old token → 404.
- Bound function reads seeded env heap, repeated call shows no mutation.

**Step 3: Run**

Run: `cargo test --test function_calls`
Expected: PASS.

**Step 4: Commit**

```bash
git add src/routes/functions.rs src/routes/mod.rs src/lib.rs tests/function_calls.rs
git commit -m "feat(functions): POST /fn/:slug and /fn/p/:token with validation + logging"
```

---

## Task 14: Dashboard Functions section (UI)

**Files:**
- Create: `templates/_functions_list.html`
- Modify: `templates/dashboard.html`
- Modify: `src/routes/web.rs`

**Step 1: Extend `dashboard` handler to load functions**

Add `functions: Vec<Function>` to `DashboardTemplate`. Fetch via `Function::list_for_user`.

**Step 2: Write the list fragment**

`templates/_functions_list.html`: for each function, show name/slug, mode badge, public indicator (with the URL copyable if public), created_at. Include a "New function" form that POSTs to `/functions` (web route, see step 3) with fields: slug, mode (radio), env picker (select populated from the user's envs), code textarea, `is_public` checkbox.

**Step 3: Web routes for create/delete/update**

In `src/routes/web.rs` add:

```rust
.route("/functions", post(create_function_ui))
.route("/functions/:id/delete", post(delete_function_ui))
.route("/functions/:id", get(function_detail).post(update_function_ui))
.route("/functions/:id/rotate", post(rotate_public_token_ui))
```

`create_function_ui` accepts a form, performs the same validation as the API route, and redirects back to `/dashboard` on success (or re-renders with an error on failure).

**Step 4: Integration test** — `tests/function_ui.rs`

- Logged-in user sees the Functions section on `/dashboard`.
- Creating a function via the form redirects to dashboard and shows the new row.
- Bound-mode form with no env → error shown inline.

Run: `cargo test --test function_ui`
Expected: PASS.

**Step 5: Commit**

```bash
git add templates/dashboard.html templates/_functions_list.html src/routes/web.rs tests/function_ui.rs
git commit -m "feat(functions): dashboard Functions section + create form"
```

---

## Task 15: Function detail page

**Files:**
- Create: `templates/function_detail.html`
- Create: `templates/_function_calls.html`
- Modify: `src/routes/web.rs` — implement `function_detail`

**Step 1: Template**

`function_detail.html` mirrors `env_detail.html`:
- Header with the function slug, mode badge, and the appropriate call URL (authed `POST /fn/<slug>` or public `POST /fn/p/<token>`). Copy button.
- Left panel: `<textarea name="code">` pre-populated with `function.code`. Save button (POSTs to `/functions/:id` with the form-encoded body). "Rotate public URL" button (POST to `/functions/:id/rotate`). "Delete".
- Right panel: recent calls, rendered via `_function_calls.html` — show status chip, duration, `request_body` / `response_body` / `error`. Each call collapsible.

**Step 2: Handler**

```rust
async fn function_detail(
    State(state): State<AppState>,
    ApprovedSessionUser(user): ApprovedSessionUser,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let Some(function) = Function::find_for_user(&state.pool, user.id, &id).await? else {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    };
    let calls = FunctionCall::list_recent(&state.pool, &function.id, 100).await?;
    let tmpl = FunctionDetailTemplate { user, function, calls };
    Ok(tmpl.into_response())
}
```

**Step 3: Test** — add to `tests/function_ui.rs`

- Detail page shows the code, the call URL, and the last call's status.
- Public function detail shows the `/fn/p/<token>` URL; after rotating, URL differs.

Run: `cargo test --test function_ui`
Expected: PASS.

**Step 4: Commit**

```bash
git add templates/function_detail.html templates/_function_calls.html src/routes/web.rs tests/function_ui.rs
git commit -m "feat(functions): function detail page with code editor + call log"
```

---

## Task 16: E2E public webhook smoke test

**Files:**
- Create: `tests/function_public_e2e.rs`

**Step 1: Write an end-to-end test**

A user creates a function via the web form, toggles it public, captures the generated URL from the detail page, and hits it with a raw HTTP body (no auth) — verifies 200 + correct return value + a `function_calls` row was written.

**Step 2: Run**

Run: `cargo test --test function_public_e2e`
Expected: PASS.

**Step 3: Commit**

```bash
git add tests/function_public_e2e.rs
git commit -m "test(functions): end-to-end public webhook flow"
```

---

## Task 17: README + API docs

**Files:**
- Modify: `README.md`

**Step 1: Add a Functions row to the API table and a short "Functions" section**

Append to the API table:

| Route | Description |
|-------|-------------|
| `POST /api/v1/functions` | Create a function. Body: `{slug, code, mode, env_id?, is_public?, truncate_logs?}` |
| `GET /api/v1/functions` | List your functions |
| `PATCH /api/v1/functions/:id` | Update a function |
| `POST /api/v1/functions/:id/rotate` | Rotate the public URL |
| `DELETE /api/v1/functions/:id` | Delete a function |
| `POST /fn/:slug` | Call your function (bearer) |
| `POST /fn/p/:token` | Call a public function (no auth) |

Add a short "Functions" section explaining: `def handler(...)` with typed params, JSON body → kwargs, 400 on shape mismatch, stateless vs bound-to-env.

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: README — Functions API and concept"
```

---

## Final verification

Run the full test suite and make sure everything passes:

```bash
cargo test
```

Expected: all tests pass. If any test is flaky on the `MontyObject → JSON` path, loop back to Task 9b.

---

## Notes for the executing engineer

- **Don't start the server yourself** to hand-test. `cargo watch` is running; rely on `cargo test` and the live rebuild.
- **Reuse the existing `LockRegistry`** across `MontyExecutor` and `FunctionExecutor`. Bound function calls and REPL calls must contend for the same env lock.
- **Don't dump the REPL after a bound function call.** The env is a read-only template; this is load-bearing behavior covered by a test in Task 10.
- **Monty's return-value formatting.** If `MontyObject`'s `Debug` doesn't give clean JSON, Task 9b is the time to fix it. Don't push through on ugly output — loop back.
- **Slug validation** is a pure function; test it with 5-6 cases (happy path, too long, leading digit, hyphen/underscore, uppercase, empty).
- **`is_public = true` on create** must mint a token even though the initial insert stores `public_token = NULL`. Either insert + `set_public(true)`, or compute the token and insert with it in one go. Whichever is cleaner.
- **Pruning on every insert** is fine for v1. If it shows up in profiles, switch to a periodic sweep.
