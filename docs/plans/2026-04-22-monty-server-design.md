# Monty Server — Design

Date: 2026-04-22
Status: Design approved, ready for implementation planning

## Goal

A self-hostable web server that exposes the [Monty](../../monty) Python interpreter as a multi-tenant service with persistent, stateful execution environments. Users sign in via GitHub, manage API keys, and programmatically create/use Monty envs that survive across requests.

## Core behaviors

- GitHub OAuth sign-in, browser sessions
- API key management (create, list, delete) from a minimal web UI
- Programmatic API (bearer-token auth): create, list, delete stateful envs; execute code in an env; one-off stateless execution
- Per-env working directory, read/write from inside the sandbox, uploadable from the UI
- Web UI per env: file browser, file upload, live REPL, "current state" view (variables)

## Architecture summary

Single Rust binary. Axum + SQLite + local disk. Monty linked as a Rust crate — no FFI. Server-rendered HTML with HTMX for interactivity. One static container, deployed to Fly (or equivalent) with a single persistent volume.

Env state is persisted as **serialized Monty snapshot bytes** (Monty's `MontyRun::dump()` / `load()`). Each execution: load snapshot → mount env dir → run → dump → write back. This is the Snapshot/Rehydrate model — it makes request handlers stateless-ish, avoids long-lived Monty instances in memory, and is the use case Monty's snapshot API was built for.

## Stack

| Concern | Choice |
| --- | --- |
| Web framework | Axum + tower |
| Templates | Askama + HTMX |
| DB | SQLite via sqlx |
| Auth | `oauth2` crate (GitHub), `tower-sessions` for cookies, bearer tokens for API |
| Monty | `monty` crate linked directly |
| Deploy | Single static binary in a distroless container + one persistent volume |

## Data model (SQLite)

```
users(id, github_id, github_login, email, created_at)
api_keys(id, user_id, name, token_hash, prefix, created_at, last_used_at)
envs(id, user_id, name, snapshot_blob_path, created_at, updated_at)
sessions(id, user_id, expires_at, ...)   -- via tower-sessions
```

Notes:
- `api_keys.token_hash` stores a hash (argon2 or similar); `prefix` (first 8 chars) is stored plaintext so the UI can show "mk_live_abcd1234…" for identification.
- `envs.snapshot_blob_path` points to a file on disk; the snapshot bytes are *not* stored in SQLite (they can be large and we already have a volume).

## Filesystem layout (on the persistent volume)

```
/data/
  monty-server.db                       # SQLite
  envs/
    <env_uuid>/
      snapshot.bin                      # serialized Monty state
      files/                            # mounted into the sandbox (ReadWrite)
```

The sandbox sees only `/mnt/` → host's `envs/<env_uuid>/files/`, enforced by Monty's `MountTable`. No host path ever appears inside the sandbox.

## Env execution flow

```
load snapshot.bin from disk
   ↓
MontyRun::load(bytes)
   ↓
mount envs/<id>/files/ as /mnt/ (ReadWrite)
   ↓
run code with resource limits
   ↓
capture stdout / stderr / return value
   ↓
dump() → write new snapshot.bin atomically (write-temp + rename)
   ↓
return result
```

One-off execution is the same path minus the final dump/write. The REPL and the "current state" view both use this same primitive — state introspection is just a one-off execution that runs `{k: repr(v) for k, v in globals().items() if not k.startswith('_')}` against the env's snapshot. Monty is fast enough that doing this on page load is fine; no cached repr column needed.

## Concurrency

Per-env `tokio::Mutex`, stored in a `DashMap<EnvId, Arc<Mutex<()>>>`. Any operation that reads-then-writes the snapshot (code execution, file upload) acquires the lock. Read-only operations (file listing, file download) do not. Entries are reaped on env deletion.

Isolate the execution path behind a trait (`EnvExecutor`) so the implementation can later be swapped for an actor-per-env model if streaming stdout (SSE/WebSocket) becomes a requirement.

## API surface (programmatic, bearer-auth)

```
POST   /api/v1/envs                     # create env → { id, name }
GET    /api/v1/envs                     # list caller's envs
DELETE /api/v1/envs/:id                 # delete env + files
POST   /api/v1/envs/:id/execute         # { code } → { stdout, stderr, result, error? }
POST   /api/v1/execute                  # one-off { code } → same response shape
GET    /api/v1/envs/:id/files           # list files in env dir
POST   /api/v1/envs/:id/files           # upload (multipart)
GET    /api/v1/envs/:id/files/:path     # download
DELETE /api/v1/envs/:id/files/:path     # delete a file
```

## Web UI (session-auth, HTMX)

- `/` — landing + sign-in
- `/dashboard` — env list + API key management
- `/envs/:id` — file browser, file upload, state view, REPL (HTMX-driven, not a SPA)

## Auth

- Browser flow: GitHub OAuth (`oauth2` crate) → `tower-sessions` cookie → authorized for UI routes.
- API flow: `Authorization: Bearer <token>` → lookup by hash → authorized for `/api/v1/*`.
- Both resolve to the same `UserId`; API key actions and UI actions share the same underlying handlers.

## Deploy

Distroless container, Fly.io with one 10GB persistent volume mounted at `/data`. `fly.toml` pins a single machine (no horizontal scaling — the whole design assumes single-instance). Config via env vars: `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, `SESSION_SECRET`, `DATA_DIR=/data`.

## Non-goals

- Multi-instance / horizontal scaling (deferred — would require Postgres + S3)
- Streaming execution output (deferred — deliverable as an actor-mode swap behind the executor trait)
- Billing, rate limits beyond per-key sanity caps
- Public env sharing / collaboration

## Open questions for implementation

- Exact Monty resource limits per env (memory cap, wallclock cap, allocation cap) — probably configurable per env with sane defaults.
- Max upload size per file, max total bytes per env.
- Token format and hash algorithm (probably argon2id).
- Snapshot backup / export (nice-to-have, not MVP).
