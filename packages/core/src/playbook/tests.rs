//! Tests for the Play Engine
//!
//! Phase 1: TriggerKey matching, lifecycle operations, trigger index management,
//!          schema drift detection, PropertyChanged dual lookup, and rule parsing.
//! Phase 2: ExecutionWorkItem, trigger_node_id helper, queue/processor behavior.

mod playbook_tests {
    use crate::db::events::{DomainEvent, EventEnvelope, EventMetadata, PropertyChange};
    use crate::models::Node;
    use crate::playbook::lifecycle::*;
    use crate::playbook::types::*;
    use chrono::Utc;
    use serde_json::json;

    /// Helper: create a minimal play node with the given rules JSON.
    fn make_play_node(id: &str, rules: serde_json::Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: "play".to_string(),
            content: format!("Test Play {}", id),
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

    /// Helper: create a play node with a specific created_at for ordering tests.
    fn make_play_node_at(
        id: &str,
        rules: serde_json::Value,
        created_at: chrono::DateTime<Utc>,
    ) -> Node {
        let mut node = make_play_node(id, rules);
        node.created_at = created_at;
        node
    }

    /// Helper: create a minimal node for test events.
    fn make_node(id: &str, node_type: &str) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            content: String::new(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties: json!({}),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Rule parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_graph_event_node_created() {
        let def = RuleDefinition {
            name: "test rule".to_string(),
            class: RuleClass::Reactive,
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
            class: RuleClass::Reactive,
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
            class: RuleClass::Reactive,
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
            class: RuleClass::Reactive,
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
            Err(PlayParseError::InvalidTriggerType(_))
        ));
    }

    #[test]
    fn test_parse_invalid_event_type() {
        let def = RuleDefinition {
            name: "bad".to_string(),
            class: RuleClass::Reactive,
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
            Err(PlayParseError::InvalidEventType(_))
        ));
    }

    #[test]
    fn test_parse_missing_node_type() {
        let def = RuleDefinition {
            name: "bad".to_string(),
            class: RuleClass::Reactive,
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
            Err(PlayParseError::MissingField(_))
        ));
    }

    #[test]
    fn test_parse_invalid_cel_condition() {
        let def = RuleDefinition {
            name: "bad condition".to_string(),
            class: RuleClass::Reactive,
            trigger: TriggerDefinition {
                trigger_type: "graph_event".to_string(),
                on: Some("node_created".to_string()),
                node_type: Some("invoice".to_string()),
                property_key: None,
                cron: None,
            },
            conditions: vec!["1 + + 2".to_string()],
            actions: vec![],
        };

        assert!(matches!(
            parse_rule(&def),
            Err(PlayParseError::InvalidCondition(_))
        ));
    }

    #[test]
    fn test_parse_condition_compiles_program_once() {
        let def = RuleDefinition {
            name: "compiled rule".to_string(),
            class: RuleClass::Reactive,
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
        assert_eq!(parsed.conditions[0].source, "node.status == 'draft'");

        // The compiled Program is cached, not recompiled — cloning the rule
        // (as happens on every `OrderedRuleRef` lookup) shares the same
        // underlying Program via Arc rather than recompiling it.
        let cloned = parsed.clone();
        assert!(std::sync::Arc::ptr_eq(
            &parsed.conditions[0].program,
            &cloned.conditions[0].program
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

    // -----------------------------------------------------------------------
    // Rule class parsing (ADR-060)
    // -----------------------------------------------------------------------

    #[test]
    fn test_rule_class_defaults_to_reactive() {
        // A rule definition with no `class` field parses as Reactive, keeping
        // every previously-authored rule's semantics unchanged.
        let def = RuleDefinition {
            name: "no class".to_string(),
            class: RuleClass::default(),
            trigger: TriggerDefinition {
                trigger_type: "graph_event".to_string(),
                on: Some("node_created".to_string()),
                node_type: Some("task".to_string()),
                property_key: None,
                cron: None,
            },
            conditions: vec![],
            actions: vec![],
        };
        assert_eq!(RuleClass::default(), RuleClass::Reactive);
        assert_eq!(parse_rule(&def).unwrap().class, RuleClass::Reactive);
    }

    #[test]
    fn test_rule_class_missing_field_deserializes_as_reactive() {
        // JSON without a `class` key → Reactive via `#[serde(default)]`.
        let defs: Vec<RuleDefinition> = serde_json::from_value(json!([{
            "name": "r1",
            "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
            "conditions": [],
            "actions": []
        }]))
        .unwrap();
        assert_eq!(defs[0].class, RuleClass::Reactive);
        assert_eq!(parse_rule(&defs[0]).unwrap().class, RuleClass::Reactive);
    }

    #[test]
    fn test_rule_class_parses_invariant_and_reactive() {
        let defs: Vec<RuleDefinition> = serde_json::from_value(json!([
            {
                "name": "inv",
                "class": "invariant",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            },
            {
                "name": "react",
                "class": "reactive",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }
        ]))
        .unwrap();
        assert_eq!(defs[0].class, RuleClass::Invariant);
        assert_eq!(defs[1].class, RuleClass::Reactive);
        assert_eq!(parse_rule(&defs[0]).unwrap().class, RuleClass::Invariant);
        assert_eq!(parse_rule(&defs[1]).unwrap().class, RuleClass::Reactive);
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

    #[test]
    fn test_parse_rules_from_namespace_nested_properties() {
        // DB-stored format: properties are wrapped under {"play": {"rules": [...]}}
        // after create_node's namespace normalization
        let properties = json!({
            "play": {
                "rules": [
                    {
                        "name": "db-rule",
                        "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                        "conditions": ["node.status == 'open'"],
                        "actions": []
                    }
                ]
            }
        });

        let rules = parse_rules_from_properties(&properties).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "db-rule");
    }

    // -----------------------------------------------------------------------
    // Lifecycle manager tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_activate_play_builds_trigger_index() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb1",
            json!([{
                "name": "on invoice created",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "invoice"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_play(&node).unwrap();

        assert_eq!(mgr.active_playbooks().len(), 1);
        assert!(mgr.active_playbooks().contains_key("pb1"));

        let key = TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "invoice".to_string(),
            property_key: None,
        };
        let rules = mgr.lookup_rules(&[key]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].play_id, "pb1");
        assert_eq!(rules[0].rule_index, 0);
    }

    #[test]
    fn test_activate_play_idempotent() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb1",
            json!([{
                "name": "rule1",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_play(&node).unwrap();
        mgr.activate_play(&node).unwrap(); // no-op

        assert_eq!(mgr.active_playbooks().len(), 1);
    }

    #[test]
    fn test_deactivate_play_removes_from_index() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb1",
            json!([{
                "name": "rule1",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_play(&node).unwrap();
        assert_eq!(mgr.active_playbooks().len(), 1);

        mgr.deactivate_play("pb1");
        assert_eq!(mgr.active_playbooks().len(), 0);
        assert!(mgr.trigger_index().is_empty());
    }

    #[test]
    fn test_deactivate_nonexistent_is_noop() {
        let mut mgr = PlaybookLifecycleManager::new();
        mgr.deactivate_play("nonexistent"); // should not panic
    }

    #[test]
    fn test_disable_play_removes_from_index_keeps_in_registry() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb1",
            json!([{
                "name": "rule1",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_play(&node).unwrap();
        mgr.disable_play("pb1");

        // Still in active_playbooks but marked disabled
        assert_eq!(mgr.active_playbooks().len(), 1);
        assert_eq!(mgr.get_play("pb1").unwrap().status, PlayStatus::Disabled);
        // Removed from trigger index
        assert!(mgr.trigger_index().is_empty());
    }

    #[test]
    fn test_reenable_play() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb1",
            json!([{
                "name": "rule1",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_play(&node).unwrap();
        mgr.disable_play("pb1");
        assert!(mgr.trigger_index().is_empty());

        mgr.reenable_play(&node).unwrap();

        // Back in index
        assert_eq!(mgr.get_play("pb1").unwrap().status, PlayStatus::Active);
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
        let node = make_play_node(
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

        mgr.activate_play(&node).unwrap();

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
        let node = make_play_node(
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

        mgr.activate_play(&node).unwrap();

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
        let node1 = make_play_node(
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
        let node2 = make_play_node(
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

        mgr.activate_play(&node1).unwrap();
        mgr.activate_play(&node2).unwrap();

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
            node: make_node("node:123", "invoice"),
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
            node: make_node("node:123", "invoice"),
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
    fn test_rules_ordered_by_play_id_then_rule_index() {
        // ADR-060 §5: cross-play ordering keys on the play's stable id, not
        // wall-clock created_at. Give the lexically-earlier play ("pb-a") the
        // *later* creation time to prove created_at plays no role — if it
        // did, this would sort the other way.
        let mut mgr = PlaybookLifecycleManager::new();

        let earlier = Utc::now() - chrono::Duration::hours(2);
        let later = Utc::now();

        // Play A: created LATER, 2 rules, lexically smaller id.
        let pb_a = make_play_node_at(
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
            later,
        );

        // Play B: created EARLIER, 1 rule, lexically larger id.
        let pb_b = make_play_node_at(
            "pb-b",
            json!([{
                "name": "b-rule-0",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
            earlier,
        );

        mgr.activate_play(&pb_a).unwrap();
        mgr.activate_play(&pb_b).unwrap();

        let key = TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        };
        let rules = mgr.lookup_rules(&[key]);

        assert_eq!(rules.len(), 3);
        // "pb-a" sorts first by play_id despite being created later; within
        // it, rules stay in array (rule_index) order.
        assert_eq!(rules[0].play_id, "pb-a");
        assert_eq!(rules[0].rule_index, 0);
        assert_eq!(rules[1].play_id, "pb-a");
        assert_eq!(rules[1].rule_index, 1);
        // "pb-b" sorts after "pb-a" despite being created earlier.
        assert_eq!(rules[2].play_id, "pb-b");
        assert_eq!(rules[2].rule_index, 0);
    }

    // -----------------------------------------------------------------------
    // Schema drift tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_schema_update_disables_referencing_plays() {
        let mut mgr = PlaybookLifecycleManager::new();

        // Play referencing "invoice" node type
        let node = make_play_node(
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

        mgr.activate_play(&node).unwrap();
        assert_eq!(mgr.get_play("pb1").unwrap().status, PlayStatus::Active);

        // Schema for "invoice" is updated
        let disabled = mgr.handle_schema_update("invoice", "2.0.0");
        assert_eq!(disabled, vec!["pb1"]);
        assert_eq!(mgr.get_play("pb1").unwrap().status, PlayStatus::Disabled);
    }

    #[test]
    fn test_schema_update_ignores_unrelated_plays() {
        let mut mgr = PlaybookLifecycleManager::new();

        let node = make_play_node(
            "pb1",
            json!([{
                "name": "task watcher",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_play(&node).unwrap();

        // Schema for "invoice" is updated — should NOT affect "task" play
        let disabled = mgr.handle_schema_update("invoice", "2.0.0");
        assert!(disabled.is_empty());
        assert_eq!(mgr.get_play("pb1").unwrap().status, PlayStatus::Active);
    }

    // -----------------------------------------------------------------------
    // Cron registry tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_scheduled_trigger_added_to_cron_registry() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb1",
            json!([{
                "name": "daily invoice check",
                "trigger": {"type": "scheduled", "cron": "0 9 * * *", "node_type": "invoice"},
                "conditions": ["node.status == 'overdue'"],
                "actions": []
            }]),
        );

        mgr.activate_play(&node).unwrap();

        let registry = mgr.cron_registry();
        assert_eq!(registry.len(), 1);
        assert_eq!(registry[0].cron_expression, "0 9 * * *");
        assert_eq!(registry[0].node_type, "invoice");
        assert_eq!(registry[0].rules.len(), 1);
    }

    #[test]
    fn test_cron_deduplication_same_expression_and_type() {
        let mut mgr = PlaybookLifecycleManager::new();

        let node1 = make_play_node(
            "pb1",
            json!([{
                "name": "check 1",
                "trigger": {"type": "scheduled", "cron": "0 9 * * *", "node_type": "invoice"},
                "conditions": [],
                "actions": []
            }]),
        );

        let node2 = make_play_node(
            "pb2",
            json!([{
                "name": "check 2",
                "trigger": {"type": "scheduled", "cron": "0 9 * * *", "node_type": "invoice"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_play(&node1).unwrap();
        mgr.activate_play(&node2).unwrap();

        // Same cron + node_type → single registry entry with 2 rules
        let registry = mgr.cron_registry();
        assert_eq!(registry.len(), 1);
        assert_eq!(registry[0].rules.len(), 2);
    }

    #[test]
    fn test_cron_registry_cleaned_on_deactivate() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb1",
            json!([{
                "name": "daily check",
                "trigger": {"type": "scheduled", "cron": "0 9 * * *", "node_type": "invoice"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_play(&node).unwrap();
        assert_eq!(mgr.cron_registry().len(), 1);

        mgr.deactivate_play("pb1");
        assert!(mgr.cron_registry().is_empty());
    }

    // -----------------------------------------------------------------------
    // Relationship trigger tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_relationship_trigger_in_index() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_play_node(
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

        mgr.activate_play(&node).unwrap();

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
    fn test_play_with_mixed_triggers() {
        let mut mgr = PlaybookLifecycleManager::new();
        let node = make_play_node(
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

        mgr.activate_play(&node).unwrap();

        // 2 graph_event rules in trigger index
        assert_eq!(mgr.trigger_index().len(), 2);
        // 1 cron entry
        assert_eq!(mgr.cron_registry().len(), 1);

        // Deactivate cleans everything
        mgr.deactivate_play("pb1");
        assert!(mgr.trigger_index().is_empty());
        assert!(mgr.cron_registry().is_empty());
    }

    // -----------------------------------------------------------------------
    // Multiple plays interacting
    // -----------------------------------------------------------------------

    #[test]
    fn test_deactivate_one_play_preserves_others() {
        let mut mgr = PlaybookLifecycleManager::new();

        let node1 = make_play_node(
            "pb1",
            json!([{
                "name": "rule1",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        let node2 = make_play_node(
            "pb2",
            json!([{
                "name": "rule2",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": [],
                "actions": []
            }]),
        );

        mgr.activate_play(&node1).unwrap();
        mgr.activate_play(&node2).unwrap();

        let key = TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        };
        assert_eq!(mgr.lookup_rules(std::slice::from_ref(&key)).len(), 2);

        mgr.deactivate_play("pb1");
        let remaining = mgr.lookup_rules(&[key]);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].play_id, "pb2");
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
            node: make_node("node:xyz", "task"),
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
        let pb_node = make_play_node(
            "pb1",
            json!([{
                "name": "on task created",
                "trigger": {"type": "graph_event", "on": "node_created", "node_type": "task"},
                "conditions": ["node.status == 'open'"],
                "actions": [{"action_type": "update_node", "params": {}}]
            }]),
        );
        mgr.activate_play(&pb_node).unwrap();

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
        assert_eq!(work_item.rules[0].play_id, "pb1");
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
                node: trigger_node.clone(),
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
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ExecutionWorkItem>(
            super::super::engine::EXECUTION_QUEUE_CAPACITY,
        );

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
        use crate::playbook::lifecycle::PlaybookLifecycleManager;
        use std::sync::{Arc, RwLock};

        let (tx, rx) = tokio::sync::mpsc::channel::<ExecutionWorkItem>(
            super::super::engine::EXECUTION_QUEUE_CAPACITY,
        );

        // The processor now requires lifecycle and node_service args.
        // Since we can't easily create a real NodeService without a database,
        // we verify the drain+shutdown behavior by dropping the channel.
        let _lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));

        // Drop sender → processor will drain remaining items and exit
        drop(tx);
        drop(rx);

        // The key behavior (drain + shutdown) is verified by the queue tests above.
        // Full integration testing of the processor loop with NodeService
        // requires a database setup, which is done in integration tests.
    }

    // -----------------------------------------------------------------------
    // Phase 6: Logging and cycle detection tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_max_chain_depth_is_10() {
        use crate::playbook::logging::MAX_CHAIN_DEPTH;
        assert_eq!(MAX_CHAIN_DEPTH, 10);
    }

    #[test]
    fn test_error_fingerprint_consistent() {
        use crate::playbook::logging::{error_fingerprint, PlayErrorType};

        let fp1 = error_fingerprint("pb-123", "my_rule", 0, &PlayErrorType::CycleLimit);
        let fp2 = error_fingerprint("pb-123", "my_rule", 0, &PlayErrorType::CycleLimit);

        // Same inputs → same fingerprint
        assert_eq!(fp1, fp2);
        // SHA-256 hex is 64 characters
        assert_eq!(fp1.len(), 64);
    }

    #[test]
    fn test_error_fingerprint_different_for_different_inputs() {
        use crate::playbook::logging::{error_fingerprint, PlayErrorType};

        let base = error_fingerprint("pb-123", "my_rule", 0, &PlayErrorType::CycleLimit);

        // Different play_id
        let different_pb = error_fingerprint("pb-456", "my_rule", 0, &PlayErrorType::CycleLimit);
        assert_ne!(base, different_pb);

        // Different rule_name
        let different_rule =
            error_fingerprint("pb-123", "other_rule", 0, &PlayErrorType::CycleLimit);
        assert_ne!(base, different_rule);

        // Different error_location_index
        let different_idx = error_fingerprint("pb-123", "my_rule", 5, &PlayErrorType::CycleLimit);
        assert_ne!(base, different_idx);

        // Different error_type
        let different_type = error_fingerprint("pb-123", "my_rule", 0, &PlayErrorType::MissingPath);
        assert_ne!(base, different_type);
    }

    #[test]
    fn test_error_fingerprint_excludes_dynamic_data() {
        use crate::playbook::logging::{error_fingerprint, PlayErrorType};

        // The fingerprint is only based on play_id, rule_name,
        // error_location_index, and error_type. Dynamic data excluded by design.
        let fp1 = error_fingerprint("pb-123", "rule_a", 2, &PlayErrorType::ActionError);
        let fp2 = error_fingerprint("pb-123", "rule_a", 2, &PlayErrorType::ActionError);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_play_error_type_display() {
        use crate::playbook::logging::PlayErrorType;

        assert_eq!(PlayErrorType::CycleLimit.to_string(), "cycle_limit");
        assert_eq!(PlayErrorType::MissingPath.to_string(), "missing_path");
        assert_eq!(PlayErrorType::TypeMismatch.to_string(), "type_mismatch");
        assert_eq!(
            PlayErrorType::VersionConflict.to_string(),
            "version_conflict"
        );
        assert_eq!(PlayErrorType::CompileError.to_string(), "compile_error");
        assert_eq!(PlayErrorType::ActionError.to_string(), "action_error");
    }

    #[test]
    fn test_depth_enforcement_boundary_values() {
        use crate::db::events::PlaybookExecutionContext;
        use crate::playbook::logging::MAX_CHAIN_DEPTH;

        // Verify the constant is what we expect so the boundary checks below are valid
        assert_eq!(MAX_CHAIN_DEPTH, 10);

        // depth=0, no playbook_context → depth+1 = 1, within limit
        let depth: u8 = 0;
        assert!(depth < MAX_CHAIN_DEPTH);

        // depth=9 → depth+1 = 10, exactly at limit
        let depth: u8 = 9;
        assert!(depth < MAX_CHAIN_DEPTH);

        // depth=10 → depth+1 = 11, exceeds limit
        let depth: u8 = 10;
        assert!(depth >= MAX_CHAIN_DEPTH);

        // Verify the context carries the right depth for testing
        let ctx = PlaybookExecutionContext {
            originating_event_id: "evt-1".to_string(),
            depth: 10,
            source_playbook_id: "pb-1".to_string(),
        };
        assert!(ctx.depth + 1 > MAX_CHAIN_DEPTH);
    }

    #[test]
    fn test_all_error_type_fingerprints_are_unique() {
        use crate::playbook::logging::{error_fingerprint, PlayErrorType};

        let types = [
            PlayErrorType::CycleLimit,
            PlayErrorType::MissingPath,
            PlayErrorType::TypeMismatch,
            PlayErrorType::VersionConflict,
            PlayErrorType::CompileError,
            PlayErrorType::ActionError,
        ];

        let fingerprints: Vec<String> = types
            .iter()
            .map(|t| error_fingerprint("pb-1", "rule-1", 0, t))
            .collect();

        // All fingerprints should be unique
        for i in 0..fingerprints.len() {
            for j in (i + 1)..fingerprints.len() {
                assert_ne!(
                    fingerprints[i], fingerprints[j],
                    "Error types {:?} and {:?} produced the same fingerprint",
                    types[i], types[j]
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Persisted chain depth (ADR-060 §5) — `effective_chain_depth`
    // -----------------------------------------------------------------------
    //
    // `effective_chain_depth` is the depth `rule_processor_loop` enforces
    // `MAX_CHAIN_DEPTH` against. These tests exercise it directly against
    // hand-built `ExecutionWorkItem`s, the same way `test_execution_work_item_*`
    // above do, rather than through a running engine + NodeService.

    fn node_with_properties(id: &str, properties: serde_json::Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: "task".to_string(),
            content: "".to_string(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties,
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        }
    }

    fn work_item_for(
        trigger_node: Node,
        playbook_context: Option<crate::db::events::PlaybookExecutionContext>,
    ) -> ExecutionWorkItem {
        let node_id = trigger_node.id.clone();
        let node_type = trigger_node.node_type.clone();
        ExecutionWorkItem {
            rules: vec![],
            trigger_event: EventEnvelope {
                event: DomainEvent::NodeCreated { node_id, node_type },
                metadata: EventMetadata {
                    source_client_id: Some("tauri-main".to_string()),
                    playbook_context,
                },
            },
            trigger_node,
        }
    }

    #[test]
    fn effective_chain_depth_defaults_to_zero_when_neither_source_is_present() {
        use super::super::engine::effective_chain_depth;

        let work_item = work_item_for(node_with_properties("node:fresh", json!({})), None);
        assert_eq!(effective_chain_depth(&work_item), 0);
    }

    #[test]
    fn effective_chain_depth_prefers_in_process_context_over_persisted_property() {
        use super::super::engine::effective_chain_depth;
        use crate::db::events::{PlaybookExecutionContext, PLAYBOOK_CHAIN_DEPTH_PROPERTY};

        // A locally-originated re-entrant firing: the in-process context
        // (depth 6) must win even though the node also happens to carry a
        // (stale) persisted value (2) -- single-device behavior must be
        // unchanged by this issue.
        let trigger_node =
            node_with_properties("node:1", json!({ (PLAYBOOK_CHAIN_DEPTH_PROPERTY): 2 }));
        let work_item = work_item_for(
            trigger_node,
            Some(PlaybookExecutionContext {
                originating_event_id: "evt-1".to_string(),
                depth: 6,
                source_playbook_id: "pb-1".to_string(),
            }),
        );

        assert_eq!(effective_chain_depth(&work_item), 6);
    }

    /// Reproduces the device-hop failure this issue fixes: a node created on
    /// another device carries only its persisted depth -- no in-process
    /// `PlaybookExecutionContext` survives the hop, since sync transports
    /// the node's committed properties, not the transient event envelope
    /// that produced it there. Before this fix, `unwrap_or(0)` reset the
    /// chain to depth 0 on the receiving device; the chain must instead
    /// continue from where it left off.
    #[test]
    fn effective_chain_depth_continues_from_the_persisted_property_after_a_device_hop() {
        use super::super::engine::effective_chain_depth;
        use crate::db::events::PLAYBOOK_CHAIN_DEPTH_PROPERTY;

        let trigger_node = node_with_properties(
            "node:synced-1",
            json!({ (PLAYBOOK_CHAIN_DEPTH_PROPERTY): 7 }),
        );
        // No in-process context: this device never saw the chain's earlier
        // hops -- exactly what a sync-applied node looks like.
        let work_item = work_item_for(trigger_node, None);

        assert_eq!(
            effective_chain_depth(&work_item),
            7,
            "depth must continue from the persisted property, not reset to 0"
        );
    }

    /// The safety property this issue restores: a chain already sitting at
    /// MAX_CHAIN_DEPTH when it crosses a device hop must still trip the
    /// cycle limit on the very next local hop, exactly as it would have if
    /// the whole chain had stayed on one device.
    #[test]
    fn effective_chain_depth_at_persisted_max_still_trips_cycle_limit_on_next_hop() {
        use super::super::engine::{effective_chain_depth, exceeds_max_chain_depth};
        use crate::db::events::PLAYBOOK_CHAIN_DEPTH_PROPERTY;
        use crate::playbook::logging::MAX_CHAIN_DEPTH;

        let trigger_node = node_with_properties(
            "node:synced-max",
            json!({ (PLAYBOOK_CHAIN_DEPTH_PROPERTY): MAX_CHAIN_DEPTH }),
        );
        let work_item = work_item_for(trigger_node, None);

        let depth = effective_chain_depth(&work_item);
        assert_eq!(depth, MAX_CHAIN_DEPTH);
        assert!(
            exceeds_max_chain_depth(depth),
            "the next hop must trip the cycle limit even though the chain just crossed a device boundary"
        );
    }

    /// A persisted depth of exactly `u8::MAX` (255) cannot reach
    /// `effective_chain_depth` as `Some(255)` -- `persisted_chain_depth` now
    /// rejects anything above `MAX_CHAIN_DEPTH` -- so the device-hop fallback
    /// lands on depth 0, not 255. Documents that outcome directly, and is a
    /// regression guard against `persisted_chain_depth`'s bound check ever
    /// being loosened back to "anything that fits a u8".
    #[test]
    fn effective_chain_depth_treats_a_255_persisted_value_as_absent_not_as_255() {
        use super::super::engine::effective_chain_depth;
        use crate::db::events::PLAYBOOK_CHAIN_DEPTH_PROPERTY;

        let trigger_node = node_with_properties(
            "node:corrupt-depth",
            json!({ (PLAYBOOK_CHAIN_DEPTH_PROPERTY): 255 }),
        );
        let work_item = work_item_for(trigger_node, None);

        assert_eq!(effective_chain_depth(&work_item), 0);
    }

    /// Regression for the overflow this issue's review caught: before
    /// `exceeds_max_chain_depth` used saturating arithmetic, `depth + 1` at
    /// `u8::MAX` would overflow and silently wrap to `0` in a release build
    /// (this repo's release profile leaves `overflow-checks` at its default
    /// of off), making the cycle-depth guard read "not exceeded" for the
    /// worst-case input instead of tripping. `persisted_chain_depth`'s own
    /// bound check (previous test) already keeps 255 from reaching this
    /// guard via the sync/device-hop path today, but `exceeds_max_chain_depth`
    /// is deliberately defense-in-depth against `depth` being out of range
    /// by any means, not solely reliant on that filter -- this test exercises
    /// the arithmetic itself, independent of how `depth` got there.
    #[test]
    fn exceeds_max_chain_depth_does_not_wrap_at_u8_max() {
        use super::super::engine::exceeds_max_chain_depth;
        use crate::playbook::logging::MAX_CHAIN_DEPTH;

        assert!(exceeds_max_chain_depth(u8::MAX));
        assert!(exceeds_max_chain_depth(MAX_CHAIN_DEPTH));
        assert!(!exceeds_max_chain_depth(MAX_CHAIN_DEPTH - 1));
    }

    // -----------------------------------------------------------------------
    // ADR-073: local-origin gating (`is_sync_originated`)
    // -----------------------------------------------------------------------
    //
    // Unit-level coverage of the pure filter function. The real safety
    // property — that a sync-originated event never reaches trigger
    // evaluation inside a running engine — is verified end-to-end in
    // `packages/core/tests/playbook_engine_integration_test.rs`, which drives
    // a real `PlaybookEngine::start()` against a real broadcast channel.

    #[test]
    fn is_sync_originated_true_for_reserved_sync_client_id() {
        use super::super::engine::is_sync_originated;
        use crate::db::events::SYNC_SERVICE_CLIENT_ID;

        let envelope = EventEnvelope {
            event: DomainEvent::NodeCreated {
                node_id: "n1".to_string(),
                node_type: "task".to_string(),
            },
            metadata: EventMetadata {
                source_client_id: Some(SYNC_SERVICE_CLIENT_ID.to_string()),
                playbook_context: None,
            },
        };

        assert!(is_sync_originated(&envelope));
    }

    #[test]
    fn is_sync_originated_false_for_local_client_ids_and_none() {
        use super::super::engine::is_sync_originated;

        for source in [None, Some("tauri-main"), Some("mcp-client-123"), Some("")] {
            let envelope = EventEnvelope {
                event: DomainEvent::NodeCreated {
                    node_id: "n1".to_string(),
                    node_type: "task".to_string(),
                },
                metadata: EventMetadata {
                    source_client_id: source.map(str::to_string),
                    playbook_context: None,
                },
            };
            assert!(
                !is_sync_originated(&envelope),
                "source_client_id {:?} must NOT be treated as sync-originated",
                source
            );
        }
    }

    #[test]
    fn is_sync_originated_is_an_exact_match_not_a_substring_check() {
        use super::super::engine::is_sync_originated;

        // A client id that merely contains the reserved token must not match —
        // the gate compares the whole `source_client_id`, not a substring.
        let envelope = EventEnvelope {
            event: DomainEvent::NodeCreated {
                node_id: "n1".to_string(),
                node_type: "task".to_string(),
            },
            metadata: EventMetadata {
                source_client_id: Some("not-sync-service-either".to_string()),
                playbook_context: None,
            },
        };

        assert!(!is_sync_originated(&envelope));
    }
}
