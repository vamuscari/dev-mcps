use anyhow::Result;
use codex_orchestrator::codex::Manager;
mod util;

fn set_stub_codex() {
    let stub: String = env!("CARGO_BIN_EXE_stub_codex").to_string();
    std::env::set_var("CODEX_BIN", &stub);
}

#[tokio::test]
async fn spawn_list_kill_agent_with_stub() -> Result<()> {
    set_stub_codex();
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

#[tokio::test]
async fn conversation_flow_send_message_and_turn() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("test-agent".to_string()), None).await?;

        // Start conversation with a simple string param
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("New session for tests"))
            .await?;
        let cid = conv
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Send a user message as a string (should be coerced into text items)
        let _ = mgr
            .send_user_message(&agent_id, serde_json::json!("Please do X"))
            .await?;

        // Send a user turn as a string
        let _ = mgr
            .send_user_turn(&agent_id, serde_json::json!("Proceed with Y"))
            .await?;

        // Interrupt the conversation; ensure response has an abort reason
        let resp = mgr
            .interrupt(&agent_id, serde_json::json!({"conversationId": cid}))
            .await?;
        assert!(resp.get("abortReason").is_some());

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}
