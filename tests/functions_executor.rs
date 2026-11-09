mod common;
use common::TestApp;
use monty_server::envs::executor::EnvExecutor;
use monty_server::envs::locks::LockRegistry;
use monty_server::envs::storage::EnvStorage;
use monty_server::functions::executor::FunctionExecutor;
use monty_server::functions::signature::parse_handler;
use monty_server::models::{Env, User};
use serde_json::Map;

#[tokio::test]
async fn bound_function_sees_env_state_but_does_not_mutate_it() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let env = Env::create(&app.pool, user.id, "e").await.unwrap();
    let storage = EnvStorage::new(app.tmp.path());
    storage.create(&env.id).await.unwrap();

    // Seed env heap with `value = 100`.
    let sandbox = common::sandbox().await;
    let locks = LockRegistry::new();
    let exec = monty_server::envs::executor::MontyExecutor::new(storage.clone(), locks.clone(), sandbox.clone());
    exec.execute(&env.id, "value = 100").await.unwrap();

    let fn_exec = FunctionExecutor::new(storage.clone(), locks, sandbox);
    let code = "def handler() -> int:\n    global value\n    value = value + 1\n    return value\n";
    let sig = parse_handler(code).unwrap();

    let out1 = fn_exec.call_bound(&env.id, code, &sig, Map::new()).await.unwrap();
    let out2 = fn_exec.call_bound(&env.id, code, &sig, Map::new()).await.unwrap();
    // Both calls see value=100 → both return 101 (no mutation persists).
    assert_eq!(out1.response_json.as_deref(), Some("101"));
    assert_eq!(out2.response_json.as_deref(), Some("101"));

    // And the env REPL still sees value=100.
    let out3 = exec.execute(&env.id, "value").await.unwrap();
    assert_eq!(out3.result.as_deref(), Some("100"));
}

#[tokio::test]
async fn bound_function_cannot_write_env_files() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    storage.create("fnenv0001").await.unwrap();
    storage.write_file("fnenv0001", "in.txt", b"hello").await.unwrap();
    let fn_exec = FunctionExecutor::new(storage.clone(), LockRegistry::new(), common::sandbox().await);
    let code = "from pathlib import Path\ndef handler() -> str:\n    return Path('/mnt/in.txt').read_text()\n";
    let out = fn_exec.call_bound("fnenv0001", code, &parse_handler(code).unwrap(), Map::new()).await.unwrap();
    assert_eq!(out.response_json.as_deref(), Some("\"hello\""), "{:?}", out.error);

    let code = "from pathlib import Path\ndef handler() -> int:\n    return Path('/mnt/out.txt').write_text('x')\n";
    let out = fn_exec.call_bound("fnenv0001", code, &parse_handler(code).unwrap(), Map::new()).await.unwrap();
    assert_eq!(out.status, 500);
    assert!(!storage.files_dir("fnenv0001").unwrap().join("out.txt").exists());
}

async fn stateless(code: &str, kw: serde_json::Value) -> monty_server::functions::executor::CallOutcome {
    let tmp = tempfile::tempdir().unwrap();
    let exec = FunctionExecutor::new(EnvStorage::new(tmp.path()), LockRegistry::new(), common::sandbox().await);
    let kw = kw.as_object().unwrap().clone();
    exec.call_stateless(code, &parse_handler(code).unwrap(), kw).await.unwrap()
}

#[tokio::test]
async fn stateless_round_trips_json() {
    let out = stateless("def handler(x: int) -> int:\n    return x * 2\n", serde_json::json!({"x": 21})).await;
    assert_eq!(out.status, 200, "error: {:?}", out.error);
    assert_eq!(out.response_json.unwrap(), "42");
}

#[tokio::test]
async fn stateless_json_string_is_quoted() {
    let out = stateless("def handler(name: str) -> str:\n    return 'hi ' + name\n", serde_json::json!({"name": "drew"})).await;
    assert_eq!(out.status, 200);
    assert_eq!(out.response_json.unwrap(), "\"hi drew\"");
}

#[tokio::test]
async fn stateless_json_list_is_array() {
    let out = stateless("def handler() -> list:\n    return [1, 2, 3]\n", serde_json::json!({})).await;
    assert_eq!(out.status, 200);
    assert_eq!(out.response_json.unwrap(), "[1,2,3]");
}

#[tokio::test]
async fn stateless_kwargs_with_quotes_and_unicode_survive() {
    let tricky = "it's a \"test\" \\ with ünïcode\n and newline ''' \"\"\"";
    let out = stateless("def handler(s: str, d: dict) -> dict:\n    return {'s': s, 'd': d}\n",
        serde_json::json!({"s": tricky, "d": {"a": [1, null, true, 1.5]}})).await;
    assert_eq!(out.status, 200, "{:?}", out.error);
    let v: serde_json::Value = serde_json::from_str(&out.response_json.unwrap()).unwrap();
    assert_eq!(v["s"], tricky);
    assert_eq!(v["d"], serde_json::json!({"a": [1, null, true, 1.5]}));
}

#[tokio::test]
async fn stateless_non_json_return_is_500() {
    let out = stateless("def handler():\n    return b'bytes'\n", serde_json::json!({})).await;
    assert_eq!(out.status, 500);
    assert!(out.error.unwrap().contains("TypeError"));
    let out = stateless("def handler():\n    return float('nan')\n", serde_json::json!({})).await;
    assert_eq!(out.status, 500);
}

#[tokio::test]
async fn stateless_handler_exception_is_500() {
    let out = stateless("def handler():\n    raise KeyError('nope')\n", serde_json::json!({})).await;
    assert_eq!(out.status, 500);
    assert!(out.error.unwrap().contains("KeyError"));
}
