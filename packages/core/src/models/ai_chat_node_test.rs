//! Tests for AiChatNode wrapper

#[cfg(test)]
mod tests {
    use crate::models::{AiChatMessage, AiChatNode, Node};
    use serde_json::json;

    fn ai_chat_node(properties: serde_json::Value) -> Node {
        Node::new("ai-chat".to_string(), "My chat".to_string(), properties)
    }

    #[test]
    fn from_node_validates_type() {
        let node = ai_chat_node(json!({ "ai-chat": {} }));
        assert!(AiChatNode::from_node(node).is_ok());

        let wrong = Node::new("task".to_string(), "x".to_string(), json!({}));
        let err = AiChatNode::from_node(wrong).unwrap_err();
        assert!(err.to_string().contains("Expected 'ai-chat'"));
    }

    #[test]
    fn from_node_reads_nested_namespace() {
        let node = ai_chat_node(json!({
            "ai-chat": {
                "turn_status": "processing",
                "session_status": "active",
                "provider": "native",
                "model": "gemma-3n-e4b",
                "messages": [
                    { "role": "user", "content": "Hello", "timestamp": "2026-06-05T00:00:00Z" }
                ]
            }
        }));

        let chat = AiChatNode::from_node(node).unwrap();
        assert_eq!(chat.turn_status, "processing");
        assert_eq!(chat.session_status, "active");
        assert_eq!(chat.provider.as_deref(), Some("native"));
        assert_eq!(chat.model.as_deref(), Some("gemma-3n-e4b"));
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].role, "user");
        assert_eq!(chat.messages[0].content, "Hello");
        assert_eq!(
            chat.messages[0].timestamp.as_deref(),
            Some("2026-06-05T00:00:00Z")
        );
    }

    #[test]
    fn from_node_falls_back_to_flat_properties() {
        // After flatten_properties_for_api, fields live at the top level.
        let node = ai_chat_node(json!({
            "turn_status": "idle",
            "session_status": "archived",
            "model": "gemma-3n-e4b",
            "messages": [{ "role": "assistant", "content": "Hi" }]
        }));

        let chat = AiChatNode::from_node(node).unwrap();
        assert_eq!(chat.turn_status, "idle");
        assert_eq!(chat.session_status, "archived");
        assert_eq!(chat.model.as_deref(), Some("gemma-3n-e4b"));
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].role, "assistant");
    }

    /// Deliberately NOT recognized: a real "Send" click once wrote camelCase
    /// (`turnStatus`) here, and `from_node` briefly dual-read it as a
    /// convenience. That was wrong, not just redundant — writes here
    /// deep-merge onto the existing stored object instead of replacing it,
    /// so a stray camelCase key is never cleaned up, and reading it
    /// preferentially meant a stale value permanently shadowed every
    /// subsequent fresh canonical write (this is exactly how
    /// `ai_chat_send_to_idle_test` broke: the turn completed and the daemon
    /// correctly wrote `turn_status: "idle"`, but the stale `turnStatus:
    /// "processing"` from the original send kept winning forever). The real
    /// fix is at the write side — every writer uses this exact canonical
    /// key — so `from_node` must ignore camelCase entirely, not merely
    /// deprioritize it.
    #[test]
    fn from_node_ignores_camel_case_turn_and_session_status() {
        let node = ai_chat_node(json!({
            "ai-chat": {
                "turnStatus": "processing",
                "sessionStatus": "archived",
                "messages": []
            }
        }));

        let chat = AiChatNode::from_node(node).unwrap();
        assert_eq!(
            chat.turn_status, "",
            "camelCase must not be read as the turn axis"
        );
        assert_eq!(
            chat.session_status, "",
            "camelCase must not be read as the session axis"
        );
    }

    /// The scenario that broke `ai_chat_send_to_idle_test`: a stale camelCase
    /// key sits beside a fresh canonical one (deep-merge never removes the
    /// former). The canonical key must win regardless — this is what makes
    /// it safe for `to_properties_value` to always emit only the canonical
    /// name without needing to actively strip a same-axis camelCase key.
    #[test]
    fn from_node_reads_canonical_key_even_beside_a_stale_camel_case_one() {
        let node = ai_chat_node(json!({
            "ai-chat": {
                "turnStatus": "processing",
                "turn_status": "idle",
                "sessionStatus": "archived",
                "session_status": "active",
                "messages": []
            }
        }));

        let chat = AiChatNode::from_node(node).unwrap();
        assert_eq!(chat.turn_status, "idle");
        assert_eq!(chat.session_status, "active");
    }

    #[test]
    fn from_node_defaults_when_empty() {
        let node = ai_chat_node(json!({ "ai-chat": {} }));
        let chat = AiChatNode::from_node(node).unwrap();
        assert_eq!(chat.turn_status, "");
        assert_eq!(chat.session_status, "");
        assert!(chat.provider.is_none());
        assert!(chat.model.is_none());
        assert!(chat.messages.is_empty());
    }

    #[test]
    fn round_trips_through_into_node_and_from_node() {
        let original = ai_chat_node(json!({
            "ai-chat": {
                "turn_status": "processing",
                "session_status": "active",
                "provider": "native",
                "model": "gemma-3n-e4b",
                "messages": [
                    { "role": "user", "content": "Q", "timestamp": "2026-06-05T00:00:00Z" }
                ]
            }
        }));

        let chat = AiChatNode::from_node(original).unwrap();
        let rebuilt = chat.clone().into_node();
        let chat2 = AiChatNode::from_node(rebuilt).unwrap();

        assert_eq!(chat.turn_status, chat2.turn_status);
        assert_eq!(chat.session_status, chat2.session_status);
        assert_eq!(chat.provider, chat2.provider);
        assert_eq!(chat.model, chat2.model);
        assert_eq!(chat.messages, chat2.messages);
    }

    /// The split's whole point: archiving a session must not disturb whatever
    /// the turn state happened to be, and vice versa — the two axes are
    /// independently valued and independently persisted, never one clobbering
    /// the other through a shared key.
    #[test]
    fn session_status_and_turn_status_are_independent_through_a_round_trip() {
        let node = ai_chat_node(json!({
            "ai-chat": {
                "turn_status": "processing",
                "session_status": "active",
                "messages": []
            }
        }));

        // The PTY path archives the session — it never touches turn_status.
        let mut chat = AiChatNode::from_node(node).unwrap();
        assert_eq!(chat.turn_status, "processing");
        chat.session_status = "archived".to_string();

        let rebuilt = chat.into_node();
        let chat2 = AiChatNode::from_node(rebuilt).unwrap();
        assert_eq!(
            chat2.session_status, "archived",
            "the archive write must persist"
        );
        assert_eq!(
            chat2.turn_status, "processing",
            "archiving the session must not disturb the turn state"
        );

        // The daemon then completes the turn — it never touches session_status.
        let mut chat3 = chat2;
        chat3.turn_status = "idle".to_string();
        let rebuilt2 = chat3.into_node();
        let chat4 = AiChatNode::from_node(rebuilt2).unwrap();
        assert_eq!(
            chat4.turn_status, "idle",
            "the turn-completion write must persist"
        );
        assert_eq!(
            chat4.session_status, "archived",
            "completing the turn must not un-archive the session"
        );
    }

    #[test]
    fn reasoning_persists_through_round_trip() {
        let node = ai_chat_node(json!({
            "ai-chat": {
                "messages": [
                    {
                        "role": "assistant",
                        "content": "The answer is 42.",
                        "reasoning": "Considered the question carefully."
                    }
                ]
            }
        }));

        let chat = AiChatNode::from_node(node).unwrap();
        assert_eq!(
            chat.messages[0].reasoning.as_deref(),
            Some("Considered the question carefully.")
        );

        let rebuilt = chat.into_node();
        let chat2 = AiChatNode::from_node(rebuilt).unwrap();
        assert_eq!(
            chat2.messages[0].reasoning.as_deref(),
            Some("Considered the question carefully.")
        );
    }

    #[test]
    fn to_properties_value_preserves_sibling_namespaces() {
        // Simulate the daemon's read-modify-write: a node whose properties carry
        // both the ai-chat namespace and an unrelated sibling namespace.
        let node = ai_chat_node(json!({
            "ai-chat": { "turn_status": "processing", "session_status": "active", "messages": [] },
            "custom": { "keep": "me" }
        }));

        let mut chat = AiChatNode::from_node(node.clone()).unwrap();
        chat.turn_status = "idle".to_string();
        chat.messages.push(AiChatMessage {
            role: "assistant".to_string(),
            content: "done".to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: Vec::new(),
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        });

        let mut props = node.properties.clone();
        props["ai-chat"] = chat.to_properties_value();

        assert_eq!(props["custom"]["keep"], json!("me"));
        assert_eq!(props["ai-chat"]["turn_status"], json!("idle"));
        assert_eq!(
            props["ai-chat"]["session_status"],
            json!("active"),
            "an untouched axis must survive the write unchanged"
        );
        assert_eq!(props["ai-chat"]["messages"][0]["content"], json!("done"));
    }

    /// One unreadable message must not take the conversation with it.
    ///
    /// Decoding the array as a single `Vec` fails wholesale on any one bad
    /// element, and callers that write the node back — a turn-status update
    /// does — re-serialise from this struct, so a read that silently yields
    /// zero messages *persists* as an erased conversation. Per-message
    /// decoding keeps the loss to the message actually at fault.
    #[test]
    fn one_unreadable_message_does_not_erase_the_conversation() {
        let node = ai_chat_node(json!({
            "ai-chat": {
                "turn_status": "idle",
                "messages": [
                    { "role": "user", "content": "hello" },
                    // Malformed: `content` is required and must be a string.
                    { "role": "assistant", "content": 42 },
                    { "role": "assistant", "content": "goodbye" }
                ]
            }
        }));

        let chat = AiChatNode::from_node(node).unwrap();
        assert_eq!(
            chat.messages.len(),
            2,
            "only the unreadable message may be dropped"
        );
        assert_eq!(chat.messages[0].content, "hello");
        assert_eq!(chat.messages[1].content, "goodbye");
    }

    /// A write record missing its identity is the concrete case that made the
    /// above reachable: `canonicalArgs` is required, so a legacy write without
    /// one fails to decode. Its message is dropped; the rest of the conversation
    /// survives, rather than the whole history decoding to empty.
    #[test]
    fn a_write_without_an_identity_does_not_erase_the_conversation() {
        let node = ai_chat_node(json!({
            "ai-chat": {
                "turn_status": "idle",
                "messages": [
                    { "role": "user", "content": "add a task" },
                    {
                        "role": "assistant",
                        "content": "Added.",
                        "completedWrites": [
                            { "tool": "create_node", "nodeId": "nodespace://n1" }
                        ]
                    },
                    { "role": "user", "content": "thanks" }
                ]
            }
        }));

        let chat = AiChatNode::from_node(node).unwrap();
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].content, "add a task");
        assert_eq!(chat.messages[1].content, "thanks");
    }

    /// The identity is required, so a well-formed write round-trips with it.
    #[test]
    fn completed_write_identity_round_trips() {
        let node = ai_chat_node(json!({
            "ai-chat": {
                "turn_status": "idle",
                "messages": [{
                    "role": "assistant",
                    "content": "Added.",
                    "completedWrites": [{
                        "tool": "create_node",
                        "nodeId": "nodespace://n1",
                        "canonicalArgs": r#"{"content":"Buy milk"}"#
                    }]
                }]
            }
        }));

        let chat = AiChatNode::from_node(node).unwrap();
        assert_eq!(
            chat.messages[0].completed_writes[0].canonical_args,
            r#"{"content":"Buy milk"}"#
        );
    }
}
