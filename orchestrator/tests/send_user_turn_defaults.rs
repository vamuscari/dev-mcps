use anyhow::Result;
use codex_orchestrator::codex::Manager;
mod util;

fn set_stub_codex() {
    let stub: String = env!("CARGO_BIN_EXE_stub_codex").to_string();
    std::env::set_var("CODEX_BIN", &stub);
}

#[tokio::test]
async fn test_send_user_turn_with_string_params() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("turn-defaults-agent".to_string()), None).await?;

        // Create conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Test"))
            .await?;
        let cid = conv.get("conversationId").and_then(|v| v.as_str()).unwrap();

        // Simulate user's scenario: stringified JSON with conversationId and text
        let params_string = serde_json::json!(
            format!(r#"{{"conversationId":"{}","text":"This is a test message"}}"#, cid)
        );

        // This should work - orchestrator will:
        // 1. Parse the string to JSON
        // 2. Convert text to items
        // 3. Add default fields (cwd, approvalPolicy, sandboxPolicy, model, summary)
        let result = mgr.send_user_turn(&agent_id, params_string).await;

        assert!(result.is_ok(), "Should handle stringified params with text field: {:?}", result);

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_send_user_turn_minimal_object() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("minimal-agent".to_string()), None).await?;

        // Create conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Test"))
            .await?;
        let cid = conv.get("conversationId").and_then(|v| v.as_str()).unwrap();

        // Pass minimal object with just conversationId and text
        let result = mgr
            .send_user_turn(
                &agent_id,
                serde_json::json!({
                    "conversationId": cid,
                    "text": "Simple message"
                }),
            )
            .await;

        assert!(result.is_ok(), "Should handle minimal params: {:?}", result);

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_send_user_turn_only_text() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("text-only-agent".to_string()), None).await?;

        // Create conversation to set last_conversation_id
        let _conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Test"))
            .await?;

        // Pass just a string - should use last_conversation_id and fill in defaults
        let result = mgr
            .send_user_turn(&agent_id, serde_json::json!("Just a simple text message"))
            .await;

        assert!(result.is_ok(), "Should handle plain text with defaults: {:?}", result);

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_send_user_turn_with_overrides() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("override-agent".to_string()), None).await?;

        // Create conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Test"))
            .await?;
        let cid = conv.get("conversationId").and_then(|v| v.as_str()).unwrap();

        // Pass params with some defaults overridden
        let result = mgr
            .send_user_turn(
                &agent_id,
                serde_json::json!({
                    "conversationId": cid,
                    "text": "Custom message",
                    "model": "gpt-5",  // Override default
                    "approvalPolicy": "on-request",  // Override default
                }),
            )
            .await;

        assert!(result.is_ok(), "Should allow overriding defaults: {:?}", result);

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_send_user_turn_fully_specified() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("full-spec-agent".to_string()), None).await?;

        // Create conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Test"))
            .await?;
        let cid = conv.get("conversationId").and_then(|v| v.as_str()).unwrap();

        // Pass fully specified params (like the old way)
        let result = mgr
            .send_user_turn(
                &agent_id,
                serde_json::json!({
                    "conversationId": cid,
                    "items": [{"type": "text", "data": {"text": "Fully specified"}}],
                    "cwd": "/tmp",
                    "approvalPolicy": "never",
                    "sandboxPolicy": {"mode": "danger-full-access"},
                    "model": "gpt-4",
                    "summary": "concise"
                }),
            )
            .await;

        assert!(result.is_ok(), "Should work with fully specified params: {:?}", result);

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}
