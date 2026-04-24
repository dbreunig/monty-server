use anyhow::{Context, Result};
use monty::{
    ExtFunctionResult, JsonMontyObject, LimitedTracker, MontyObject, MontyRepl, PrintStream,
    PrintWriter, ReplProgress, ReplStartError,
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

fn run_handler(
    repl: MontyRepl<LimitedTracker>,
    code: &str,
    kwargs: &Map<String, Value>,
    files_dir: &PathBuf,
    limits: &ExecLimits,
) -> Result<CallOutcome> {
    let kwargs_json = serde_json::to_string(kwargs).unwrap();
    // Embed the JSON as a Python single-quoted string literal. `serde_json`'s
    // compact form has no literal newlines, so we only need to escape `\` and
    // `'` to make it a valid Python string that round-trips back through
    // `json.loads`. Monty does not ship a `base64` module, so we pass the JSON
    // text directly rather than base64-encoding it host-side.
    let py_literal = kwargs_json.replace('\\', "\\\\").replace('\'', "\\'");
    let combined = format!(
        "{code}\n\
         import json\n\
         __mk_kwargs = json.loads('{py_literal}')\n\
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

/// Serialize a handler's return value to the JSON form monty-server sends on the wire.
///
/// Uses Monty's [`JsonMontyObject`] wrapper, which maps JSON-native Python values
/// to their natural JSON (e.g. `Int(42)` → `42`, `String("hi")` → `"hi"`) and
/// tags non-JSON-native variants (tuple, bytes, exception, etc.) with a
/// `$`-prefixed key. This matches what a caller of the HTTP endpoint expects.
fn monty_object_to_json(obj: &MontyObject) -> Result<String> {
    serde_json::to_string(&JsonMontyObject(obj)).context("serialize handler return value")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envs::locks::LockRegistry;
    use crate::envs::storage::EnvStorage;
    use crate::functions::signature::parse_handler;

    #[tokio::test]
    async fn stateless_round_trips_json() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = EnvStorage::new(tmp.path());
        let exec = FunctionExecutor::new(storage, LockRegistry::new());
        let code = "def handler(x: int) -> int:\n    return x * 2\n";
        let sig = parse_handler(code).unwrap();
        let mut kw = serde_json::Map::new();
        kw.insert("x".into(), serde_json::json!(21));
        let out = exec.call_stateless(code, &sig, kw).await.unwrap();
        assert_eq!(out.status, 200, "error: {:?}", out.error);
        assert_eq!(out.response_json.unwrap(), "42");
    }

    #[tokio::test]
    async fn stateless_json_string_is_quoted() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = EnvStorage::new(tmp.path());
        let exec = FunctionExecutor::new(storage, LockRegistry::new());
        let code = "def handler(name: str) -> str:\n    return 'hi ' + name\n";
        let sig = parse_handler(code).unwrap();
        let mut kw = serde_json::Map::new();
        kw.insert("name".into(), serde_json::json!("drew"));
        let out = exec.call_stateless(code, &sig, kw).await.unwrap();
        assert_eq!(out.status, 200);
        assert_eq!(out.response_json.unwrap(), "\"hi drew\"");
    }

    #[tokio::test]
    async fn stateless_json_list_is_array() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = EnvStorage::new(tmp.path());
        let exec = FunctionExecutor::new(storage, LockRegistry::new());
        let code = "def handler() -> list:\n    return [1, 2, 3]\n";
        let sig = parse_handler(code).unwrap();
        let out = exec.call_stateless(code, &sig, serde_json::Map::new()).await.unwrap();
        assert_eq!(out.status, 200);
        assert_eq!(out.response_json.unwrap(), "[1,2,3]");
    }
}
