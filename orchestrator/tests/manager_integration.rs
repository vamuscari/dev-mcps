use anyhow::Result;
use codex_orchestrator::codex::Manager;
mod util;
 

#[tokio::test]
async fn spawn_list_kill_agent_with_stub() -> Result<()> {
    // Resolve stub binary path in target dir
    // When running tests in debug profile, the bin should be at target/debug/stub_codex or similar.
    // Use CARGO_BIN_EXE environment if available.
    let stub: String = env!("CARGO_BIN_EXE_stub_codex").to_string();
    std::env::set_var("CODEX_BIN", &stub);

    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(None, None).await?;
        let list = mgr.list_agents().await;
        assert!(list.contains(&agent_id));
        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}
