use anyhow::Result;
use codex_orchestrator::codex::Manager;
mod util;

fn set_stub_codex() {
    let stub: String = env!("CARGO_BIN_EXE_stub_codex").to_string();
    std::env::set_var("CODEX_BIN", &stub);
}

#[tokio::test]
async fn test_list_conversations_empty() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("test-agent".to_string()), None).await?;

        // List conversations before creating any
        let result = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;

        let items = result
            .get("items")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(items.len(), 0, "Should start with no conversations");

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_list_conversations_with_items() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("test-agent".to_string()), None).await?;

        // Create a few conversations
        let conv1 = mgr
            .new_conversation(&agent_id, serde_json::json!("First conversation"))
            .await?;
        let cid1 = conv1
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        let conv2 = mgr
            .new_conversation(&agent_id, serde_json::json!("Second conversation"))
            .await?;
        let cid2 = conv2
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // List conversations
        let result = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;

        let items = result
            .get("items")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(items.len(), 2, "Should have 2 conversations");

        // Check that both conversation IDs are present
        let ids: Vec<String> = items
            .iter()
            .filter_map(|item| {
                item.get("conversationId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        assert!(ids.contains(&cid1), "Should contain first conversation ID");
        assert!(ids.contains(&cid2), "Should contain second conversation ID");

        // Check that items have required fields
        for item in items {
            assert!(item.get("conversationId").is_some());
            assert!(item.get("path").is_some());
            assert!(item.get("preview").is_some());
            assert!(item.get("timestamp").is_some());
        }

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_list_conversations_pagination() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("test-agent".to_string()), None).await?;

        // Create 5 conversations
        for i in 0..5 {
            mgr.new_conversation(&agent_id, serde_json::json!(format!("Conversation {}", i)))
                .await?;
        }

        // List with page size 2
        let result = mgr
            .list_conversations(&agent_id, serde_json::json!({"pageSize": 2}))
            .await?;

        let items = result
            .get("items")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(items.len(), 2, "Should return 2 items with pageSize=2");

        let next_cursor = result
            .get("nextCursor")
            .and_then(|v| v.as_str());
        assert!(next_cursor.is_some(), "Should have a nextCursor");

        // Get next page
        let result2 = mgr
            .list_conversations(
                &agent_id,
                serde_json::json!({"pageSize": 2, "cursor": next_cursor.unwrap()}),
            )
            .await?;

        let items2 = result2
            .get("items")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(items2.len(), 2, "Should return 2 more items");

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_resume_conversation() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("test-agent".to_string()), None).await?;

        // Create a conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Original conversation"))
            .await?;
        let rollout_path = conv
            .get("rolloutPath")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Resume the conversation
        let resumed = mgr
            .resume_conversation(
                &agent_id,
                serde_json::json!({
                    "path": rollout_path
                }),
            )
            .await?;

        // Check response
        assert!(resumed.get("conversationId").is_some());
        assert!(resumed.get("model").is_some());
        assert_eq!(
            resumed.get("model").and_then(|v| v.as_str()),
            Some("gpt-5")
        );

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_resume_conversation_with_overrides() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("test-agent".to_string()), None).await?;

        // Create a conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Original conversation"))
            .await?;
        let rollout_path = conv
            .get("rolloutPath")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Resume with overrides
        let resumed = mgr
            .resume_conversation(
                &agent_id,
                serde_json::json!({
                    "path": rollout_path,
                    "overrides": {
                        "model": "gpt-4",
                        "approvalPolicy": "never"
                    }
                }),
            )
            .await?;

        // Check that conversation was resumed
        assert!(resumed.get("conversationId").is_some());

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_archive_conversation() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("test-agent".to_string()), None).await?;

        // Create a conversation
        let conv = mgr
            .new_conversation(&agent_id, serde_json::json!("Test conversation"))
            .await?;
        let cid = conv
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Verify it's in the list
        let before_archive = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;
        let items_before = before_archive
            .get("items")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(items_before.len(), 1, "Should have 1 conversation before archive");

        // Archive the conversation
        let result = mgr
            .archive_conversation(&agent_id, serde_json::json!({"conversationId": cid}))
            .await?;

        assert_eq!(
            result.get("ok").and_then(|v| v.as_bool()),
            Some(true),
            "Archive should return ok: true"
        );

        // Verify it's removed from the list
        let after_archive = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;
        let items_after = after_archive
            .get("items")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(items_after.len(), 0, "Should have 0 conversations after archive");

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_full_conversation_lifecycle() -> Result<()> {
    set_stub_codex();
    util::with_timeout(async move {
        let mgr = Manager::default();
        let agent_id = mgr.spawn_agent(Some("lifecycle-agent".to_string()), None).await?;

        // 1. Start with empty list
        let empty_list = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;
        assert_eq!(
            empty_list
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(0)
        );

        // 2. Create first conversation
        let conv1 = mgr
            .new_conversation(&agent_id, serde_json::json!("First"))
            .await?;
        let cid1 = conv1
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let path1 = conv1
            .get("rolloutPath")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // 3. Send a message
        mgr.send_user_message(&agent_id, serde_json::json!("Hello"))
            .await?;

        // 4. Create second conversation
        let conv2 = mgr
            .new_conversation(&agent_id, serde_json::json!("Second"))
            .await?;
        let cid2 = conv2
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // 5. List should show 2 conversations
        let list = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;
        assert_eq!(
            list.get("items")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(2)
        );

        // 6. Archive first conversation
        mgr.archive_conversation(&agent_id, serde_json::json!({"conversationId": cid1}))
            .await?;

        // 7. List should show 1 conversation
        let list_after_archive = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;
        assert_eq!(
            list_after_archive
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1)
        );

        // 8. Resume the first conversation (from archived rollout)
        let resumed = mgr
            .resume_conversation(&agent_id, serde_json::json!({"path": path1}))
            .await?;
        let resumed_cid = resumed
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(resumed_cid, cid1, "Resumed conversation should have same ID");

        // 9. Archive second conversation
        mgr.archive_conversation(&agent_id, serde_json::json!({"conversationId": cid2}))
            .await?;

        // 10. Final list should be empty
        let final_list = mgr
            .list_conversations(&agent_id, serde_json::json!({}))
            .await?;
        assert_eq!(
            final_list
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(0)
        );

        mgr.kill_agent(&agent_id).await?;
        Ok(())
    })
    .await
}
