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
