use anyhow::Result;
use codex_orchestrator::codex::Manager;
mod util;

fn set_stub_codex() {
    let stub: String = env!("CARGO_BIN_EXE_stub_codex").to_string();
    std::env::set_var("CODEX_BIN", &stub);
}

#[tokio::test]
async fn test_multiple_conversations_per_agent() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("multi-conv-agent".to_string()), None).await?;

        // Create first conversation
        let conv1 = mgr
            .new_conversation(&agent_id, serde_json::json!("First conversation"))
            .await?;
        let cid1 = conv1
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Create second conversation on same agent
        let conv2 = mgr
            .new_conversation(&agent_id, serde_json::json!("Second conversation"))
            .await?;
        let cid2 = conv2
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Create third conversation on same agent
        let conv3 = mgr
            .new_conversation(&agent_id, serde_json::json!("Third conversation"))
            .await?;
        let cid3 = conv3
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Verify all three IDs are different
        assert_ne!(cid1, cid2, "Conversation IDs should be unique");
        assert_ne!(cid2, cid3, "Conversation IDs should be unique");
        assert_ne!(cid1, cid3, "Conversation IDs should be unique");

        // Send messages to different conversations
        let _ = mgr
            .send_user_message(
                &agent_id,
                serde_json::json!({
                    "conversationId": cid1,
                    "items": [{"type": "text", "data": {"text": "Message to conv1"}}]
                }),
            )
            .await?;

        let _ = mgr
            .send_user_message(
                &agent_id,
                serde_json::json!({
                    "conversationId": cid2,
                    "items": [{"type": "text", "data": {"text": "Message to conv2"}}]
                }),
            )
            .await?;

        let _ = mgr
            .send_user_message(
                &agent_id,
                serde_json::json!({
                    "conversationId": cid3,
                    "items": [{"type": "text", "data": {"text": "Message to conv3"}}]
                }),
            )
            .await?;

        // All three conversations should be listed
        let list = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;
        let items = list
            .get("items")
            .and_then(|v| v.as_array())
            .unwrap();

        assert_eq!(items.len(), 3, "Should have 3 active conversations");

        // Verify all conversation IDs are present
        let ids: Vec<String> = items
            .iter()
            .filter_map(|item| {
                item.get("conversationId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        assert!(ids.contains(&cid1), "Should contain first conversation");
        assert!(ids.contains(&cid2), "Should contain second conversation");
        assert!(ids.contains(&cid3), "Should contain third conversation");

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_interleaved_conversation_operations() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("interleave-agent".to_string()), None).await?;

        // Create two conversations
        let conv1 = mgr
            .new_conversation(&agent_id, serde_json::json!("Conversation A"))
            .await?;
        let cid1 = conv1.get("conversationId").and_then(|v| v.as_str()).unwrap();

        let conv2 = mgr
            .new_conversation(&agent_id, serde_json::json!("Conversation B"))
            .await?;
        let cid2 = conv2.get("conversationId").and_then(|v| v.as_str()).unwrap();

        // Interleave operations on both conversations
        let _ = mgr
            .send_user_message(
                &agent_id,
                serde_json::json!({"conversationId": cid1, "items": [{"type": "text", "data": {"text": "A1"}}]}),
            )
            .await?;

        let _ = mgr
            .send_user_message(
                &agent_id,
                serde_json::json!({"conversationId": cid2, "items": [{"type": "text", "data": {"text": "B1"}}]}),
            )
            .await?;

        let _ = mgr
            .send_user_message(
                &agent_id,
                serde_json::json!({"conversationId": cid1, "items": [{"type": "text", "data": {"text": "A2"}}]}),
            )
            .await?;

        let _ = mgr
            .send_user_message(
                &agent_id,
                serde_json::json!({"conversationId": cid2, "items": [{"type": "text", "data": {"text": "B2"}}]}),
            )
            .await?;

        // Both should still be active
        let list = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;
        let items = list.get("items").and_then(|v| v.as_array()).unwrap();

        assert_eq!(items.len(), 2, "Both conversations should still be active");

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_archive_one_keep_others() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("archive-selective-agent".to_string()), None).await?;

        // Create three conversations
        let conv1 = mgr
            .new_conversation(&agent_id, serde_json::json!("Keep 1"))
            .await?;
        let cid1 = conv1.get("conversationId").and_then(|v| v.as_str()).unwrap().to_string();

        let conv2 = mgr
            .new_conversation(&agent_id, serde_json::json!("Archive this"))
            .await?;
        let cid2 = conv2.get("conversationId").and_then(|v| v.as_str()).unwrap().to_string();

        let conv3 = mgr
            .new_conversation(&agent_id, serde_json::json!("Keep 2"))
            .await?;
        let cid3 = conv3.get("conversationId").and_then(|v| v.as_str()).unwrap().to_string();

        // Archive only the middle one
        mgr.archive_conversation(&agent_id, serde_json::json!({"conversationId": cid2}))
            .await?;

        // List should show only 2 conversations
        let list = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;
        let items = list.get("items").and_then(|v| v.as_array()).unwrap();

        assert_eq!(items.len(), 2, "Should have 2 conversations after archiving one");

        // Check that the right ones remain
        let ids: Vec<String> = items
            .iter()
            .filter_map(|item| {
                item.get("conversationId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        assert!(ids.contains(&cid1), "First conversation should remain");
        assert!(!ids.contains(&cid2), "Second conversation should be archived");
        assert!(ids.contains(&cid3), "Third conversation should remain");

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_concurrent_message_sends() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("concurrent-agent".to_string()), None).await?;

        // Create two conversations
        let conv1 = mgr
            .new_conversation(&agent_id, serde_json::json!("Concurrent A"))
            .await?;
        let cid1 = conv1.get("conversationId").and_then(|v| v.as_str()).unwrap().to_string();

        let conv2 = mgr
            .new_conversation(&agent_id, serde_json::json!("Concurrent B"))
            .await?;
        let cid2 = conv2.get("conversationId").and_then(|v| v.as_str()).unwrap().to_string();

        // Send messages concurrently to both conversations
        let mgr1 = mgr.clone();
        let mgr2 = mgr.clone();
        let agent_id1 = agent_id.clone();
        let agent_id2 = agent_id.clone();

        let task1 = tokio::spawn(async move {
            mgr1.send_user_message(
                &agent_id1,
                serde_json::json!({
                    "conversationId": cid1,
                    "items": [{"type": "text", "data": {"text": "Concurrent message 1"}}]
                }),
            )
            .await
        });

        let task2 = tokio::spawn(async move {
            mgr2.send_user_message(
                &agent_id2,
                serde_json::json!({
                    "conversationId": cid2,
                    "items": [{"type": "text", "data": {"text": "Concurrent message 2"}}]
                }),
            )
            .await
        });

        // Both should succeed
        let result1 = task1.await?;
        let result2 = task2.await?;

        assert!(result1.is_ok(), "First concurrent message should succeed");
        assert!(result2.is_ok(), "Second concurrent message should succeed");

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}
