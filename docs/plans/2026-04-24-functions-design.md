# Functions — Design

A **function** is a stored Python snippet whose `def handler(...)` is exposed as
an HTTP endpoint. It's the third first-class object in monty-server, after envs
and API keys, and reuses the Monty sandbox + per-env lock + snapshot machinery
already in place.

## Summary

- Every function belongs to a user and has a per-user-unique slug.
- Two execution modes: **stateless** (fresh interpreter per call) or **bound**
  (mounts an env's heap and `/mnt` files dir).
- Request body is JSON. The server introspects `handler`'s signature and binds
  keys to kwargs. Shape mismatches return `400` with a schema-describing body.
- Private by default, bearer-authenticated. Flipping "public" mints an
  unguessable URL segment; toggling off rotates/clears it.
- Calls are logged to a capped history (4 KB body truncation by default, flag
  to disable truncation per function).

## Data model

New table `functions`:

| column | type | notes |
|---|---|---|
| `id` | TEXT PRIMARY KEY | uuid |
| `user_id` | INTEGER NOT NULL | FK → users, ON DELETE CASCADE |
| `slug` | TEXT NOT NULL | URL-safe, per-user unique |
| `code` | TEXT NOT NULL | Python source; must define `def handler(...)` |
| `mode` | TEXT NOT NULL | `'stateless'` or `'bound'` |
| `env_id` | TEXT | FK → envs, required iff `mode = 'bound'` |
| `is_public` | INTEGER NOT NULL DEFAULT 0 | |
| `public_token` | TEXT | set when `is_public = 1`; rotatable |
| `truncate_logs` | INTEGER NOT NULL DEFAULT 1 | |
| `created_at` | TEXT NOT NULL DEFAULT (datetime('now')) | |
| `updated_at` | TEXT NOT NULL DEFAULT (datetime('now')) | |

Indexes: `UNIQUE (user_id, slug)`, `UNIQUE (public_token) WHERE public_token IS NOT NULL`,
`INDEX (user_id)`.

New table `function_calls`:

| column | type | notes |
|---|---|---|
| `id` | INTEGER PRIMARY KEY | |
| `function_id` | TEXT NOT NULL | FK → functions, ON DELETE CASCADE |
| `status` | INTEGER NOT NULL | HTTP status returned |
| `request_body` | TEXT | truncated to 4 KB unless the function has `truncate_logs = 0` |
| `response_body` | TEXT | same truncation policy |
| `error` | TEXT | handler exception or engine error, nullable |
| `duration_ms` | INTEGER NOT NULL | |
| `created_at` | TEXT NOT NULL DEFAULT (datetime('now')) | |

Index: `(function_id, id)`. Prune to last 500 rows per function — mechanism
(on-write vs periodic) decided in the plan.

## Routing & auth

- `POST /fn/:slug` — authed. Bearer token resolves the user; function looked
  up by `(user_id, slug)`.
- `POST /fn/p/:public_token` — no auth. Function looked up by `public_token`;
  `404` if null or if the function is currently private.
- `POST /api/v1/functions` — create. Body: `{ slug, code, mode, env_id?,
  is_public?, truncate_logs? }`.
- `GET /api/v1/functions` — list for the authed user.
- `GET /api/v1/functions/:id` — fetch one.
- `PATCH /api/v1/functions/:id` — update any mutable field (slug, code, mode,
  env_id, is_public, truncate_logs). Toggling `is_public` on mints a new token;
  toggling off clears it. A dedicated `POST /api/v1/functions/:id/rotate`
  endpoint issues a new token while staying public.
- `DELETE /api/v1/functions/:id` — deletes the row; cascades to
  `function_calls`.
- Web UI: `/functions/:id` (detail page), plus a Functions section on
  `/dashboard`.

## Request / response shape

Request: `POST` with `Content-Type: application/json`, body is a JSON object.
Keys bind to `handler` kwargs.

```
POST /fn/summarize
{ "text": "...", "max_words": 50 }
```

Success: `200 application/json`, body = JSON-encoded return value of `handler`
(including `null` for `None`).

Invalid input (missing keys, wrong types, unknown keys): `400` with

```json
{
  "error": "invalid input",
  "expected": { "text": "str", "max_words": "int" },
  "missing": ["max_words"],
  "unexpected": [],
  "wrong_type": []
}
```

Handler exception: `500`, `{ "error": "handler raised", "exception": "<formatted>" }`.

Resource limits:
- Timeout → `504`, `{ "error": "timeout" }`
- OOM / allocation limit → `507`, `{ "error": "resource limit" }`
- Malformed request JSON → `400`, `{ "error": "invalid json" }`

Private function called without / with wrong bearer → `401`. Public token
rotated/cleared → `404`.

## Execution mechanics

**Stateless.** On each call:
1. `MontyRepl::new("<fn>", ExecLimits::default().tracker())`.
2. Feed the stored `code` to define `handler` and any helpers/imports.
3. Introspect `handler`'s signature (see note below); validate the JSON body
   → `kwargs`. If it doesn't match, return 400 without calling.
4. Feed a trampoline expression that calls `handler(**kwargs)` with the bound
   arguments, using the same `feed_start` path the executor uses today.
5. JSON-encode the return value. Discard the interpreter.

**Bound.** Same as stateless, except:
1. Acquire the env's lock (`executor.locks().acquire(env_id)`). Shared with
   REPL and file ops, so function calls serialize against them.
2. Load the env's snapshot; mount its files dir at `/mnt`.
3. Steps 2–5 above run on the loaded REPL.
4. **Do not dump** — function calls do not mutate the env's persisted heap.
   The env is a read-only template. Documented in UI copy on the function
   detail page.

**Signature introspection.** One real implementation unknown: whether Monty's
interpreter exposes enough reflection on user-defined functions to read
parameter names, annotations, and defaults at runtime. Resolution deferred to
the plan:

- **Preferred:** use Monty's equivalent of `inspect.signature`, if available.
- **Fallback:** parse the `def handler(...)` line from the stored source and
  extract names/annotations textually. Type tokens stay as strings in the
  `expected` response either way.

The plan must start with a short spike that verifies which path is viable.

**Concurrency.** Stateless calls run in parallel; each spawns its own
`spawn_blocking` task with a fresh `MontyRepl`. Bound calls serialize on the
env lock, same as REPL calls — preserves the single-writer invariant.

**Resource limits.** All calls use `ExecLimits::default()` for v1. Per-function
overrides are out of scope.

## UI

- **Dashboard** (`/dashboard`) gets a Functions section between Envs and API
  Keys, mirroring `_env_list.html`: name/slug, mode, public indicator,
  last-called-at, call count. A "new function" form creates the row with slug,
  mode radio, env picker (when bound), public toggle, and a `<textarea>` for
  code.
- **`/functions/:id`** mirrors `env_detail.html`:
  - Left panel: code editor (`<textarea>`), Save, Delete, Rotate public URL.
  - Right panel: call log — last N rows, each expandable to show request /
    response / error / duration.
  - Header: the call URL (copyable) — authed form for private functions,
    public form when `is_public = 1`.
- Editor is a plain textarea for v1. Fancier editing later.

## Error handling (server-side validation)

- `slug` unique-per-user violation on create or rename → `409`.
- `mode = 'bound'` with missing/foreign `env_id` → `400`.
- Stored `code` missing a `def handler` → reject on save with `400`; on call,
  fall through to `500` only if somehow persisted.
- Malformed JSON body → `400 invalid json`.
- Public call hitting a rotated/cleared token → `404`.
- Bearer missing / invalid on a private function → `401`.

## Testing

Unit:
- Signature introspection from source (or via Monty reflection, whichever
  path wins).
- Schema-error rendering for all three shapes (missing, wrong_type,
  unexpected).
- Slug validation rules.
- Public-token minting and rotation.

Integration (Axum test harness + real SQLite, following `tests/env_ui.rs`):
- Create function, call with valid body → 200 + expected return value.
- Call with missing field → 400 with `missing` populated.
- Call with wrong type → 400 with `wrong_type` populated.
- Public function callable without bearer; private function without bearer →
  401.
- After rotating the public URL, the old token → 404, the new one → 200.
- Bound function reads `/mnt` files and env imports; calling twice and
  inspecting the env's snapshot afterward confirms no heap mutation.
- Handler exception → 500 with formatted exception.
- Timeout → 504.
- Call log entries are written with capped bodies; toggling
  `truncate_logs = 0` captures full bodies.

## Out of scope for v1

- GET / other HTTP methods.
- Query-string / header / path-param binding — JSON body only.
- Non-JSON responses (HTML, files, streaming).
- Versioning / rollback of function code.
- Per-function resource limits.
- Scheduled / cron invocation.
- One function calling another.
- Function-scoped secrets.
