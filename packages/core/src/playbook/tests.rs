//! Tests for the Playbook Engine
//!
//! Phase 1: TriggerKey matching, lifecycle operations, trigger index management,
//!          schema drift detection, PropertyChanged dual lookup, and rule parsing.
//! Phase 2: ExecutionWorkItem, trigger_node_id helper, queue/processor behavior.

mod tests {
    use crate::db::events::{DomainEvent, EventEnvelope, EventMetadata, PropertyChange};
    use crate::models::Node;
    use crate::playbook::lifecycle::*;
    use crate::playbook::types::*;
    use chrono::Utc;
    use serde_json::json;

    /// Helper: create a minimal playbook node with the given rules JSON.
    fn make_playbook_node(id: &str, rules: serde_json::Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: "playbook".to_string(),
            content: format!("Test Playbook {}", id),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties: json!({ "rules": rules }),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        }
    }

    /// Helper: create a playbook node with a specific created_at for ordering tests.
    fn make_playbook_node_at(
        id: &str,
        rules: serde_json::Value,
        created_at: chrono::DateTime<Utc>,
    ) -> Node {
        let mut node = make_playbook_node(id, rules);
        node.created_at = created_at;
        node
    }

    // -----------------------------------------------------------------------
    // Rule parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_graph_event_node_created() {
        let def = RuleDefinition {
            name: "test rule".to_string(),
            trigger: TriggerDefinition {
                trigger_type: "graph_event".to_string(),
                on: Some("node_created".to_string()),
                node_type: Some("invoice".to_string()),
                property_key: None,
                cron: None,
            },
            conditions: vec!["node.status == 'draft'".to_string()],
            actions: vec![],
        };

        let parsed = parse_rule(&def).unwrap();
        assert_eq!(parsed.name, "test rule");
        assert_eq!(parsed.conditions.len(), 1);
        match &parsed.trigger {
            ParsedTrigger::GraphEvent {
                on,
                node_type,
                property_key,
            } => {
                assert_eq!(*on, GraphEventType::NodeCreated);
                assert_eq!(node_type, "invoice");
                assert!(property_key.is_none());
            }
            _ => panic!("expected GraphEvent trigger"),
        }
    }

    #[test]
    fn test_parse_graph_event_property_changed() {
        let def = RuleDefinition {
            name: "status watcher".to_string(),
            trigger: TriggerDefinition {
                trigger_type: "graph_event".to_string(),
                on: Some("property_changed".to_string()),
                node_type: Some("invoice".to_string()),
                property_key: Some("status".to_string()),
                cron: None,
            },
            conditions: vec![],
            actions: vec![ActionDefinition {
                action_type: "update_node".to_string(),
                params: json!({"target": "trigger.node", "property": "status", "value": "overdue"}),
                for_each: None,
            }],
        };

        let parsed = parse_rule(&def).unwrap();
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.actions[0].action_type, ActionType::UpdateNode);
        match &parsed.trigger {
            ParsedTrigger::GraphEvent {
                on, property_key, ..
            } => {
                assert_eq!(*on, GraphEventType::PropertyChanged);
                assert_eq!(property_key.as_deref(), Some("status"));
            }
            _ => panic!("expected GraphEvent trigger"),
        }
    }

    #[test]
    fn test_parse_scheduled_trigger() {
        let def = RuleDefinition {
            name: "daily check".to_string(),
            trigger: TriggerDefinition {
                trigger_type: "scheduled".to_string(),
                on: None,
                node_type: Some("invoice".to_string()),
                property_key: None,
                cron: Some("0 9 * * *".to_string()),
            },
            conditions: vec![],
            actions: vec![],
        };

        let parsed = parse_rule(&def).unwrap();
        match &parsed.trigger {
            ParsedTrigger::Scheduled { cron, node_type } => {
                assert_eq!(cron, "0 9 * * *");
                assert_eq!(node_type, "invoice");
            }
            _ => panic!("expected Scheduled trigger"),
        }
    }

    #[test]
    fn test_parse_invalid_trigger_type() {
        let def = RuleDefinition {
            name: "bad".to_string(),
            trigger: TriggerDefinition {
                trigger_type: "webhook".to_string(),
                on: None,
                node_type: None,
                property_key: None,
                cron: None,
            },
            conditions: vec![],
            actions: vec![],
        };

        assert!(matches!(
            parse_rule(&def),
            Err(PlaybookParseError::InvalidTriggerType(_))
        ));
    }

    #[test]
    fn test_parse_invalid_event_type() {
        let def = RuleDefinition {
            name: "bad".to_string(),
            trigger: TriggerDefinition {
                trigger_type: "graph_event".to_string(),
                on: Some("node_exploded".to_string()),
                node_type: Some("invoice".to_string()),
                property_key: None,
                cron: None,
            },
            conditions: vec![],
            actions: vec![],
        };

        assert!(matches!(
            parse_rule(&def),
            Err(PlaybookParseError::InvalidEventType(_))
        ));
    }

    #[test]
    fn test_parse_missing_node_type() {
        let def = RuleDefinition {
            name: "bad".to_string(),
            trigger: TriggerDefinition {
                trigger_type: "graph_event".to_string(),
                on: Some("node_created".to_string()),
                node_type: None,
                property_key: None,
                cron: None,
            },
            conditions: vec![],
            actions: vec![],
        };

        assert!(matches!(
            parse_rule(&def),
            Err(PlaybookParseError::MissingField(_))
        ));
    }

    #[test]
    fn test_parse_all_action_types() {
        for (input, expected) in [
            ("create_node", ActionType::CreateNode),
            ("update_node", ActionType::UpdateNode),
            ("add_relationship", ActionType::AddRelationship),
            ("remove_relationship", ActionType::RemoveRelationship),
        ] {
            let def = ActionDefinition {
                action_type: input.to_string(),
                params: json!({}),
                for_each: None,
            };
            let parsed = super::super::types::parse_action(&def).unwrap();
            assert_eq!(parsed.action_type, expected);
        }
    }

    #[test]
    fn test_parse_invalid_action_type() {
        let def = ActionDefinition {
            action_type: "spawn_agent".to_string(),
            params: json!({}),
            for_each: None,
        };
        assert!(super::super::types::parse_action(&def).is_err());
    }

    #[test]
    fn test_parse_for_each_action() {
        let def = ActionDefinition {
            action_type: "update_node".to_string(),
            params: json!({"target": "item.id", "property": "reviewed", "value": true}),
            for_each: Some("trigger.node.tasks".to_string()),
        };
        let parsed = super::super::types::parse_action(&def).unwrap();
        assert_eq!(parsed.for_each, Some("trigger.node.tasks".to_string()));
    }

    #[test]
    fn test_parse_rules_from_properties() {
        let properties = json!({
            "rules": [
                {
                    "name": "rule1",
                    "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                    "conditions": [],
                    "actions": []
                },
                {
                    "name": "rule2",
                    "trigger": {"type": "scheduled", "cron": "0 9 * * *", "node_type": "invoice"},
                    "conditions": ["node.status == 'overdue'"],
                    "actions": [{"action_type": "update_node", "params": {}}]
                }
            ]
        });

        let rules = parse_rules_from_properties(&properties).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].name, "rule1");
        assert_eq!(rules[1].name, "rule2");
    }

    // -----------------------------------------------------------------------
    // Lifecycle manager tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_activate_playbook_builds_trigger_index() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "on invoice created",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "invoice"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();

        assert_eq!(mgr.active_playbooks().len(), 1);
        assert!(mgr.active_playbooks().contains_key("pb1"));

        let key = TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "invoice".to_string(),
            property_key: None,
        };
        let rules = mgr.lookup_rules(&[key]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].playbook_id, "pb1");
        assert_eq!(rules[0].rule_index, 0);
    }

    #[test]
    fn test_activate_playbook_idempotent() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "rule1",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();
        mgr.activate_playbook(&node).unwrap(); // no-op

        assert_eq!(mgr.active_playbooks().len(), 1);
    }

    #[test]
    fn test_deactivate_playbook_removes_from_index() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "rule1",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();
        assert_eq!(mgr.active_playbooks().len(), 1);

        mgr.deactivate_playbook("pb1");
        assert_eq!(mgr.active_playbooks().len(), 0);
        assert!(mgr.trigger_index().is_empty());
    }

    #[test]
    fn test_deactivate_nonexistent_is_noop() {
        let mut mgr = PlaybookLifecycleManager::new();
        mgr.deactivate_playbook("nonexistent"); // should not panic
    }

    #[test]
    fn test_disable_playbook_removes_from_index_keeps_in_registry() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "rule1",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();
        mgr.disable_playbook("pb1");

        // Still in active_playbooks but marked disabled
        assert_eq!(mgr.active_playbooks().len(), 1);
        assert_eq!(
            mgr.get_playbook("pb1").unwrap().status,
            PlaybookStatus::Disabled
        );
        // Removed from trigger index
        assert!(mgr.trigger_index().is_empty());
    }

    #[test]
    fn test_reenable_playbook() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "rule1",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();
        mgr.disable_playbook("pb1");
        assert!(mgr.trigger_index().is_empty());

        mgr.reenable_playbook(&node).unwrap();

        // Back in index
        assert_eq!(
            mgr.get_playbook("pb1").unwrap().status,
            PlaybookStatus::Active
        );
        let key = TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        };
        assert_eq!(mgr.lookup_rules(&[key]).len(), 1);
    }

    // -----------------------------------------------------------------------
    // TriggerKey matching tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_property_changed_exact_match() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "status watcher",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "node_type": "invoice",
                    "property_key": "status"
                },
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();

        // Exact match: property_key = "status"
        let exact = TriggerKey::NodeEvent {
            event: NodeEventType::PropertyChanged,
            node_type: "invoice".to_string(),
            property_key: Some("status".to_string()),
        };
        assert_eq!(mgr.lookup_rules(&[exact]).len(), 1);

        // No match: different property
        let other = TriggerKey::NodeEvent {
            event: NodeEventType::PropertyChanged,
            node_type: "invoice".to_string(),
            property_key: Some("amount".to_string()),
        };
        assert_eq!(mgr.lookup_rules(&[other]).len(), 0);
    }

    #[test]
    fn test_property_changed_wildcard_match() {
        let mut mgr = PlaybookLifecycleManager::new();

        // Rule with no property_key = wildcard (matches all property changes)
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "any change watcher",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "node_type": "invoice"
                },
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();

        // Wildcard key lookup
        let wildcard = TriggerKey::NodeEvent {
            event: NodeEventType::PropertyChanged,
            node_type: "invoice".to_string(),
            property_key: None,
        };
        assert_eq!(mgr.lookup_rules(&[wildcard]).len(), 1);
    }

    #[test]
    fn test_property_changed_dual_lookup() {
        let mut mgr = PlaybookLifecycleManager::new();

        // Rule 1: exact key "status"
        let node1 = make_playbook_node(
            "pb1",
            json!([{
                "name": "status watcher",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "node_type": "invoice",
                    "property_key": "status"
                },
                "conditions": [],
                "actions": []
            }]),
        );

        // Rule 2: wildcard (any property change on invoice)
        let node2 = make_playbook_node(
            "pb2",
            json!([{
                "name": "any change watcher",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "node_type": "invoice"
                },
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node1).unwrap();
        mgr.activate_playbook(&node2).unwrap();

        // Dual lookup: exact "status" + wildcard None (as trigger_keys_for_event produces)
        let keys = vec![
            TriggerKey::NodeEvent {
                event: NodeEventType::PropertyChanged,
                node_type: "invoice".to_string(),
                property_key: Some("status".to_string()),
            },
            TriggerKey::NodeEvent {
                event: NodeEventType::PropertyChanged,
                node_type: "invoice".to_string(),
                property_key: None,
            },
        ];
        let rules = mgr.lookup_rules(&keys);
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn test_trigger_keys_for_node_created_event() {
        let event = DomainEvent::NodeCreated {
            node_id: "node:123".to_string(),
            node_type: "invoice".to_string(),
        };

        let keys = trigger_keys_for_event(&event);
        assert_eq!(keys.len(), 1);
        assert_eq!(
            keys[0],
            TriggerKey::NodeEvent {
                event: NodeEventType::NodeCreated,
                node_type: "invoice".to_string(),
                property_key: None,
            }
        );
    }

    #[test]
    fn test_trigger_keys_for_node_updated_event() {
        let event = DomainEvent::NodeUpdated {
            node_id: "node:123".to_string(),
            node_type: "invoice".to_string(),
            changed_properties: vec![
                PropertyChange {
                    key: "invoice.status".to_string(),
                    old_value: Some(json!("draft")),
                    new_value: Some(json!("sent")),
                },
                PropertyChange {
                    key: "invoice.amount".to_string(),
                    old_value: Some(json!(100)),
                    new_value: Some(json!(200)),
                },
            ],
        };

        let keys = trigger_keys_for_event(&event);
        // 2 exact property keys + 1 wildcard = 3
        assert_eq!(keys.len(), 3);

        assert!(keys.contains(&TriggerKey::NodeEvent {
            event: NodeEventType::PropertyChanged,
            node_type: "invoice".to_string(),
            property_key: Some("invoice.status".to_string()),
        }));
        assert!(keys.contains(&TriggerKey::NodeEvent {
            event: NodeEventType::PropertyChanged,
            node_type: "invoice".to_string(),
            property_key: Some("invoice.amount".to_string()),
        }));
        assert!(keys.contains(&TriggerKey::NodeEvent {
            event: NodeEventType::PropertyChanged,
            node_type: "invoice".to_string(),
            property_key: None,
        }));
    }

    #[test]
    fn test_trigger_keys_for_node_updated_no_changes() {
        let event = DomainEvent::NodeUpdated {
            node_id: "node:123".to_string(),
            node_type: "invoice".to_string(),
            changed_properties: vec![],
        };

        let keys = trigger_keys_for_event(&event);
        // No property changes → no trigger keys
        assert!(keys.is_empty());
    }

    #[test]
    fn test_trigger_keys_for_node_deleted() {
        let event = DomainEvent::NodeDeleted {
            id: "node:123".to_string(),
            node_type: "invoice".to_string(),
        };

        let keys = trigger_keys_for_event(&event);
        assert!(keys.is_empty());
    }

    // -----------------------------------------------------------------------
    // Rule ordering tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_rules_ordered_by_created_at_then_index() {
        let mut mgr = PlaybookLifecycleManager::new();

        let earlier = Utc::now() - chrono::Duration::hours(2);
        let later = Utc::now();

        // Playbook A: created earlier, 2 rules
        let pb_a = make_playbook_node_at(
            "pb-a",
            json!([
                {
                    "name": "a-rule-0",
                    "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                    "conditions": [],
                    "actions": []
                },
                {
                    "name": "a-rule-1",
                    "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                    "conditions": [],
                    "actions": []
                }
            ]),
            earlier,
        );

        // Playbook B: created later, 1 rule
        let pb_b = make_playbook_node_at(
            "pb-b",
            json!([{
                "name": "b-rule-0",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
            later,
        );

        mgr.activate_playbook(&pb_a).unwrap();
        mgr.activate_playbook(&pb_b).unwrap();

        let key = TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        };
        let rules = mgr.lookup_rules(&[key]);

        assert_eq!(rules.len(), 3);
        // Earlier playbook's rules first, in index order
        assert_eq!(rules[0].playbook_id, "pb-a");
        assert_eq!(rules[0].rule_index, 0);
        assert_eq!(rules[1].playbook_id, "pb-a");
        assert_eq!(rules[1].rule_index, 1);
        // Later playbook's rules after
        assert_eq!(rules[2].playbook_id, "pb-b");
        assert_eq!(rules[2].rule_index, 0);
    }

    // -----------------------------------------------------------------------
    // Schema drift tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_schema_update_disables_referencing_playbooks() {
        let mut mgr = PlaybookLifecycleManager::new();

        // Playbook referencing "invoice" node type
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "invoice watcher",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "node_type": "invoice",
                    "property_key": "status"
                },
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();
        assert_eq!(
            mgr.get_playbook("pb1").unwrap().status,
            PlaybookStatus::Active
        );

        // Schema for "invoice" is updated
        let disabled = mgr.handle_schema_update("invoice", "2.0.0");
        assert_eq!(disabled, vec!["pb1"]);
        assert_eq!(
            mgr.get_playbook("pb1").unwrap().status,
            PlaybookStatus::Disabled
        );
    }

    #[test]
    fn test_schema_update_ignores_unrelated_playbooks() {
        let mut mgr = PlaybookLifecycleManager::new();

        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "task watcher",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();

        // Schema for "invoice" is updated — should NOT affect "task" playbook
        let disabled = mgr.handle_schema_update("invoice", "2.0.0");
        assert!(disabled.is_empty());
        assert_eq!(
            mgr.get_playbook("pb1").unwrap().status,
            PlaybookStatus::Active
        );
    }

    // -----------------------------------------------------------------------
    // Cron registry tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_scheduled_trigger_added_to_cron_registry() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "daily invoice check",
                "trigger": {"type": "scheduled", "cron": "0 9 * * *", "node_type": "invoice"},
                "conditions": ["node.status == 'overdue'"],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();

        let registry = mgr.cron_registry();
        assert_eq!(registry.len(), 1);
        assert_eq!(registry[0].cron_expression, "0 9 * * *");
        assert_eq!(registry[0].node_type, "invoice");
        assert_eq!(registry[0].rules.len(), 1);
    }

    #[test]
    fn test_cron_deduplication_same_expression_and_type() {
        let mut mgr = PlaybookLifecycleManager::new();

        let node1 = make_playbook_node(
            "pb1",
            json!([{
                "name": "check 1",
                "trigger": {"type": "scheduled", "cron": "0 9 * * *", "node_type": "invoice"},
                "conditions": [],
                "actions": []
            }]),
        );

        let node2 = make_playbook_node(
            "pb2",
            json!([{
                "name": "check 2",
                "trigger": {"type": "scheduled", "cron": "0 9 * * *", "node_type": "invoice"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node1).unwrap();
        mgr.activate_playbook(&node2).unwrap();

        // Same cron + node_type → single registry entry with 2 rules
        let registry = mgr.cron_registry();
        assert_eq!(registry.len(), 1);
        assert_eq!(registry[0].rules.len(), 2);
    }

    #[test]
    fn test_cron_registry_cleaned_on_deactivate() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "daily check",
                "trigger": {"type": "scheduled", "cron": "0 9 * * *", "node_type": "invoice"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();
        assert_eq!(mgr.cron_registry().len(), 1);

        mgr.deactivate_playbook("pb1");
        assert!(mgr.cron_registry().is_empty());
    }

    // -----------------------------------------------------------------------
    // Relationship trigger tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_relationship_trigger_in_index() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_playbook_node(
            "pb1",
            json!([{
                "name": "on relationship added",
                "trigger": {
                    "type": "graph_event",
                    "on": "relationship_added",
                    "node_type": "story"
                },
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node).unwrap();

        let key = TriggerKey::RelationshipEvent {
            event: RelEventType::RelationshipAdded,
            source_node_type: "story".to_string(),
        };
        let rules = mgr.lookup_rules(&[key]);
        assert_eq!(rules.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Mixed trigger type tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_playbook_with_mixed_triggers() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_playbook_node(
            "pb1",
            json!([
                {
                    "name": "on created",
                    "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                    "conditions": [],
                    "actions": []
                },
                {
                    "name": "daily scan",
                    "trigger": {"type": "scheduled", "cron": "0 9 * * *", "node_type": "task"},
                    "conditions": [],
                    "actions": []
                },
                {
                    "name": "on rel added",
                    "trigger": {
                        "type": "graph_event",
                        "on": "relationship_added",
                        "node_type": "task"
                    },
                    "conditions": [],
                    "actions": []
                }
            ]),
        );

        mgr.activate_playbook(&node).unwrap();

        // 2 graph_event rules in trigger index
        assert_eq!(mgr.trigger_index().len(), 2);
        // 1 cron entry
        assert_eq!(mgr.cron_registry().len(), 1);

        // Deactivate cleans everything
        mgr.deactivate_playbook("pb1");
        assert!(mgr.trigger_index().is_empty());
        assert!(mgr.cron_registry().is_empty());
    }

    // -----------------------------------------------------------------------
    // Multiple playbooks interacting
    // -----------------------------------------------------------------------

    #[test]
    fn test_deactivate_one_playbook_preserves_others() {
        let mut mgr = PlaybookLifecycleManager::new();

        let node1 = make_playbook_node(
            "pb1",
            json!([{
                "name": "rule1",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        let node2 = make_playbook_node(
            "pb2",
            json!([{
                "name": "rule2",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_playbook(&node1).unwrap();
        mgr.activate_playbook(&node2).unwrap();

        let key = TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        };
        assert_eq!(mgr.lookup_rules(&[key.clone()]).len(), 2);

        mgr.deactivate_playbook("pb1");
        let remaining = mgr.lookup_rules(&[key]);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].playbook_id, "pb2");
    }

    // -----------------------------------------------------------------------
    // Phase 2: ExecutionWorkItem and trigger_node_id tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_trigger_node_id_node_created() {
        let event = DomainEvent::NodeCreated {
            node_id: "node:abc".to_string(),
            node_type: "invoice".to_string(),
        };
        assert_eq!(
            super::super::engine::trigger_node_id(&event),
            Some("node:abc")
        );
    }

    #[test]
    fn test_trigger_node_id_node_updated() {
        let event = DomainEvent::NodeUpdated {
            node_id: "node:xyz".to_string(),
            node_type: "task".to_string(),
            changed_properties: vec![],
        };
        assert_eq!(
            super::super::engine::trigger_node_id(&event),
            Some("node:xyz")
        );
    }

    #[test]
    fn test_trigger_node_id_node_deleted_returns_none() {
        let event = DomainEvent::NodeDeleted {
            id: "node:123".to_string(),
            node_type: "invoice".to_string(),
        };
        assert_eq!(super::super::engine::trigger_node_id(&event), None);
    }

    #[test]
    fn test_trigger_node_id_relationship_returns_none() {
        let event = DomainEvent::RelationshipCreated {
            relationship: crate::db::events::RelationshipEvent {
                id: "rel:1".to_string(),
                from_id: "node:a".to_string(),
                to_id: "node:b".to_string(),
                relationship_type: "has_child".to_string(),
                properties: json!({}),
            },
        };
        assert_eq!(super::super::engine::trigger_node_id(&event), None);
    }

    #[test]
    fn test_execution_work_item_construction() {
        let mut mgr = PlaybookLifecycleManager::new();
        let pb_node = make_playbook_node(
            "pb1",
            json!([{
                "name": "on task created",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": ["node.status == 'open'"],
                "actions": [{"action_type": "update_node", "params": {}}]
            }]),
        );
        mgr.activate_playbook(&pb_node).unwrap();

        let key = TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        };
        let matched_rules = mgr.lookup_rules(&[key]);
        assert_eq!(matched_rules.len(), 1);

        let trigger_node = Node {
            id: "node:task-1".to_string(),
            node_type: "task".to_string(),
            content: "My task".to_string(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties: json!({"status": "open"}),
            mentions: vec![],
            mentioned_in: vec![],
            title: Some("My task".to_string()),
            lifecycle_status: "active".to_string(),
        };

        let envelope = EventEnvelope {
            event: DomainEvent::NodeCreated {
                node_id: "node:task-1".to_string(),
                node_type: "task".to_string(),
            },
            metadata: EventMetadata {
                source_client_id: Some("tauri-main".to_string()),
                playbook_context: None,
            },
        };

        let work_item = ExecutionWorkItem {
            rules: matched_rules,
            trigger_event: envelope,
            trigger_node,
        };

        // Verify the work item carries all the data the processor needs
        assert_eq!(work_item.rules.len(), 1);
        assert_eq!(work_item.rules[0].playbook_id, "pb1");
        assert_eq!(work_item.rules[0].rule.name, "on task created");
        assert_eq!(work_item.rules[0].rule.conditions.len(), 1);
        assert_eq!(work_item.rules[0].rule.actions.len(), 1);
        assert_eq!(work_item.trigger_node.id, "node:task-1");
        assert_eq!(work_item.trigger_node.node_type, "task");
        assert!(work_item.trigger_event.metadata.playbook_context.is_none());
    }

    #[test]
    fn test_execution_work_item_with_playbook_context() {
        use crate::db::events::PlaybookExecutionContext;

        let trigger_node = Node {
            id: "node:invoice-1".to_string(),
            node_type: "invoice".to_string(),
            content: "Invoice".to_string(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties: json!({}),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        };

        let envelope = EventEnvelope {
            event: DomainEvent::NodeUpdated {
                node_id: "node:invoice-1".to_string(),
                node_type: "invoice".to_string(),
                changed_properties: vec![PropertyChange {
                    key: "status".to_string(),
                    old_value: Some(json!("draft")),
                    new_value: Some(json!("sent")),
                }],
            },
            metadata: EventMetadata {
                source_client_id: Some("playbook_engine".to_string()),
                playbook_context: Some(PlaybookExecutionContext {
                    originating_event_id: "evt-root-123".to_string(),
                    depth: 3,
                    source_playbook_id: "pb-upstream".to_string(),
                }),
            },
        };

        let work_item = ExecutionWorkItem {
            rules: vec![],
            trigger_event: envelope,
            trigger_node,
        };

        // Verify playbook_context is carried through for cycle detection
        let ctx = work_item
            .trigger_event
            .metadata
            .playbook_context
            .as_ref()
            .unwrap();
        assert_eq!(ctx.depth, 3);
        assert_eq!(ctx.originating_event_id, "evt-root-123");
        assert_eq!(ctx.source_playbook_id, "pb-upstream");
    }

    // -----------------------------------------------------------------------
    // Phase 2: Queue and RuleProcessor async tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_execution_queue_send_receive() {
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<ExecutionWorkItem>(super::super::engine::EXECUTION_QUEUE_CAPACITY);

        let trigger_node = Node {
            id: "node:1".to_string(),
            node_type: "task".to_string(),
            content: "".to_string(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties: json!({}),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        };

        let envelope = EventEnvelope {
            event: DomainEvent::NodeCreated {
                node_id: "node:1".to_string(),
                node_type: "task".to_string(),
            },
            metadata: EventMetadata {
                source_client_id: None,
                playbook_context: None,
            },
        };

        let work_item = ExecutionWorkItem {
            rules: vec![],
            trigger_event: envelope,
            trigger_node,
        };

        tx.try_send(work_item).unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.trigger_node.id, "node:1");
    }

    #[tokio::test]
    async fn test_execution_queue_backpressure() {
        // Use a tiny capacity to test backpressure
        let (tx, _rx) = tokio::sync::mpsc::channel::<ExecutionWorkItem>(1);

        let make_item = || {
            let trigger_node = Node {
                id: "node:1".to_string(),
                node_type: "task".to_string(),
                content: "".to_string(),
                version: 1,
                created_at: Utc::now(),
                modified_at: Utc::now(),
                properties: json!({}),
                mentions: vec![],
                mentioned_in: vec![],
                title: None,
                lifecycle_status: "active".to_string(),
            };
            let envelope = EventEnvelope {
                event: DomainEvent::NodeCreated {
                    node_id: "node:1".to_string(),
                    node_type: "task".to_string(),
                },
                metadata: EventMetadata {
                    source_client_id: None,
                    playbook_context: None,
                },
            };
            ExecutionWorkItem {
                rules: vec![],
                trigger_event: envelope,
                trigger_node,
            }
        };

        // First send succeeds
        assert!(tx.try_send(make_item()).is_ok());
        // Second send fails (channel full, capacity=1)
        assert!(tx.try_send(make_item()).is_err());
    }

    #[tokio::test]
    async fn test_rule_processor_drains_and_shuts_down() {
        let (tx, rx) =
            tokio::sync::mpsc::channel::<ExecutionWorkItem>(super::super::engine::EXECUTION_QUEUE_CAPACITY);

        // Spawn the processor
        let handle = tokio::spawn(super::super::engine::rule_processor_loop(rx));

        // Send a work item
        let trigger_node = Node {
            id: "node:1".to_string(),
            node_type: "task".to_string(),
            content: "".to_string(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties: json!({}),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        };
        let envelope = EventEnvelope {
            event: DomainEvent::NodeCreated {
                node_id: "node:1".to_string(),
                node_type: "task".to_string(),
            },
            metadata: EventMetadata {
                source_client_id: None,
                playbook_context: None,
            },
        };
        tx.send(ExecutionWorkItem {
            rules: vec![],
            trigger_event: envelope,
            trigger_node,
        })
        .await
        .unwrap();

        // Drop sender → processor will drain remaining items and exit
        drop(tx);

        // Processor should complete without error
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(result.is_ok(), "RuleProcessor should shut down promptly");
        assert!(
            result.unwrap().is_ok(),
            "RuleProcessor should not panic"
        );
    }
}
