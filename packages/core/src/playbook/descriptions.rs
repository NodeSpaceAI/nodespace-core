//! Save-time checks on the descriptions a play's rules carry (ADR-090 §1).
//!
//! A rule, each condition and each action carry a `description` written by
//! the play's author. Decoding already requires one to be present; these
//! checks reject one that says nothing, and one that was written for content
//! the write has since changed:
//!
//! - **Blank:** empty or whitespace only.
//! - **Stale:** on a write that changes a play's rules, a component whose
//!   content changed keeps the description it was stored with. A rule is
//!   matched to the stored rule with the same `name`, and its conditions and
//!   actions by index within it. A component with no match is new, and needs
//!   only a description that is not blank.
//!
//! The checks read the typed rules and nothing else, so they need no store.
//! Their messages are read by agents repairing a rejected write: each names
//! the rule, the component and its 1-based index, and what to change.

use crate::playbook::types::{Action, RuleDefinition};

/// The part of a rule a description belongs to. Indexes are 0-based here and
/// printed 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescribedComponent {
    Rule,
    Condition(usize),
    Action(usize),
}

/// What is wrong with a description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptionProblem {
    /// Empty or whitespace only.
    Blank,
    /// The component's content changed and its description did not.
    Stale,
}

/// One description a write is rejected for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptionError {
    /// The `name` of the rule the description is in.
    pub rule: String,
    pub component: DescribedComponent,
    pub problem: DescriptionProblem,
}

impl std::fmt::Display for DescriptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rule `{}`", self.rule)?;
        match self.component {
            DescribedComponent::Rule => {}
            DescribedComponent::Condition(index) => write!(f, ", condition {}", index + 1)?,
            DescribedComponent::Action(index) => write!(f, ", action {}", index + 1)?,
        }
        let detail = match (self.problem, self.component) {
            (DescriptionProblem::Blank, DescribedComponent::Rule) => {
                "its description is blank. Say what the rule does"
            }
            (DescriptionProblem::Blank, DescribedComponent::Condition(_)) => {
                "its description is blank. Say what must hold"
            }
            (DescriptionProblem::Blank, DescribedComponent::Action(_)) => {
                "its description is blank. Say what the action does"
            }
            (DescriptionProblem::Stale, DescribedComponent::Rule) => {
                "its trigger or class changed and its description didn't. Rewrite the \
                 description to say what the rule does now"
            }
            (DescriptionProblem::Stale, DescribedComponent::Condition(_)) => {
                "its expression changed and its description didn't. Rewrite the description \
                 to say what must hold now"
            }
            (DescriptionProblem::Stale, DescribedComponent::Action(_)) => {
                "its action changed and its description didn't. Rewrite the description to \
                 say what the action does now"
            }
        };
        write!(f, ": {detail}")
    }
}

/// Check the descriptions of `rules`, the rules a write would store.
///
/// `stored` is the rules the play holds now, for a write that changes them;
/// `None` for a new play, where nothing can be stale. Every problem is
/// reported, in rule order.
pub fn check_descriptions(
    rules: &[RuleDefinition],
    stored: Option<&[RuleDefinition]>,
) -> Result<(), Vec<DescriptionError>> {
    let mut errors = Vec::new();
    for rule in rules {
        let previous =
            stored.and_then(|stored| stored.iter().find(|previous| previous.name == rule.name));
        let mut check = |component, description: &str, stale: bool| {
            let problem = if description.trim().is_empty() {
                DescriptionProblem::Blank
            } else if stale {
                DescriptionProblem::Stale
            } else {
                return;
            };
            errors.push(DescriptionError {
                rule: rule.name.clone(),
                component,
                problem,
            });
        };

        check(
            DescribedComponent::Rule,
            &rule.description,
            previous.is_some_and(|previous| {
                (previous.trigger != rule.trigger || previous.class != rule.class)
                    && previous.description == rule.description
            }),
        );
        for (index, condition) in rule.conditions.iter().enumerate() {
            let before = previous.and_then(|previous| previous.conditions.get(index));
            check(
                DescribedComponent::Condition(index),
                &condition.description,
                before.is_some_and(|before| {
                    before.expr != condition.expr && before.description == condition.description
                }),
            );
        }
        for (index, action) in rule.actions.iter().enumerate() {
            let before = previous.and_then(|previous| previous.actions.get(index));
            check(
                DescribedComponent::Action(index),
                action.description(),
                before.is_some_and(|before| {
                    action_content(before) != action_content(action)
                        && before.description() == action.description()
                }),
            );
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// What an action's description describes: everything but the description.
fn action_content(
    action: &Action,
) -> (
    crate::playbook::types::ActionType,
    serde_json::Value,
    Option<&str>,
) {
    (
        action.action_type(),
        action.params_value(),
        action.for_each(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn rules(value: Value) -> Vec<RuleDefinition> {
        serde_json::from_value(value).expect("the test's rules decode")
    }

    /// One rule named `complete parent`: a trigger, two conditions and two
    /// actions, each with its own description.
    fn stored() -> Value {
        json!([{
            "name": "complete parent",
            "description": "Mark a task done when its sub-tasks are",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "select": { "target_type": "task" },
                "property_key": "task.status"
            },
            "conditions": [
                { "expr": "node.status == 'done'", "description": "The task is done" },
                { "expr": "node.priority == 'high'", "description": "The task is high priority" }
            ],
            "actions": [
                {
                    "action_type": "update_node",
                    "description": "Mark the parent done",
                    "params": { "node_id": "{trigger.node.child_of.id}", "properties": { "status": "done" } }
                },
                {
                    "action_type": "add_relationship",
                    "description": "Link each sub-task to the parent's cycle",
                    "for_each": "trigger.node.has_child",
                    "params": { "source_id": "{item.id}", "relationship_type": "cycle", "target_id": "{trigger.node.id}" }
                }
            ]
        }])
    }

    /// The problems a write of `incoming` over [`stored`] is rejected for.
    fn problems(incoming: &Value) -> Vec<(DescribedComponent, DescriptionProblem)> {
        match check_descriptions(&rules(incoming.clone()), Some(&rules(stored()))) {
            Ok(()) => Vec::new(),
            Err(errors) => errors
                .into_iter()
                .map(|e| (e.component, e.problem))
                .collect(),
        }
    }

    #[test]
    fn unchanged_rules_and_reworded_descriptions_pass() {
        assert_eq!(problems(&stored()), []);

        let mut reworded = stored();
        reworded[0]["description"] = json!("Roll completion up to the parent");
        reworded[0]["conditions"][0]["description"] = json!("Its status is done");
        reworded[0]["actions"][1]["description"] = json!("Put every sub-task in the cycle");
        assert_eq!(problems(&reworded), []);
    }

    #[test]
    fn a_blank_description_is_rejected_on_each_component() {
        for blank in ["", "   ", "\n\t"] {
            let mut incoming = stored();
            incoming[0]["description"] = json!(blank);
            incoming[0]["conditions"][1]["description"] = json!(blank);
            incoming[0]["actions"][0]["description"] = json!(blank);
            assert_eq!(
                problems(&incoming),
                [
                    (DescribedComponent::Rule, DescriptionProblem::Blank),
                    (DescribedComponent::Condition(1), DescriptionProblem::Blank),
                    (DescribedComponent::Action(0), DescriptionProblem::Blank),
                ],
                "{blank:?}"
            );
        }

        // A new play has nothing stored, and is held to the same rule.
        let mut new_play = stored();
        new_play[0]["actions"][1]["description"] = json!(" ");
        let errors = check_descriptions(&rules(new_play), None).unwrap_err();
        assert_eq!(errors[0].component, DescribedComponent::Action(1));
        assert_eq!(errors[0].problem, DescriptionProblem::Blank);
    }

    #[test]
    fn a_changed_expression_must_change_its_description() {
        let mut incoming = stored();
        incoming[0]["conditions"][1]["expr"] = json!("node.priority == 'urgent'");
        assert_eq!(
            problems(&incoming),
            [(DescribedComponent::Condition(1), DescriptionProblem::Stale)]
        );

        incoming[0]["conditions"][1]["description"] = json!("The task is urgent");
        assert_eq!(problems(&incoming), []);
    }

    #[test]
    fn a_changed_action_must_change_its_description() {
        type Change = fn(&mut Value);
        let changes: [(&str, Change); 3] = [
            ("params", |action| {
                action["params"]["properties"]["status"] = json!("cancelled")
            }),
            ("for_each", |action| {
                action["for_each"] = json!("trigger.node.has_child")
            }),
            ("action_type", |action| {
                *action = json!({
                    "action_type": "reject",
                    "description": action["description"],
                    "params": { "message": "no" }
                })
            }),
        ];
        for (what, change) in changes {
            let mut incoming = stored();
            change(&mut incoming[0]["actions"][0]);
            assert_eq!(
                problems(&incoming),
                [(DescribedComponent::Action(0), DescriptionProblem::Stale)],
                "a changed {what}"
            );

            incoming[0]["actions"][0]["description"] = json!("Something else now");
            assert_eq!(problems(&incoming), [], "a changed {what}, redescribed");
        }
    }

    #[test]
    fn a_changed_trigger_or_class_must_change_the_rules_description() {
        let mut retriggered = stored();
        retriggered[0]["trigger"]["property_key"] = json!("task.priority");
        assert_eq!(
            problems(&retriggered),
            [(DescribedComponent::Rule, DescriptionProblem::Stale)]
        );

        let mut reclassed = stored();
        reclassed[0]["class"] = json!("invariant");
        assert_eq!(
            problems(&reclassed),
            [(DescribedComponent::Rule, DescriptionProblem::Stale)]
        );

        reclassed[0]["description"] = json!("Refuse the write unless the sub-tasks are done");
        assert_eq!(problems(&reclassed), []);

        // A rule's description answers for its trigger and class only: a
        // changed condition or action is that component's to redescribe.
        let mut condition_only = stored();
        condition_only[0]["conditions"][0] =
            json!({ "expr": "node.status == 'cancelled'", "description": "The task is cancelled" });
        assert_eq!(problems(&condition_only), []);
    }

    #[test]
    fn a_new_component_needs_only_a_description_that_is_not_blank() {
        let mut incoming = stored();
        incoming[0]["conditions"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "expr": "node.status != 'open'", "description": "The task is done" }));
        incoming[0]["actions"].as_array_mut().unwrap().push(json!({
            "action_type": "reject",
            "description": "Mark the parent done",
            "params": { "message": "no" }
        }));
        assert_eq!(problems(&incoming), []);
    }

    /// A rule is matched by `name`, so a renamed rule is a new one: nothing
    /// in it is compared with what the old name held.
    #[test]
    fn a_renamed_rule_is_new() {
        let mut incoming = stored();
        incoming[0]["name"] = json!("roll up");
        incoming[0]["trigger"]["property_key"] = json!("task.priority");
        incoming[0]["conditions"][0]["expr"] = json!("node.status == 'cancelled'");
        incoming[0]["actions"][0]["params"]["properties"]["status"] = json!("cancelled");
        assert_eq!(problems(&incoming), []);
    }

    /// Rules are matched by name wherever they sit, so reordering them is not
    /// a change to any of them.
    #[test]
    fn rules_are_matched_by_name_not_position() {
        let mut other = stored()[0].clone();
        other["name"] = json!("other");
        other["conditions"][0]["expr"] = json!("node.status == 'open'");
        let stored = json!([stored()[0], other]);
        let reordered = json!([stored[1], stored[0]]);
        assert_eq!(
            check_descriptions(&rules(reordered), Some(&rules(stored))),
            Ok(())
        );
    }

    #[test]
    fn the_message_names_the_rule_the_component_and_its_index() {
        let mut incoming = stored();
        incoming[0]["conditions"][1]["expr"] = json!("node.priority == 'urgent'");
        incoming[0]["actions"][0]["description"] = json!("");
        incoming[0]["class"] = json!("invariant");
        let messages: Vec<String> = check_descriptions(&rules(incoming), Some(&rules(stored())))
            .unwrap_err()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            messages,
            [
                "rule `complete parent`: its trigger or class changed and its description \
                 didn't. Rewrite the description to say what the rule does now",
                "rule `complete parent`, condition 2: its expression changed and its \
                 description didn't. Rewrite the description to say what must hold now",
                "rule `complete parent`, action 1: its description is blank. Say what the \
                 action does",
            ]
        );
    }
}
