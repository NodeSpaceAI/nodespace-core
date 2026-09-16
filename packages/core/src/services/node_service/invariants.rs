//! Synchronous, pre-commit invariant-rule dispatch (ADR-060 §1).
//!
//! Called from `crud.rs`'s `create_node_in_tx` after the node row lands on
//! the open transaction but before `with_transaction` commits it. Looks up
//! any `RuleClass::Invariant` rules whose trigger matches the node just
//! created, evaluates their conditions, and runs their actions through the
//! transaction-scoped executor (`playbook::actions::execute_actions_in_tx`).
//! An action failure returns `Err`, which propagates out through
//! `create_node_in_tx` and `with_transaction`'s `?`, rolling back the whole
//! transaction — the node this dispatch ran for was never durably created.
//! A `reject` action (ADR-060 §2, #2642) firing is reported as a distinct
//! `NodeServiceError::PlayRuleRejected`, not the generic
//! `InvariantRuleFailed` every other action failure produces — see
//! `execute_matched_invariant_rules_in_tx`, the shared execution core every
//! synchronous dispatch path (this one and, per #2643, `update_node`'s) runs
//! through.
//!
//! Deliberately NOT part of `playbook::engine`'s post-commit `mpsc` queue:
//! that queue is async and after-the-fact by construction, exactly what an
//! invariant rule must not be (see `playbook::engine`'s module doc for why
//! the two paths are architecturally separate). Symmetrically,
//! `playbook::engine::PlaybookEngine::handle_event` filters
//! `RuleClass::Invariant` OUT of what it enqueues onto that mpsc queue for a
//! local event — dispatch here has already fully handled it (or, for a
//! sync-applied event, `handle_event`'s repair-and-log branch has) — so an
//! invariant rule is never run twice.
//!
//! One scope note for `create_node_with_parent`/`create_node_with_parent_in_tx`
//! composed creates specifically: this dispatch runs from inside
//! `create_node_in_tx`, called BEFORE the parent/collection edge is added in
//! that composition. A condition that inspects the trigger node's
//! parent/collection relationship therefore always sees it as absent at
//! evaluation time, for every node created this way — not a bug (nothing
//! about "same-graph scope", ADR-060 §2, promises a not-yet-created edge is
//! visible), but worth naming: it is a real constraint on what an invariant
//! rule can usefully condition on for a node created with a parent.

use super::*;
use crate::playbook::types::{NodeEventType, RuleClass, TriggerKey};

impl NodeService {
    /// Dispatch invariant rules matching `node`'s creation, inside `tx`.
    ///
    /// A no-op (returns `Ok(())` immediately) in three cases, all by design:
    /// - This write is not local — `self.client_id` is the reserved sync
    ///   client id, so the node arrived via sync and its invariant effect (if
    ///   any) already committed atomically on the originating device. Running
    ///   this again here would be exactly the re-execution ADR-060 §1
    ///   requires never happens on a receiving device.
    /// - No lifecycle handle has been injected (`set_playbook_lifecycle` was
    ///   never called) — a bare `NodeService` built without a play engine,
    ///   e.g. most unit tests and standalone tools. No invariant rules can
    ///   exist without an engine to have activated them.
    /// - No active rule's trigger matches this node's creation.
    pub(crate) async fn dispatch_invariant_rules_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        node: &Node,
    ) -> Result<(), NodeServiceError> {
        if self.client_id.as_deref() == Some(crate::db::events::SYNC_SERVICE_CLIENT_ID) {
            return Ok(());
        }

        let Some(lifecycle) = self.playbook_lifecycle() else {
            return Ok(());
        };

        let key = TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: node.node_type.clone(),
            property_key: None,
        };

        let matched = {
            let lm = lifecycle.read().unwrap_or_else(|e| e.into_inner());
            lm.lookup_rules(std::slice::from_ref(&key))
        };
        if matched.is_empty() {
            return Ok(());
        }

        let event = DomainEvent::NodeCreated {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
        };

        self.execute_matched_invariant_rules_in_tx(tx, node, &event, matched)
            .await
    }

    /// Shared execution core for every synchronous invariant-rule dispatch
    /// path (today: [`Self::dispatch_invariant_rules_in_tx`] for
    /// `create_node`/`create_node_in_tx`; ADR-060 §2's `update_node` wiring
    /// reuses this identically). Filters `matched` down to `RuleClass::Invariant`
    /// rules, evaluates each one's conditions, and executes its actions inside
    /// `tx`. Rules already come back sorted by (play_id, rule_index) — the
    /// same stable order the reactive path uses (`lookup_rules`'s own doc).
    ///
    /// Keeping exactly one execution loop for every synchronous caller — rather
    /// than each caller re-implementing "evaluate conditions, run actions,
    /// interpret the result" — is deliberate: the #2642/#2643 reject-action
    /// distinction below (an `ActionError::Rejected` becomes
    /// `NodeServiceError::PlayRuleRejected`, everything else becomes
    /// `NodeServiceError::InvariantRuleFailed`) needs to hold identically for
    /// every trigger kind an invariant rule can run from, and a single shared
    /// implementation is what makes that true by construction instead of by
    /// convention.
    pub(crate) async fn execute_matched_invariant_rules_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        node: &Node,
        event: &DomainEvent,
        matched: Vec<crate::playbook::types::OrderedRuleRef>,
    ) -> Result<(), NodeServiceError> {
        for rule_ref in matched {
            if rule_ref.rule.class != RuleClass::Invariant {
                continue;
            }

            let scoped = Arc::new(self.clone());
            let mut resolver = crate::playbook::graph_resolver::GraphResolver::new(scoped.clone());
            let condition_result = crate::playbook::cel::evaluate_conditions(
                &rule_ref.rule.conditions,
                node,
                event,
                Some(&mut resolver),
            )
            .await;

            match condition_result {
                crate::playbook::cel::ConditionResult::Pass => {}
                crate::playbook::cel::ConditionResult::Fail { .. } => continue,
            }

            // Invariant rules are non-chaining depth-1 by save-time
            // eligibility (`playbook::validation`), so there is no
            // in-process chain depth to propagate — this is always the
            // start of a fresh (bounded) execution, never a continuation of
            // a reactive chain. `depth: 0` matches that: nodes this
            // dispatch's actions create/update are stamped depth 0, which
            // is correct for the reactive engine's OWN cycle tracking should
            // one of those nodes also match a reactive trigger once this
            // transaction's buffered events flush after commit.
            let execution_context = crate::db::events::PlaybookExecutionContext {
                originating_event_id: uuid::Uuid::new_v4().to_string(),
                depth: 0,
                source_playbook_id: rule_ref.play_id.clone(),
            };

            let result = crate::playbook::actions::execute_actions_in_tx(
                &rule_ref.rule.actions,
                node,
                event,
                &scoped,
                tx,
                execution_context,
            )
            .await;

            if let crate::playbook::actions::ActionResult::Failed(err) = result {
                // A `reject` action (ADR-060 §2, #2642) firing is a distinct
                // outcome from every other action failure: the rule did
                // exactly what it was authored to do, deliberately vetoing
                // this write, rather than malfunctioning. Surfaced as its own
                // error variant — modeled on `VersionConflict` — so a caller
                // (and eventually the frontend) can tell "your write was
                // rejected by a rule" apart from "a rule's action errored."
                if let crate::playbook::actions::ActionError::Rejected { message, .. } = &err {
                    return Err(NodeServiceError::play_rule_rejected(
                        node.id.clone(),
                        rule_ref.play_id.clone(),
                        rule_ref.rule.name.clone(),
                        message.clone(),
                    ));
                }
                return Err(NodeServiceError::invariant_rule_failed(
                    rule_ref.play_id.clone(),
                    rule_ref.rule.name.clone(),
                    err.to_string(),
                ));
            }
        }

        Ok(())
    }
}
