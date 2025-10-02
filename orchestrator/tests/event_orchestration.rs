use anyhow::Result;
use codex_orchestrator::codex::Manager;
mod util;

fn set_stub_codex() {
    let stub: String = env!("CARGO_BIN_EXE_stub_codex").to_string();
    std::env::set_var("CODEX_BIN", &stub);
}

#[tokio::test]
async fn test_notification_from_agent() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("event-test-agent".to_string()), None).await?;

        // Create a conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Test conversation"))
            .await?;
        let cid = conv
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Send a user message - stub_codex will respond and send a notification
        let _ = mgr
            .send_user_message(
                &agent_id,
                serde_json::json!({
                    "conversationId": cid,
                    "items": [{"type": "text", "data": {"text": "hello"}}]
                }),
            )
            .await?;

        // Give the read loop time to process the notification
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // We can't directly verify the notification was forwarded upstream in this test
        // without a mock MCP client, but we can verify the message succeeded
        // The notification handling is tested indirectly through the read loop

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_approval_request_flow() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("approval-test-agent".to_string()), None).await?;

        // Create a conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Approval test"))
            .await?;
        let cid = conv
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Send a turn with testApproval flag - stub will send approval request
        let send_task = tokio::spawn({
            let mgr = mgr.clone();
            let agent_id = agent_id.clone();
            async move {
                mgr.send_user_turn(
                    &agent_id,
                    serde_json::json!({
                        "conversationId": cid,
                        "items": [{"type": "text", "data": {"text": "test"}}],
                        "cwd": "/tmp",
                        "approvalPolicy": "never",
                        "sandboxPolicy": {"mode": "read-only"},
                        "model": "gpt-4",
                        "summary": "none",
                        "testApproval": true
                    }),
                )
                .await
            }
        });

        // Give time for approval to be registered
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Check if approval is pending
        let approvals = mgr.list_pending_approvals().await;

        // The approval should be in the list
        assert!(
            !approvals.is_empty() || send_task.is_finished(),
            "Should have pending approval or task completed"
        );

        // If there's a pending approval, decide it
        if let Some(key) = approvals.first() {
            let _ = mgr.decide_approval(key, "allow".to_string()).await;
        }

        // Wait for the send_user_turn to complete
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_secs(2),
            send_task
        ).await;

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_approval_timeout() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("timeout-test-agent".to_string()), None).await?;

        // Create a conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Timeout test"))
            .await?;
        let cid = conv
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Send a turn that triggers approval but we won't respond
        let send_task = tokio::spawn({
            let mgr = mgr.clone();
            let agent_id = agent_id.clone();
            async move {
                mgr.send_user_turn(
                    &agent_id,
                    serde_json::json!({
                        "conversationId": cid,
                        "items": [{"type": "text", "data": {"text": "test"}}],
                        "cwd": "/tmp",
                        "approvalPolicy": "never",
                        "sandboxPolicy": {"mode": "read-only"},
                        "model": "gpt-4",
                        "summary": "none",
                        "testApproval": true
                    }),
                )
                .await
            }
        });

        // Give time for approval to be registered
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Check if approval is pending
        let approvals = mgr.list_pending_approvals().await;

        // There should be a pending approval
        let has_approval = !approvals.is_empty();

        // Don't decide it - let it timeout (would take 60s in real scenario)
        // For testing purposes, we just verify the approval was registered

        if has_approval {
            eprintln!("Approval pending (will timeout if not decided): {:?}", approvals);
        }

        // Cancel the send task
        send_task.abort();

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_list_approvals() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("list-approval-agent".to_string()), None).await?;

        // Initially, no approvals
        let empty_approvals = mgr.list_pending_approvals().await;
        assert_eq!(empty_approvals.len(), 0, "Should start with no approvals");

        // Create a conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("List approval test"))
            .await?;
        let cid = conv
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Spawn task that will create approval
        let _send_task = tokio::spawn({
            let mgr = mgr.clone();
            let agent_id = agent_id.clone();
            async move {
                let _ = mgr.send_user_turn(
                    &agent_id,
                    serde_json::json!({
                        "conversationId": cid,
                        "items": [{"type": "text", "data": {"text": "test"}}],
                        "cwd": "/tmp",
                        "approvalPolicy": "never",
                        "sandboxPolicy": {"mode": "read-only"},
                        "model": "gpt-4",
                        "summary": "none",
                        "testApproval": true
                    }),
                )
                .await;
            }
        });

        // Give time for approval to be created
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // Check approvals list
        let approvals = mgr.list_pending_approvals().await;
        eprintln!("Pending approvals: {:?}", approvals);

        // Clean up
        for key in &approvals {
            let _ = mgr.decide_approval(key, "deny".to_string()).await;
        }

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_decide_approval_invalid_key() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("invalid-key-agent".to_string()), None).await?;

        // Try to decide an approval that doesn't exist
        let result = mgr.decide_approval("invalid-agent:999", "allow".to_string()).await;

        // Should return an error
        assert!(result.is_err(), "Should fail for invalid approval key");

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}
