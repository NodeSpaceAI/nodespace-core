//! Play Engine
//!
//! The top-level engine that subscribes to domain events, matches them against
//! the trigger index, and enqueues work items for the RuleProcessor.
//!
//! Phases wired through this module:
//! - Phase 1: subscribe to events, manage lifecycle, match triggers
//! - Phase 2: ExecutionQueue (bounded mpsc) + RuleProcessor (single tokio task)
//! - Phase 3: CEL condition evaluation (via `cel.rs`)
//! - Phase 4: Action execution (via `actions.rs`)
//! - Phase 5: CronRunner spawn and shutdown (via `cron_runner.rs`)
//! - Phase 6: Cycle detection (max depth 10)
//! - Phase 7: Save-time validation before play activation

use crate::db::events::{persisted_chain_depth, DomainEvent, EventEnvelope};
use crate::playbook::lifecycle::{trigger_keys_for_event, PlaybookLifecycleManager};
use crate::playbook::types::*;
use crate::services::NodeService;
use std::sync::{Arc, RwLock};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

/// Bounded capacity for the ExecutionQueue.
///
/// Backpressure prevents unbounded memory growth if the engine falls behind
/// (e.g., desktop wakes from sleep and many events arrive at once).
pub(crate) const EXECUTION_QUEUE_CAPACITY: usize = 1024;

/// The play engine — subscribes to domain events and manages play lifecycle.
///
/// Runs in-process alongside NodeService. Subscribes to the domain event
/// broadcast channel that `core` owns; other subscribers (e.g.
/// `desktop-app`'s frontend relay) subscribe independently, and this engine
/// has no dependency on or awareness of them, per the `core`/`desktop-app`
/// crate boundary.
pub struct PlaybookEngine {
    /// Lifecycle manager behind RwLock for concurrent access.
    /// Read: event subscriber (frequent). Write: lifecycle ops (infrequent).
    ///
    /// `cron_runner.rs`'s poll loop recovers from a poisoned lock (`into_inner()`)
    /// rather than panicking, since a panic there would permanently kill the
    /// 60-second cron loop. The `.expect(...)` call sites in this file still
    /// propagate a poisoned lock as a panic — deliberately deferred, not an
    /// oversight, since those run on the event-subscriber/lifecycle-mutation
    /// paths rather than an unattended background loop.
    lifecycle: Arc<RwLock<PlaybookLifecycleManager>>,
    /// NodeService for fetching play nodes and (later) executing actions.
    node_service: Arc<NodeService>,
    /// Set when a `refresh_ancestor_cache` call failed and the cache is
    /// therefore of unknown staleness (ADR-078).
    ///
    /// A failed refresh keeps the previous cache, which is the right immediate
    /// choice — dropping every Play's subtype matching is worse than serving
    /// slightly stale ancestry. But without this flag nothing ever retries: a
    /// transient DB error at startup would leave subtype trigger matching
    /// quietly degraded for the whole process lifetime, with no error and no
    /// log after the first warning. `handle_event` clears it by re-refreshing
    /// before the next event is matched, which bounds the degraded window to
    /// one event rather than the process.
    ancestry_dirty: Arc<std::sync::atomic::AtomicBool>,
}

impl PlaybookEngine {
    /// Create a new PlaybookEngine.
    ///
    /// Does NOT start the event subscription — call `start()` to begin processing.
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self {
            lifecycle: Arc::new(RwLock::new(PlaybookLifecycleManager::new())),
            node_service,
            ancestry_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Initialize the engine: load active plays, build indexes, start event loop.
    ///
    /// Follows the startup sequence from the spec:
    /// 1. Subscribe to event broadcast channel FIRST (to avoid race)
    /// 2. Query all active play nodes
    /// 3. Parse rules, build TriggerIndex and CronRegistry
    /// 4. Start the RuleProcessor task (drains the ExecutionQueue)
    /// 5. Start the CronRunner task (60-second polling for scheduled triggers)
    /// 6. Begin processing events
    ///
    /// The `shutdown_rx` watch channel signals graceful shutdown when it receives `true`.
    pub async fn start(
        self: Arc<Self>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        // Step 1: Subscribe FIRST to avoid missing events between load and subscribe
        let mut rx = self.node_service.subscribe_to_events();
        info!("Play engine subscribed to event channel");

        // Step 2-3: Load active plays and build indexes
        self.load_active_plays().await?;

        // Step 4: Create ExecutionQueue and spawn RuleProcessor
        let (queue_tx, queue_rx) = mpsc::channel::<ExecutionWorkItem>(EXECUTION_QUEUE_CAPACITY);
        let processor_handle = tokio::spawn(rule_processor_loop(
            queue_rx,
            Arc::clone(&self.lifecycle),
            Arc::clone(&self.node_service),
        ));

        // Step 5: Spawn CronRunner (60-second polling loop for scheduled triggers)
        let cron_handle = tokio::spawn(crate::playbook::cron_runner::cron_runner_loop(
            Arc::clone(&self.lifecycle),
            Arc::clone(&self.node_service),
            queue_tx.clone(),
            shutdown_rx.clone(),
        ));

        info!("Play engine started, processing events...");

        // Step 6: Process events
        let result = loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(envelope) => {
                            self.handle_event(envelope, &queue_tx).await;
                        }
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            warn!("Play engine lagged, missed {} events", count);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Event channel closed, play engine shutting down");
                            break Ok(());
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Play engine received shutdown signal");
                        break Ok(());
                    }
                }
            }
        };

        // Shutdown: drop the sender to signal the processor to drain and exit.
        // The CronRunner exits via the shutdown_rx watch (already signalled).
        drop(queue_tx);
        if let Err(e) = processor_handle.await {
            error!("RuleProcessor task panicked: {:?}", e);
        }
        if let Err(e) = cron_handle.await {
            error!("CronRunner task panicked: {:?}", e);
        }

        result
    }

    /// Report each error from a failed `validate_play` call.
    ///
    /// Shared by `load_active_plays` / `handle_play_created` /
    /// `handle_play_updated`, which all re-run the same save-time validation
    /// and need identical error reporting.
    ///
    /// Every error is reported independently, so two structurally different
    /// problems on the same play both surface with their own message. This
    /// used to require fingerprint disambiguation, because errors were
    /// written to the graph as deduplicated `playbook_log` nodes and two
    /// errors sharing a fingerprint collapsed into one, silently discarding
    /// the second's text. Diagnostics now go to tracing, where each event
    /// stands alone, so no dedup identity is computed at all.
    fn log_validation_errors(
        &self,
        play_id: &str,
        errors: &[crate::playbook::validation::PlayValidationError],
    ) {
        for err in errors {
            warn!(
                play_id = %play_id,
                location = %err.location(),
                kind = %err.kind(),
                error = %err,
                "Play validation error"
            );
        }
    }

    /// Load all active play nodes from the database and activate them.
    ///
    /// Phase 7: save-time validation (belt-and-suspenders — primary gate is
    /// in `NodeService`), same as `handle_play_created`/`handle_play_updated`.
    /// A persisted play must not bypass this just because it is reaching
    /// activation via the startup load path rather than the create/update
    /// path: ADR-060 is explicitly about multi-device sync, so a play row
    /// here may have been written by another device (or an earlier build)
    /// whose validation rules differ, and the store layer does not
    /// re-validate business rules on replicated writes. Without this check,
    /// e.g. a `reject` action (ADR-060 §2) on a `Reactive` rule — invalid,
    /// but only caught at save time — would reactivate unvalidated on every
    /// restart and then, on first trigger, disable its *entire* play (not
    /// just the offending rule): the async reactive dispatch loop treats any
    /// `ActionResult::Failed` the same, and `execute_reject` always fails.
    /// Rebuild the lifecycle manager's `extends` ancestry cache (ADR-078).
    ///
    /// The manager itself has no store access, so the walk happens here and
    /// the result is handed over. Every extending type gets an entry; an
    /// unextended type gets none, and `ancestors_of` treats an absent entry as
    /// "just itself" — so this is empty, and costs nothing, until a schema
    /// declares `extends`.
    ///
    /// Rebuilt wholesale rather than diffed: `extends` edits are rare,
    /// administrative operations, and the map holds one entry per extending
    /// type.
    /// Build the CEL evaluation scope for a rule firing on a node (ADR-078).
    ///
    /// A rule registered against a base type evaluates its conditions at that
    /// type's scope, so it sees the field set and enum vocabulary it was
    /// authored against whatever concrete subtype fired it. Returns `None`
    /// when there is nothing to scope — the node is already the registered
    /// type, or the trigger is not type-scoped — which is every rule until
    /// something declares `extends`.
    pub(crate) async fn cel_scope_for(
        node_service: &Arc<NodeService>,
        rule: &ParsedRule,
        node: &crate::models::Node,
    ) -> Option<crate::playbook::cel::CelScope> {
        let scope_type = match &rule.trigger {
            ParsedTrigger::GraphEvent { node_type, .. } => node_type,
            ParsedTrigger::Scheduled { node_type, .. } => node_type,
        };

        // A node of exactly the registered type reads natively; nothing to
        // project or resolve.
        if scope_type == &node.node_type || scope_type == "*" {
            return None;
        }

        let chain = node_service.resolve_type_chain(scope_type).await.ok()?;
        let scope_fields = node_service.resolve_field_owners(scope_type).await.ok()?.0;
        // The node's OWN chain, not the scope's. Reading the scope's ancestry
        // would skip every bucket between the node and the reading scope — on
        // `bug → ticket → workitem` read at `workitem`, the `ticket` bucket
        // would never be opened. `resolve_field_owners` already computes this
        // chain as its third element, so taking it costs nothing.
        let (node_fields, _, node_chain) = node_service
            .resolve_field_owners(&node.node_type)
            .await
            .ok()?;

        Some(crate::playbook::cel::CelScope {
            scope_type: scope_type.clone(),
            node_chain,
            chain,
            scope_fields,
            node_fields,
        })
    }

    /// Rebuild the `extends` ancestry cache from the store (ADR-078).
    ///
    /// On failure the previous cache stays in place and `ancestry_dirty` is
    /// set, so the next event retries rather than the process running on
    /// unknown-staleness ancestry forever — see the field's own comment.
    pub(crate) async fn refresh_ancestor_cache(&self) {
        use std::sync::atomic::Ordering;

        let parent_map = match self.node_service.store().get_extends_parent_map().await {
            Ok(map) => map,
            Err(e) => {
                // A failed refresh leaves the previous cache in place. Stale
                // ancestry can only mean a base-scoped Play misses a
                // newly-extending type until the next schema write, which is
                // preferable to dropping every Play's subtype matching.
                //
                // Marked dirty so this is a bounded window rather than a
                // permanent one: `handle_event` retries before matching the
                // next event.
                self.ancestry_dirty.store(true, Ordering::Relaxed);
                warn!("Failed to refresh extends ancestry cache (will retry on next event): {e}");
                return;
            }
        };

        let cache: std::collections::HashMap<String, Vec<String>> = parent_map
            .keys()
            .map(|child| {
                let lookup = |id: &str| parent_map.get(id).cloned();
                (
                    child.clone(),
                    crate::schema::extends_chain::resolve_ancestor_chain(child, &lookup),
                )
            })
            .collect();

        let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
        lifecycle.set_ancestor_cache(cache);
        drop(lifecycle);

        // Cleared only after the new cache is actually installed, so a refresh
        // that failed and one that succeeded are never confused.
        self.ancestry_dirty.store(false, Ordering::Relaxed);
    }

    async fn load_active_plays(&self) -> anyhow::Result<()> {
        // Ancestry must be warm before any event is dispatched, or a
        // base-scoped Play would silently miss subtype events until the first
        // schema write of the process.
        self.refresh_ancestor_cache().await;

        let nodes = self
            .node_service
            .query_nodes_by_type("play", Some("active"))
            .await?;

        let mut loaded = 0;
        for node in &nodes {
            let parsed_rules = match parse_rules_for_validation(node) {
                Ok(rules) => rules,
                Err(e) => {
                    warn!("Failed to parse play {}: {}", node.id, e);
                    continue;
                }
            };

            if let Err(errors) =
                crate::playbook::validation::validate_play(&parsed_rules, &self.node_service).await
            {
                self.log_validation_errors(&node.id, &errors);
                if crate::playbook::validation::has_genuine_failure(&errors) {
                    warn!(
                        "Play {} failed save-time validation with {} error(s) at load time, \
                         skipping activation",
                        node.id,
                        errors.len()
                    );
                    continue;
                }
                // Every error is SchemaResolutionFailed — inconclusive (a
                // transient DB error), not evidence this play is broken.
                // Unlike the other three call sites gated the same way,
                // this one has no later event to retry validation on: a
                // play skipped here at startup stays un-activated for the
                // rest of the process's life, which is a worse outcome for
                // a play a user already set active in a previous session
                // than optimistically activating it despite the unconfirmed
                // verdict. Falls through to activate below.
                warn!(
                    "Play {} save-time validation was inconclusive ({} resolution failure(s)) \
                     at load time — activating anyway rather than leaving it off for the \
                     process lifetime on an unconfirmed verdict",
                    node.id,
                    errors.len()
                );
            }

            let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
            match lifecycle.activate_play(node) {
                Ok(()) => loaded += 1,
                Err(e) => {
                    warn!("Failed to parse play {}: {}", node.id, e);
                }
            }
        }

        info!(
            "Loaded {} active plays ({} total found)",
            loaded,
            nodes.len()
        );
        Ok(())
    }

    /// Handle a single event from the broadcast channel.
    ///
    /// Performs lifecycle management (detect play/schema CRUD), then trigger
    /// matching. Matched rules are bundled with the pre-fetched trigger node
    /// into an ExecutionWorkItem and sent to the RuleProcessor queue.
    async fn handle_event(
        &self,
        envelope: EventEnvelope,
        queue_tx: &mpsc::Sender<ExecutionWorkItem>,
    ) {
        // A previous refresh failed and left the cache of unknown staleness.
        // Retry before matching this event, so a transient DB error degrades
        // subtype trigger matching for one event rather than for the process
        // lifetime. One relaxed atomic load on the hot path in the common
        // (never-failed) case.
        if self
            .ancestry_dirty
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.refresh_ancestor_cache().await;
        }

        // Lifecycle management: detect play node events
        match &envelope.event {
            DomainEvent::NodeCreated { node_type, node_id } if node_type == "play" => {
                self.handle_play_created(node_id).await;
                return;
            }
            DomainEvent::NodeDeleted { node_type, id } if node_type == "play" => {
                self.handle_play_deleted(id);
                return;
            }
            DomainEvent::NodeUpdated {
                node_type, node_id, ..
            } if node_type == "play" => {
                self.handle_play_updated(node_id).await;
                return;
            }
            // Schema version drift detection
            DomainEvent::NodeUpdated {
                node_type, node_id, ..
            } if node_type == "schema" => {
                self.handle_schema_updated(node_id).await;
                return;
            }
            // A newly created schema may declare `extends`, and a deleted one
            // may remove an edge — neither arrives as NodeUpdated, so the
            // drift hook above would never see them and the ancestry cache
            // would stay stale until some unrelated schema edit. Refresh, but
            // don't return: schema creation/deletion is not itself drift, and
            // a Play may legitimately trigger on it.
            DomainEvent::NodeCreated { node_type, .. } if node_type == "schema" => {
                self.refresh_ancestor_cache().await;
            }
            DomainEvent::NodeDeleted { node_type, .. } if node_type == "schema" => {
                self.refresh_ancestor_cache().await;
            }
            _ => {}
        }

        // ADR-073: local-origin gating, the hard-safety part of this issue.
        //
        // Play lifecycle management above (install/uninstall/enable/disable,
        // schema-drift detection) is intentionally NOT gated — a Play node
        // authored on another device is data like any other and must still
        // be installed locally once synced in. What IS gated is trigger
        // evaluation against a mutation: an event whose origin is the
        // sync-apply path (tagged with `SYNC_SERVICE_CLIENT_ID`, per
        // ADR-027's existing `source_client_id` convention) is structurally
        // excluded here, before any `TriggerKey` lookup, so a reconnecting
        // device can never replay its sync backlog as live rule firings.
        // Scheduled (cron) triggers are unaffected — `CronRunner` scans local
        // graph state directly and never reaches this event-driven path.
        //
        // This does not make `RuleClass::Invariant` rules sync-safe: their
        // fail-closed guarantee depends on ADR-060 §2/§7 mechanisms that are
        // out of scope here. It only prevents reactive (and invariant) rules
        // from firing against a sync-replayed event at all.
        if is_sync_originated(&envelope) {
            debug!(
                node_event = ?trigger_node_id(&envelope.event),
                "Skipping trigger evaluation for sync-originated event"
            );
            // ADR-060 §7: repair-and-log. This IS the mechanism that makes
            // `RuleClass::Invariant` rules sync-safe for a node received
            // already-committed via sync — architecturally separate from the
            // reactive `ExecutionQueue` above (never enqueued there; run
            // inline, here, against the already-committed node), and
            // deliberately still reached even though trigger evaluation for
            // reactive/invariant firing is skipped for this event.
            if let DomainEvent::NodeCreated { node_id, node_type } = &envelope.event {
                self.dispatch_invariant_repair(node_id, node_type).await;
            }
            return;
        }

        // Trigger matching for non-lifecycle, locally-originated events
        let keys = trigger_keys_for_event(&envelope.event);
        if keys.is_empty() {
            return;
        }

        // ADR-060 §1: `RuleClass::Invariant` rules must NEVER reach this
        // queue. For a local write, `dispatch_invariant_rules_in_tx`
        // (`services/node_service/invariants.rs`) already ran the invariant
        // rule synchronously, pre-commit, inside the SAME transaction as
        // this event's own write — by the time this event reaches the
        // engine at all, the invariant rule has already fully executed,
        // fail-closed, with rollback protection. If an invariant rule were
        // left in `matched_rules` here, `rule_processor_loop` would run it a
        // SECOND time, asynchronously and fail-open (disable-the-play, no
        // rollback) — silently double-executing every invariant rule on
        // every local create it matches, and turning a transient failure of
        // that spurious second run into "the play (and its invariant) is now
        // silently disabled for every future node," exactly the failure mode
        // ADR-060 exists to prevent. Mirrors the same filter
        // `dispatch_invariant_repair` already applies for the sync-apply
        // path (via `RuleClass::Invariant` in its own lookup).
        let matched_rules: Vec<OrderedRuleRef> = {
            let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
            lifecycle
                .lookup_rules(&keys)
                .into_iter()
                .filter(|r| r.rule.class != RuleClass::Invariant)
                .collect()
        };

        if matched_rules.is_empty() {
            return;
        }

        debug!(
            "Event matched {} rules: {:?}",
            matched_rules.len(),
            matched_rules
                .iter()
                .map(|r| format!("{}[{}]", r.play_id, r.rule_index))
                .collect::<Vec<_>>()
        );

        // Pre-fetch the trigger node
        let trigger_node_id = match trigger_node_id(&envelope.event) {
            Some(id) => id,
            None => return,
        };

        let trigger_node = match self.node_service.get_node(trigger_node_id).await {
            Ok(Some(node)) => node,
            Ok(None) => {
                debug!(
                    "Trigger node {} not found (deleted before processing?), skipping",
                    trigger_node_id
                );
                return;
            }
            Err(e) => {
                error!("Failed to fetch trigger node {}: {}", trigger_node_id, e);
                return;
            }
        };

        // Enqueue the work item
        let work_item = ExecutionWorkItem {
            rules: matched_rules,
            trigger_event: envelope,
            trigger_node,
        };

        if let Err(e) = queue_tx.try_send(work_item) {
            match e {
                mpsc::error::TrySendError::Full(_) => {
                    warn!(
                        "ExecutionQueue full (capacity {}), dropping work item",
                        EXECUTION_QUEUE_CAPACITY
                    );
                }
                mpsc::error::TrySendError::Closed(_) => {
                    debug!("ExecutionQueue closed, engine shutting down");
                }
            }
        }
    }

    /// Repair-and-log for a node received via sync (ADR-060 §7).
    ///
    /// `node_id`/`node_type` are the just-applied `NodeCreated` event's own
    /// fields (already known from the envelope — no need to re-derive them).
    /// Looks up active `RuleClass::Invariant` rules matching this node's
    /// creation, and for each whose condition STILL passes against the node
    /// as it now stands (i.e. the effect the rule would have applied is
    /// absent — the node violates an invariant this device holds), runs the
    /// rule's actions as an ordinary write (no transaction to join — the
    /// node already committed on the originating device) and records a
    /// logs the repair. A rule whose condition now fails is left alone: the
    /// node already carries the required effect (applied by whichever device
    /// originated it, or by an earlier repair — this device's own or one
    /// that already synced in), so re-running would be redundant at best.
    ///
    /// Best-effort: a failure fetching the node or evaluating/executing one
    /// rule is logged and does not block the others, since (unlike the
    /// pre-commit path) there is no write to roll back here — the node is
    /// already durably committed either way.
    async fn dispatch_invariant_repair(&self, node_id: &str, node_type: &str) {
        let key = TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: node_type.to_string(),
            property_key: None,
        };
        let matched = {
            let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
            lifecycle.lookup_rules(&[key])
        };
        let invariant_rules: Vec<_> = matched
            .into_iter()
            .filter(|r| r.rule.class == RuleClass::Invariant)
            .collect();
        if invariant_rules.is_empty() {
            return;
        }

        let node = match self.node_service.get_node(node_id).await {
            Ok(Some(n)) => n,
            Ok(None) => {
                debug!(
                    node_id,
                    "Repair-and-log: node not found (deleted after sync apply?), skipping"
                );
                return;
            }
            Err(e) => {
                error!(node_id, error = %e, "Repair-and-log: failed to fetch node");
                return;
            }
        };

        let event = DomainEvent::NodeCreated {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
        };

        for rule_ref in invariant_rules {
            let cel_scope =
                PlaybookEngine::cel_scope_for(&self.node_service, &rule_ref.rule, &node).await;
            // The resolver reads related nodes at this rule's scope too, so a
            // traversed node is projected exactly as the trigger node is.
            let mut resolver =
                crate::playbook::graph_resolver::GraphResolver::new(Arc::clone(&self.node_service))
                    .with_scope(cel_scope.clone());
            let condition_result = crate::playbook::cel::evaluate_conditions_at_scope(
                &rule_ref.rule.conditions,
                &node,
                &event,
                Some(&mut resolver),
                cel_scope.as_ref(),
            )
            .await;

            let still_violates = matches!(
                condition_result,
                crate::playbook::cel::ConditionResult::Pass
            );
            if !still_violates {
                continue;
            }

            info!(
                node_id = %node.id,
                play_id = %rule_ref.play_id,
                rule = %rule_ref.rule.name,
                "Repairing invariant violation on node received via sync"
            );

            let execution_context = crate::db::events::PlaybookExecutionContext {
                originating_event_id: uuid::Uuid::new_v4().to_string(),
                depth: 0,
                source_playbook_id: rule_ref.play_id.clone(),
            };

            let action_result = crate::playbook::actions::execute_actions(
                &rule_ref.rule.actions,
                &node,
                &event,
                &self.node_service,
                execution_context,
            )
            .await;

            match action_result {
                crate::playbook::actions::ActionResult::Success => {
                    info!(
                        node_id = %node.id,
                        play_id = %rule_ref.play_id,
                        rule = %rule_ref.rule.name,
                        "Repair-and-log: repaired invariant on node received via sync"
                    );
                }
                crate::playbook::actions::ActionResult::Failed(err) => {
                    warn!(
                        node_id = %node.id,
                        play_id = %rule_ref.play_id,
                        rule = %rule_ref.rule.name,
                        error = %err,
                        error_type = "action_error",
                        rule_index = rule_ref.rule_index,
                        "Repair-and-log: invariant repair action failed"
                    );
                }
            }
        }
    }

    /// Handle a new play node being created — validate, then parse and activate.
    ///
    /// Phase 7: runs save-time validation before activation. If validation fails,
    /// the play is disabled and each error is logged.
    async fn handle_play_created(&self, node_id: &str) {
        match self.node_service.get_node(node_id).await {
            Ok(Some(node)) if node.lifecycle_status == "active" => {
                // Parse rules first for validation
                let parsed_rules = match parse_rules_for_validation(&node) {
                    Ok(rules) => rules,
                    Err(e) => {
                        warn!(
                            play_id = %node_id,
                            error_type = "compile_error",
                            error = %e,
                            "Failed to parse play rules for validation"
                        );
                        let mut lifecycle =
                            self.lifecycle.write().expect("lifecycle lock poisoned");
                        lifecycle.disable_play(node_id);
                        return;
                    }
                };

                // Phase 7: Save-time validation (belt-and-suspenders — primary gate is in NodeService)
                if let Err(errors) =
                    crate::playbook::validation::validate_play(&parsed_rules, &self.node_service)
                        .await
                {
                    self.log_validation_errors(node_id, &errors);
                    if crate::playbook::validation::has_genuine_failure(&errors) {
                        warn!(
                            "Play {} failed save-time validation with {} error(s)",
                            node_id,
                            errors.len()
                        );
                        // Disable the play — do not activate
                        let mut lifecycle =
                            self.lifecycle.write().expect("lifecycle lock poisoned");
                        lifecycle.disable_play(node_id);
                        return;
                    }
                    // Every error is SchemaResolutionFailed — validation
                    // could not reach a definitive verdict (a transient DB
                    // error), not evidence this play is broken. Returning
                    // here without activating would NOT "leave state
                    // unchanged": this play has never been in the lifecycle
                    // manager at all, so skipping activation makes it
                    // invisible to `plays_referencing_schema` and therefore
                    // to every future schema-drift re-validation too — a
                    // permanent ghost, worse than disabling it. Fall
                    // through and activate anyway, same as
                    // `load_active_plays`'s identical reasoning.
                    warn!(
                        "Play {} save-time validation was inconclusive ({} resolution \
                         failure(s)) — activating anyway rather than leaving it invisible to \
                         future re-validation on an unconfirmed verdict",
                        node_id,
                        errors.len()
                    );
                }

                let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
                if let Err(e) = lifecycle.activate_play(&node) {
                    warn!("Failed to activate new play {}: {}", node_id, e);
                }
            }
            Ok(Some(_)) => {
                debug!("New play {} is not active, skipping", node_id);
            }
            Ok(None) => {
                warn!("Play {} not found after NodeCreated event", node_id);
            }
            Err(e) => {
                error!("Failed to fetch play {}: {}", node_id, e);
            }
        }
    }

    /// Handle a play node being deleted — remove from all indexes.
    fn handle_play_deleted(&self, play_id: &str) {
        let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
        lifecycle.deactivate_play(play_id);
    }

    /// Handle a play node being updated — detect status transitions.
    ///
    /// If lifecycle_status changed from disabled→active, re-enable (with validation).
    /// If rules changed, re-parse (with validation).
    /// Phase 7: validates before (re-)activation.
    async fn handle_play_updated(&self, node_id: &str) {
        let node = match self.node_service.get_node(node_id).await {
            Ok(Some(n)) => n,
            Ok(None) => {
                warn!("Play {} not found after NodeUpdated event", node_id);
                return;
            }
            Err(e) => {
                error!("Failed to fetch play {}: {}", node_id, e);
                return;
            }
        };

        // Read current status AND the pre-edit rule set (short lock) — the
        // latter is what ADR-060 §8's warning needs to name (the invariant
        // rule(s) this play carried BEFORE whatever update just landed), not
        // whatever `node` now contains.
        let (current_status, previously_carried_invariant) = {
            let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
            match lifecycle.get_play(node_id) {
                Some(pb) => (
                    Some(pb.status.clone()),
                    crate::playbook::seeded::carries_invariant(&pb.rules),
                ),
                None => (None, false),
            }
        };

        // ADR-060 §8: a seeded play carrying an invariant rule warns
        // explicitly, naming the concrete consequence, on edit OR disable —
        // both branches below reach this before doing anything else, so
        // neither an edit-while-active nor a disable skips it. Only reached
        // when this play was ALREADY active (a fresh first-time activation,
        // `current_status == None`, is installation, not an edit). Best-
        // effort: a warning-log failure must never block the underlying
        // disable/re-activation it describes.
        if current_status == Some(PlayStatus::Active)
            && previously_carried_invariant
            && crate::playbook::seeded::is_seeded_play(&node)
        {
            let old_rule_names = {
                let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
                lifecycle
                    .get_play(node_id)
                    .map(|pb| crate::playbook::seeded::invariant_rule_names(&pb.rules))
                    .unwrap_or_default()
            };
            let action = if node.lifecycle_status == "active" {
                "edited"
            } else {
                "disabled"
            };
            let message =
                crate::playbook::seeded::edit_or_disable_warning(node_id, action, &old_rule_names);
            warn!(
                play_id = %node_id,
                error_type = "seeded_play_warning",
                "{}",
                message
            );
        }

        let needs_activation = matches!(
            (&current_status, node.lifecycle_status.as_str()),
            (Some(PlayStatus::Disabled), "active")
                | (Some(PlayStatus::Active), "active")
                | (None, "active")
        );

        // Active → Non-active: just disable, no validation needed
        if matches!(
            (&current_status, node.lifecycle_status.as_str()),
            (Some(PlayStatus::Active), status) if status != "active"
        ) {
            let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
            lifecycle.disable_play(node_id);
            return;
        }

        if needs_activation {
            // Phase 7: validate before (re-)activation (belt-and-suspenders)
            if let Ok(parsed_rules) = parse_rules_for_validation(&node) {
                if let Err(errors) =
                    crate::playbook::validation::validate_play(&parsed_rules, &self.node_service)
                        .await
                {
                    self.log_validation_errors(node_id, &errors);
                    if crate::playbook::validation::has_genuine_failure(&errors) {
                        warn!(
                            "Play {} failed validation on update with {} error(s)",
                            node_id,
                            errors.len()
                        );
                        let mut lifecycle =
                            self.lifecycle.write().expect("lifecycle lock poisoned");
                        lifecycle.disable_play(node_id);
                        return;
                    }
                    // Every error is SchemaResolutionFailed — inconclusive,
                    // not evidence of a real break. Returning here without
                    // (re-)activating would NOT "leave state unchanged" in
                    // any of the three cases `needs_activation` covers:
                    // - `None -> active` (first-ever activation): the play
                    //   was never in the lifecycle manager, so skipping
                    //   leaves it permanently invisible to future
                    //   schema-drift re-validation — a ghost, same failure
                    //   mode `load_active_plays` guards against.
                    // - `Disabled -> active` (re-enable): same — it stays
                    //   disabled with no future retry, silently ignoring
                    //   the user's re-enable.
                    // - `Active -> active` (edit while running): the OLD,
                    //   pre-edit rules would keep executing under the
                    //   `lifecycle_status: active` node the user just
                    //   edited, silently discarding their change with
                    //   nothing but a log line to show for it.
                    // All three are worse than proceeding on an unconfirmed
                    // verdict, so fall through and (re-)activate with the
                    // new rules anyway, same reasoning as
                    // `handle_play_created`/`load_active_plays`.
                    warn!(
                        "Play {} validation on update was inconclusive ({} resolution \
                         failure(s)) — (re-)activating anyway rather than silently dropping \
                         this update on an unconfirmed verdict",
                        node_id,
                        errors.len()
                    );
                }
            }

            let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
            match &current_status {
                Some(PlayStatus::Disabled) | Some(PlayStatus::Active) => {
                    if let Err(e) = lifecycle.reenable_play(&node) {
                        warn!("Failed to re-enable/update play {}: {}", node_id, e);
                    }
                }
                None => {
                    if let Err(e) = lifecycle.activate_play(&node) {
                        warn!("Failed to activate play {}: {}", node_id, e);
                    }
                }
            }
        }
    }

    /// Handle a schema node being updated — check for version drift.
    async fn handle_schema_updated(&self, schema_node_id: &str) {
        // A schema write may have added, re-targeted or removed an `extends`
        // edge, which changes what a base-scoped Play matches. Rebuild the
        // ancestry cache before the drift check below, so this hook cannot
        // return early (a schema node missing `forNodeType`, say) and leave
        // the cache stale.
        self.refresh_ancestor_cache().await;

        match self.node_service.get_node(schema_node_id).await {
            Ok(Some(node)) => {
                // Extract schema_node_type and version from the schema node
                let schema_node_type = node
                    .properties
                    .get("schema")
                    .and_then(|s| s.get("forNodeType"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let new_version = node
                    .properties
                    .get("schema")
                    .and_then(|s| s.get("schemaVersion"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");

                if schema_node_type.is_empty() {
                    return;
                }

                // Referencing the changed type is not the same as being broken
                // by the change. Disabling every Play that merely mentions it
                // means an additive edit — `add_field_values` adding a status
                // value, blessed by ADR-076 — silently stops core automation
                // that the change provably cannot break.
                //
                // So candidates are re-validated against the NEW schema and
                // only genuinely-broken Plays are disabled. Validation needs
                // store access, so it runs outside the lifecycle lock: gather
                // candidates under a read lock, validate, then take the write
                // lock to disable.
                let candidates = {
                    let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
                    lifecycle
                        .plays_referencing_schema(schema_node_type)
                        .into_iter()
                        .filter_map(|id| lifecycle.rules_for_play(&id).map(|rules| (id, rules)))
                        .collect::<Vec<_>>()
                };

                let mut broken = Vec::new();
                for (play_id, rules) in candidates {
                    if let Err(errors) =
                        crate::playbook::validation::validate_play(&rules, &self.node_service).await
                    {
                        if crate::playbook::validation::has_genuine_failure(&errors) {
                            broken.push((play_id, errors));
                        } else {
                            // Every error is SchemaResolutionFailed —
                            // validation could not reach a definitive
                            // verdict (a transient DB error while
                            // re-resolving the extends chain), not evidence
                            // this play is actually broken by the schema
                            // change. Disabling it here would silently take
                            // a working, unrelated automation offline over
                            // nothing more than a hiccup — precisely the
                            // failure mode `SchemaResolutionFailed` exists
                            // to surface instead of hide. Left running; a
                            // later schema/play write gives re-validation
                            // another chance to reach a real verdict.
                            warn!(
                                "Play {} re-validation after schema '{}' update was \
                                 inconclusive ({} resolution failure(s)) — leaving it active \
                                 rather than disabling on an unconfirmed verdict",
                                play_id,
                                schema_node_type,
                                errors.len()
                            );
                        }
                    }
                }

                if !broken.is_empty() {
                    let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
                    for (play_id, errors) in &broken {
                        // Name the actual errors. This is the one path that
                        // stops automation at runtime, and `validate_play`
                        // checks the whole play against every schema it
                        // references — not just the one that changed — so the
                        // triggering schema is not necessarily the culprit.
                        // Without the errors, the warning would misattribute a
                        // pre-existing break to whichever schema was touched
                        // first.
                        let detail = errors
                            .iter()
                            .map(|e| e.to_string())
                            .collect::<Vec<_>>()
                            .join("; ");
                        warn!(
                            "Schema '{}' updated to version '{}', disabling play {} — its rules \
                             no longer validate: {}",
                            schema_node_type, new_version, play_id, detail
                        );
                        lifecycle.disable_play(play_id);
                    }
                    drop(lifecycle);

                    warn!(
                        "Schema drift: {} plays disabled due to schema '{}' update: {:?}",
                        broken.len(),
                        schema_node_type,
                        broken.iter().map(|(id, _)| id).collect::<Vec<_>>()
                    );
                    // Each disabled play is logged by the caller
                }
            }
            Ok(None) => {
                debug!(
                    "Schema node {} not found after update event",
                    schema_node_id
                );
            }
            Err(e) => {
                error!("Failed to fetch schema node {}: {}", schema_node_id, e);
            }
        }
    }

    /// Get a snapshot of the lifecycle manager for inspection/testing.
    pub fn lifecycle(&self) -> &Arc<RwLock<PlaybookLifecycleManager>> {
        &self.lifecycle
    }

    /// The `extends` ancestry cache's staleness flag (ADR-078), for a host to
    /// hand to `NodeService` (see `NodeService::set_playbook_ancestry_dirty`)
    /// the same way it hands over `lifecycle()`. Lets an out-of-band consumer
    /// with no other access to `PlaybookEngine` — e.g. `get_workflow_state`,
    /// which only ever receives `lifecycle` and `node_service` — detect the
    /// one class of `ancestor_cache` staleness that's both unbounded in
    /// duration and cheaply detectable: a failed refresh that hasn't yet been
    /// retried. See the field's own doc comment for why nothing shorter-lived
    /// (ordinary event-processing lag) is covered.
    pub fn ancestry_dirty(&self) -> &Arc<std::sync::atomic::AtomicBool> {
        &self.ancestry_dirty
    }
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Parse a play node's rules into `Vec<Arc<ParsedRule>>` for validation.
///
/// This is used by the engine before activation to feed the validator.
/// It mirrors the parsing in `PlaybookLifecycleManager::activate_play`.
fn parse_rules_for_validation(
    node: &crate::models::Node,
) -> Result<Vec<Arc<ParsedRule>>, PlayParseError> {
    let rule_defs = parse_rules_from_properties(&node.properties)?;
    let mut parsed = Vec::with_capacity(rule_defs.len());
    for def in &rule_defs {
        parsed.push(Arc::new(parse_rule(def)?));
    }
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// RuleProcessor
// ---------------------------------------------------------------------------

/// The RuleProcessor loop — single tokio task draining the ExecutionQueue.
///
/// Sequential processing eliminates concurrency concerns: no two rules
/// execute simultaneously, no race between condition evaluation and action
/// execution, no concurrent modifications to the same node.
///
/// Enforces cycle detection: when `exceeds_max_chain_depth` reports the next
/// execution would pass `MAX_CHAIN_DEPTH`, the work item is skipped,
/// offending plays are disabled, and the limit breach is logged.
pub(crate) async fn rule_processor_loop(
    mut rx: mpsc::Receiver<ExecutionWorkItem>,
    lifecycle: Arc<RwLock<PlaybookLifecycleManager>>,
    node_service: Arc<NodeService>,
) {
    info!("RuleProcessor started, waiting for work items...");

    while let Some(work_item) = rx.recv().await {
        let depth = effective_chain_depth(&work_item);

        // Cycle detection: if the next execution would exceed MAX_CHAIN_DEPTH,
        // skip this work item and disable offending plays.
        if exceeds_max_chain_depth(depth) {
            warn!(
                "Cycle limit reached (depth {}), skipping work item for node {}",
                depth, work_item.trigger_node.id,
            );

            for rule_ref in &work_item.rules {
                // Disable the play that would have fired
                {
                    let mut lm = lifecycle.write().expect("lifecycle lock poisoned");
                    lm.disable_play(&rule_ref.play_id);
                }

                warn!(
                    play_id = %rule_ref.play_id,
                    rule = %rule_ref.rule.name,
                    rule_index = rule_ref.rule_index,
                    trigger_node_id = %work_item.trigger_node.id,
                    error_type = "cycle_limit",
                    max_chain_depth = MAX_CHAIN_DEPTH,
                    "Cycle depth limit exceeded; play disabled"
                );
            }

            continue;
        }

        debug!(
            "RuleProcessor received work item: {} rules for node {} (type: {}, depth: {})",
            work_item.rules.len(),
            work_item.trigger_node.id,
            work_item.trigger_node.node_type,
            depth,
        );

        // One resolver for every rule on this work item: all of them resolve
        // paths from the same trigger node, so a per-rule resolver discarded a
        // cache that the next rule was about to ask the same questions of.
        //
        // Two things make the longer-lived cache safe, and the second is the
        // load-bearing one. Entries are scoped to the root they were resolved
        // from, so no rule can be served another node's answer. And stale reads
        // are not a concern even though actions run inside this loop: every rule
        // already evaluates against the same pre-fetched `work_item.trigger_node`,
        // so a work item is a snapshot by construction. A rule's own mutations
        // re-enter through the event queue as a fresh work item — with a fresh
        // node, and a fresh resolver.
        let mut resolver =
            crate::playbook::graph_resolver::GraphResolver::new(Arc::clone(&node_service));

        // Process each matched rule in order
        for rule_ref in &work_item.rules {
            debug!(
                "Processing rule '{}' from play {} (index {})",
                rule_ref.rule.name, rule_ref.play_id, rule_ref.rule_index,
            );

            // Evaluate at the rule's registered trigger scope (ADR-078), so
            // a Play on a base type sees that type's fields and vocabulary
            // whatever concrete subtype fired it.
            let cel_scope = PlaybookEngine::cel_scope_for(
                &node_service,
                &rule_ref.rule,
                &work_item.trigger_node,
            )
            .await;
            // Each rule in this work item carries its own registered scope, so
            // the shared resolver is re-pointed per rule rather than per item.
            resolver.set_scope(cel_scope.clone());
            let condition_result = crate::playbook::cel::evaluate_conditions_at_scope(
                &rule_ref.rule.conditions,
                &work_item.trigger_node,
                &work_item.trigger_event.event,
                Some(&mut resolver),
                cel_scope.as_ref(),
            )
            .await;

            match condition_result {
                crate::playbook::cel::ConditionResult::Pass => {
                    debug!(
                        "Rule '{}' (play {}) conditions passed",
                        rule_ref.rule.name, rule_ref.play_id,
                    );
                }
                crate::playbook::cel::ConditionResult::Fail { condition_index } => {
                    debug!(
                        "Rule '{}' (play {}) skipped: condition[{}] evaluated to false",
                        rule_ref.rule.name, rule_ref.play_id, condition_index,
                    );
                    continue;
                }
            }

            // Build execution context for cycle detection.
            // Actions will emit events tagged with this context so the engine
            // can track chain depth on re-entrant event processing.
            let execution_context = crate::db::events::PlaybookExecutionContext {
                originating_event_id: work_item
                    .trigger_event
                    .metadata
                    .playbook_context
                    .as_ref()
                    .map(|ctx| ctx.originating_event_id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                // Saturating, not `depth + 1`: `exceeds_max_chain_depth` above
                // already guarantees `depth <= MAX_CHAIN_DEPTH` here, so this
                // never actually saturates in practice -- the same
                // defense-in-depth reasoning as that guard applies (see its
                // doc), not a claim that this path is otherwise reachable.
                depth: depth.saturating_add(1),
                source_playbook_id: rule_ref.play_id.clone(),
            };

            // Execute actions
            let action_result = crate::playbook::actions::execute_actions(
                &rule_ref.rule.actions,
                &work_item.trigger_node,
                &work_item.trigger_event.event,
                &node_service,
                execution_context,
            )
            .await;

            match action_result {
                crate::playbook::actions::ActionResult::Success => {
                    info!(
                        "Rule '{}' (play {}) executed successfully",
                        rule_ref.rule.name, rule_ref.play_id,
                    );
                }
                crate::playbook::actions::ActionResult::Failed(err) => {
                    // A `reject` action (ADR-060 §2) is only meaningful on an
                    // `Invariant`-class rule — `validate_reject_action_class`
                    // rejects it at save time on a `Reactive` rule. Reaching
                    // it here at all means that gate was bypassed (e.g. a
                    // play loaded from disk without re-validation, or an
                    // already-active play whose class changed underneath
                    // it) — and unlike every other action, `execute_reject`
                    // ALWAYS errors when reached, so this rule would disable
                    // its whole play on its very first trigger. Logged with
                    // a distinct, actionable message rather than the generic
                    // "action failed" one, so this misconfiguration reads as
                    // what it is instead of looking like a transient bug.
                    let log_message =
                        if let crate::playbook::actions::ActionError::Rejected { message, .. } =
                            &err
                        {
                            warn!(
                                "Rule '{}' (play {}) uses a 'reject' action on a Reactive-class \
                             rule -- reject is only meaningful on Invariant rules and should \
                             have been caught at save time. Disabling the play. Reject's own \
                             message was: {}",
                                rule_ref.rule.name, rule_ref.play_id, message,
                            );
                            format!(
                                "'reject' action on a Reactive-class rule (should have been \
                             caught at save time): {}",
                                message
                            )
                        } else {
                            warn!(
                                "Rule '{}' (play {}) action failed: {}",
                                rule_ref.rule.name, rule_ref.play_id, err,
                            );
                            format!("Action execution failed: {}", err)
                        };
                    // Disable the play on action failure (per spec)
                    {
                        let mut lm = lifecycle.write().expect("lifecycle lock poisoned");
                        lm.disable_play(&rule_ref.play_id);
                    }
                    warn!(
                        play_id = %rule_ref.play_id,
                        rule = %rule_ref.rule.name,
                        rule_index = rule_ref.rule_index,
                        trigger_node_id = %work_item.trigger_node.id,
                        error_type = "action_error",
                        "{}",
                        log_message
                    );
                    // Skip remaining rules from this play in the current batch
                    continue;
                }
            }
        }
    }

    info!("RuleProcessor shutting down (queue closed)");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// ADR-073 local-origin gate: true when `envelope` was applied via the
/// sync-apply path rather than originating on this device.
///
/// Deliberately a denylist (match the reserved sync origin), not an
/// allowlist of known local client ids: local writes arrive tagged with many
/// different client ids (Tauri windows, MCP clients, CLI sessions, or none at
/// all), and enumerating them would be both impractical and the wrong
/// direction to fail in — an unrecognized *local* id would be silently
/// dropped instead of a genuinely sync-applied one slipping through. This
/// mirrors the existing `push_forward_allowed` precedent in
/// `services::node_service` (`push_excluded_origin`), which excludes by
/// origin match for the same reason.
pub(crate) fn is_sync_originated(envelope: &EventEnvelope) -> bool {
    envelope.metadata.source_client_id.as_deref() == Some(crate::db::events::SYNC_SERVICE_CLIENT_ID)
}

/// The chain depth to enforce `MAX_CHAIN_DEPTH` against for `work_item`
/// (ADR-060 §5).
///
/// Prefers the in-process `PlaybookExecutionContext` carried on the
/// triggering event — present whenever this hop's mutation was produced by
/// this same running process, which covers every same-device chain today
/// (ADR-073 currently excludes sync-applied events from trigger evaluation
/// entirely, so a work item never reaches here with a foreign in-process
/// context). Falls back to the depth persisted on the trigger node's own
/// properties (`persisted_chain_depth`) when that in-process context is
/// absent — the shape a node takes once it has crossed a device boundary via
/// sync: sync transports the node's committed `properties`, not the
/// transient `EventMetadata` that accompanied its creation elsewhere, so the
/// persisted property is the only surviving record of how deep the chain
/// already was.
///
/// Defaults to 0 when neither is present: a node never touched by a play
/// action, or the first hop of a fresh chain. Also the fallback for a
/// persisted value `persisted_chain_depth` rejects as out of range —
/// indistinguishable here from a node that was never touched, which is a
/// known, accepted limitation of a best-effort persisted signal with no
/// write protection (see `persisted_chain_depth`'s doc).
///
/// The in-process context is bounded to `0..=MAX_CHAIN_DEPTH` by
/// construction (only ever assigned `depth.saturating_add(1)` after
/// `exceeds_max_chain_depth` already passed), but the persisted property is
/// not similarly trustworthy — it is ordinary node data any
/// `create_node`/`update_node` caller can write — so `persisted_chain_depth`
/// is bounded explicitly against `MAX_CHAIN_DEPTH` here rather than trusting
/// the stored value's own range.
pub(crate) fn effective_chain_depth(work_item: &ExecutionWorkItem) -> u8 {
    work_item
        .trigger_event
        .metadata
        .playbook_context
        .as_ref()
        .map(|ctx| ctx.depth)
        .unwrap_or_else(|| {
            persisted_chain_depth(&work_item.trigger_node.properties, MAX_CHAIN_DEPTH).unwrap_or(0)
        })
}

/// Whether the next hop of a chain currently at `depth` would exceed
/// `MAX_CHAIN_DEPTH` (ADR-060 §5).
///
/// Uses saturating arithmetic rather than `depth + 1 > MAX_CHAIN_DEPTH`:
/// defense-in-depth against `depth` ever being out of range when this runs,
/// regardless of what `effective_chain_depth`'s own bound-checking
/// guarantees today. Unchecked addition at `u8::MAX` overflows and silently
/// wraps to `0` in a release build (this repo's release profile leaves
/// `overflow-checks` at its default of off), which would make an
/// out-of-range depth read as "not exceeded" instead of tripping the guard
/// — the opposite of fail-safe for a cycle-detection limit.
pub(crate) fn exceeds_max_chain_depth(depth: u8) -> bool {
    depth.saturating_add(1) > MAX_CHAIN_DEPTH
}

/// Extract the trigger node ID from a domain event.
///
/// Returns the node_id for events that can trigger play rules.
/// Returns `None` for events that don't carry a node_id relevant to triggers
/// (e.g., RelationshipCreated — those need source node lookup, deferred to Phase 2+).
pub(crate) fn trigger_node_id(event: &DomainEvent) -> Option<&str> {
    match event {
        DomainEvent::NodeCreated { node_id, .. } => Some(node_id.as_str()),
        DomainEvent::NodeUpdated { node_id, .. } => Some(node_id.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod scope_tests {
    //! CEL evaluation at a Play's registered trigger scope (ADR-078).
    //!
    //! These live in-crate rather than in `tests/` because `cel_scope_for` is
    //! `pub(crate)` — the scope is an internal contract between the engine and
    //! the CEL evaluator, not a public API, and widening it purely for a test
    //! would be the wrong trade.

    use super::*;
    use crate::db::SqliteStore;
    use crate::playbook::cel::{evaluate_conditions_at_scope, ConditionResult};
    use crate::playbook::types::{parse_rule, ParsedRule};
    use crate::schema::{handle_create_schema, handle_update_schema};
    use serde_json::json;
    use tempfile::TempDir;

    async fn test_service() -> (Arc<NodeService>, TempDir) {
        let temp_dir = TempDir::new().expect("tempdir creation failed");
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(
            SqliteStore::new(db_path)
                .await
                .expect("SqliteStore init failed"),
        );
        let node_service = Arc::new(
            NodeService::new(&mut store)
                .await
                .expect("NodeService init failed"),
        );
        (node_service, temp_dir)
    }

    /// `ticket.state` is extensible; `bug` extends it and adds `backlog`
    /// mapping to `open`. `bug` also declares its own `severity`.
    async fn seed_chain(svc: &Arc<NodeService>) {
        handle_create_schema(
            svc,
            json!({
                "name": "Ticket",
                "fields": [{
                    "name": "state",
                    "type": "enum",
                    "protection": "user",
                    "indexed": false,
                    "extensible": true,
                    "coreValues": [
                        { "value": "open", "label": "Open" },
                        { "value": "done", "label": "Done" }
                    ]
                }]
            }),
        )
        .await
        .expect("ticket schema creation failed");

        handle_create_schema(
            svc,
            json!({
                "name": "Bug",
                "extends": "ticket",
                "fields": [
                    { "name": "severity", "type": "string", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .expect("bug schema creation failed");

        handle_update_schema(
            svc,
            json!({
                "schema_id": "bug",
                "add_field_values": [{
                    "field": "state",
                    "values": [{ "value": "backlog", "label": "Backlog", "mapsTo": "open" }]
                }]
            }),
        )
        .await
        .expect("extending the inherited enum should succeed");
    }

    fn rule_on(node_type: &str, condition: &str) -> ParsedRule {
        let def = serde_json::from_value(json!({
            "name": "r",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": node_type },
            "conditions": [condition],
            "actions": []
        }))
        .expect("rule definition should parse");
        parse_rule(&def).expect("rule should compile")
    }

    async fn eval(svc: &Arc<NodeService>, rule: &ParsedRule, node: &crate::models::Node) -> bool {
        let scope = PlaybookEngine::cel_scope_for(svc, rule, node).await;
        let event = DomainEvent::NodeCreated {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
        };
        matches!(
            evaluate_conditions_at_scope(&rule.conditions, node, &event, None, scope.as_ref())
                .await,
            ConditionResult::Pass
        )
    }

    async fn make_bug(svc: &Arc<NodeService>, props: serde_json::Value) -> crate::models::Node {
        let id = svc
            .create_node(crate::models::Node::new(
                "bug".to_string(),
                "a bug".to_string(),
                props,
            ))
            .await
            .expect("bug creation failed");
        svc.get_node(&id)
            .await
            .expect("get_node failed")
            .expect("node should exist")
    }

    #[tokio::test]
    async fn base_scoped_condition_matches_through_maps_to() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;
        let node = make_bug(&svc, json!({ "state": "backlog" })).await;

        // A Play registered on the BASE type, written against the base's
        // vocabulary, firing on a subtype instance storing an extended value.
        let rule = rule_on("ticket", "node.state == 'open'");
        assert!(
            eval(&svc, &rule, &node).await,
            "a ticket-scoped `state == open` should match a bug storing `backlog`"
        );
    }

    #[tokio::test]
    async fn base_scoped_condition_does_not_see_the_extended_value() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;
        let node = make_bug(&svc, json!({ "state": "backlog" })).await;

        // The other half: a base-scoped condition must not be able to depend
        // on vocabulary its author never knew existed.
        let rule = rule_on("ticket", "node.state == 'backlog'");
        assert!(
            !eval(&svc, &rule, &node).await,
            "a ticket-scoped condition must not match the raw extended value"
        );
    }

    #[tokio::test]
    async fn native_scoped_condition_sees_the_raw_value() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;
        let node = make_bug(&svc, json!({ "state": "backlog" })).await;

        let rule = rule_on("bug", "node.state == 'backlog'");
        assert!(
            eval(&svc, &rule, &node).await,
            "a bug-scoped condition reads the stored value unresolved"
        );
    }

    #[tokio::test]
    async fn base_scoped_condition_cannot_see_a_subtypes_own_field() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;
        let node = make_bug(&svc, json!({ "state": "open", "severity": "high" })).await;

        let sees_base = rule_on("ticket", "node.state == 'open'");
        assert!(
            eval(&svc, &sees_base, &node).await,
            "the base's own field is visible at base scope"
        );

        // Projection: a Play on the base behaves identically whether it fired
        // on a plain ticket or a subtype.
        let sees_subtype = rule_on("ticket", "has(node.severity)");
        assert!(
            !eval(&svc, &sees_subtype, &node).await,
            "the subtype's own field must be absent at base scope"
        );
    }

    #[tokio::test]
    async fn scope_projection_preserves_core_keys_and_plain_strings() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;
        let node = make_bug(&svc, json!({ "state": "open", "severity": "high" })).await;

        // `resolve_value_at_scope` returns None for every string that is not a
        // declared enum value at the reading scope — which includes `id`,
        // `content`, `node_type`. The `field_is_enum` guard is what keeps
        // them; deleting it as "redundant" would silently strip these from
        // every base-scoped Play, and this is what would fail.
        for condition in [
            "node.id != ''",
            "node.content != ''",
            "node.node_type == 'bug'",
        ] {
            let rule = rule_on("ticket", condition);
            assert!(
                eval(&svc, &rule, &node).await,
                "core key must survive scope projection: {condition}"
            );
        }
    }

    #[tokio::test]
    async fn an_unextended_type_gets_no_scope_at_all() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;
        let node = make_bug(&svc, json!({ "state": "open" })).await;

        // A rule registered against the node's own type needs no projection or
        // resolution, so the engine short-circuits before touching the store.
        let rule = rule_on("bug", "node.state == 'open'");
        assert!(
            PlaybookEngine::cel_scope_for(&svc, &rule, &node)
                .await
                .is_none(),
            "a rule on the node's own type resolves no scope"
        );
    }

    #[tokio::test]
    async fn a_mid_chain_bucket_is_read_on_a_three_level_chain() {
        let (svc, _tmp) = test_service().await;

        // workitem <- ticket <- bug. The field `state` is declared by
        // `workitem`, inherited by both, and MATERIALIZED onto `ticket` when
        // ticket extends its vocabulary — so an instance stores it in the
        // `ticket` bucket: neither the node's own bucket nor the reading
        // scope's.
        handle_create_schema(
            &svc,
            json!({
                "name": "Workitem",
                "fields": [{
                    "name": "state",
                    "type": "enum",
                    "protection": "user",
                    "indexed": false,
                    "extensible": true,
                    "coreValues": [{ "value": "open", "label": "Open" }]
                }]
            }),
        )
        .await
        .expect("workitem schema creation failed");
        handle_create_schema(
            &svc,
            json!({ "name": "Ticket", "extends": "workitem", "fields": [] }),
        )
        .await
        .expect("ticket schema creation failed");
        handle_update_schema(
            &svc,
            json!({
                "schema_id": "ticket",
                "add_field_values": [{
                    "field": "state",
                    "values": [{ "value": "triage", "label": "Triage", "mapsTo": "open" }]
                }]
            }),
        )
        .await
        .expect("extending the inherited enum should succeed");
        handle_create_schema(
            &svc,
            json!({ "name": "Bug", "extends": "ticket", "fields": [] }),
        )
        .await
        .expect("bug schema creation failed");

        let id = svc
            .create_node(crate::models::Node::new(
                "bug".to_string(),
                "a bug".to_string(),
                json!({ "state": "triage" }),
            ))
            .await
            .expect("bug creation failed");
        let node = svc
            .get_node(&id)
            .await
            .expect("get_node failed")
            .expect("node should exist");

        // Read at the ROOT scope, two levels above the node. Building the
        // node's view from the reading scope's ancestry yields
        // ["bug", "workitem"] and never opens `ticket` — where the value
        // actually lives — so the condition sees nothing.
        let rule = rule_on("workitem", "node.state == 'open'");
        assert!(
            eval(&svc, &rule, &node).await,
            "a mid-chain bucket must be read on a 3-level chain; node properties were {:?}",
            node.properties
        );
    }

    /// A related node reached by traversal must be read at the SAME scope the
    /// trigger node is (ADR-078: "every read surface is affected; none can be
    /// left alone").
    ///
    /// The trigger node's own projection has been covered since `extends`
    /// landed, but a node arriving through `GraphResolver::enrich_context` was
    /// still built with `node_to_cel_value` — the node's-own-type view. For a
    /// subtype child that reads the wrong bucket entirely: `state` is declared
    /// by `ticket`, so a `bug` instance stores it at `properties.ticket.state`,
    /// and a view built at `["bug"]` alone never opens `ticket`.
    ///
    /// Note this needs no extended vocabulary to fail — the child below stores
    /// a plain, inherited `done`. Bucket projection is the broad requirement;
    /// `maps_to` translation (the next test) is the narrower one that rides on
    /// top of it.
    #[tokio::test]
    async fn a_related_subtype_node_is_read_at_the_reading_scope() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;

        let parent = make_bug(&svc, json!({ "state": "open" })).await;
        let child = make_bug(&svc, json!({ "state": "done" })).await;
        svc.create_relationship(&parent.id, "has_child", &child.id, json!({}))
            .await
            .expect("relationship creation failed");

        // Read the child THROUGH the relationship, at `ticket` scope.
        let rule = rule_on("ticket", "node.has_child.all(c, c.state == 'done')");
        let scope = PlaybookEngine::cel_scope_for(&svc, &rule, &parent).await;
        let mut resolver = crate::playbook::graph_resolver::GraphResolver::new(Arc::clone(&svc))
            .with_scope(scope.clone());
        let event = DomainEvent::NodeCreated {
            node_id: parent.id.clone(),
            node_type: parent.node_type.clone(),
        };
        let passed = matches!(
            evaluate_conditions_at_scope(
                &rule.conditions,
                &parent,
                &event,
                Some(&mut resolver),
                scope.as_ref(),
            )
            .await,
            ConditionResult::Pass
        );

        assert!(
            passed,
            "a related subtype node must be projected to the reading scope; \
             child properties were {:?}",
            child.properties
        );
    }

    /// The narrower half: a related node whose stored value belongs to a
    /// vocabulary the reading scope has never heard of must be translated
    /// through `maps_to`, not compared raw.
    ///
    /// `backlog` is `bug`-only and maps to `open` at `ticket` scope, so a
    /// `ticket`-scoped condition asking for `open` must match it.
    #[tokio::test]
    async fn a_related_nodes_extended_value_resolves_through_maps_to() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;

        let parent = make_bug(&svc, json!({ "state": "open" })).await;
        let child = make_bug(&svc, json!({ "state": "backlog" })).await;
        svc.create_relationship(&parent.id, "has_child", &child.id, json!({}))
            .await
            .expect("relationship creation failed");

        let rule = rule_on("ticket", "node.has_child.all(c, c.state == 'open')");
        let scope = PlaybookEngine::cel_scope_for(&svc, &rule, &parent).await;
        let mut resolver = crate::playbook::graph_resolver::GraphResolver::new(Arc::clone(&svc))
            .with_scope(scope.clone());
        let event = DomainEvent::NodeCreated {
            node_id: parent.id.clone(),
            node_type: parent.node_type.clone(),
        };
        let passed = matches!(
            evaluate_conditions_at_scope(
                &rule.conditions,
                &parent,
                &event,
                Some(&mut resolver),
                scope.as_ref(),
            )
            .await,
            ConditionResult::Pass
        );

        assert!(
            passed,
            "a related node's extended value must resolve through maps_to; \
             child properties were {:?}",
            child.properties
        );
    }
}

#[cfg(test)]
mod ancestry_cache_tests {
    //! The `extends` ancestry cache and its invalidation call sites (ADR-078).
    //!
    //! The cache is what lets a base-scoped Play match a subtype's events
    //! without a SQL walk per event. It is refreshed from four places — engine
    //! startup, and schema Created/Updated/Deleted — and a missed refresh is
    //! silent: the Play simply stops matching, with no error and no log. These
    //! tests cover each call site, so deleting any one of them fails here.

    use super::*;
    use crate::db::events::EventMetadata;
    use crate::db::SqliteStore;
    use crate::schema::handle_create_schema;
    use serde_json::json;
    use std::sync::atomic::Ordering;
    use tempfile::TempDir;

    async fn test_engine() -> (PlaybookEngine, Arc<NodeService>, TempDir) {
        let temp_dir = TempDir::new().expect("tempdir creation failed");
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(
            SqliteStore::new(db_path)
                .await
                .expect("SqliteStore init failed"),
        );
        let node_service = Arc::new(
            NodeService::new(&mut store)
                .await
                .expect("NodeService init failed"),
        );
        let engine = PlaybookEngine::new(Arc::clone(&node_service));
        (engine, node_service, temp_dir)
    }

    /// `bug extends ticket`, created through the real schema write path.
    async fn seed_bug_extends_ticket(svc: &Arc<NodeService>) {
        handle_create_schema(
            svc,
            json!({
                "name": "Ticket",
                "fields": [
                    { "name": "state", "type": "string", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .expect("ticket schema creation failed");

        handle_create_schema(
            svc,
            json!({
                "name": "Bug",
                "extends": "ticket",
                "fields": [
                    { "name": "severity", "type": "string", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .expect("bug schema creation failed");
    }

    /// The engine's current view of a type's ancestry.
    fn cached_ancestors(engine: &PlaybookEngine, node_type: &str) -> Vec<String> {
        engine
            .lifecycle
            .read()
            .expect("lifecycle lock poisoned")
            .ancestors_of(node_type)
    }

    fn envelope(event: DomainEvent) -> EventEnvelope {
        EventEnvelope {
            event,
            metadata: EventMetadata {
                source_client_id: None,
                playbook_context: None,
            },
        }
    }

    /// An unextended type's "ancestry" is just itself, and that is the correct
    /// answer — not a missing entry standing in for one.
    #[tokio::test]
    async fn an_unextended_type_is_its_own_ancestry() {
        let (engine, _svc, _tmp) = test_engine().await;
        engine.refresh_ancestor_cache().await;

        assert_eq!(cached_ancestors(&engine, "task"), ["task"]);
    }

    /// A refresh picks up an `extends` edge written before it ran.
    ///
    /// Every call-site test below depends on this working; testing it once on
    /// its own separates "the refresh is broken" from "a call site is missing"
    /// when something fails.
    #[tokio::test]
    async fn refresh_picks_up_an_extends_edge() {
        let (engine, svc, _tmp) = test_engine().await;
        seed_bug_extends_ticket(&svc).await;

        engine.refresh_ancestor_cache().await;

        assert_eq!(cached_ancestors(&engine, "bug"), ["bug", "ticket"]);
    }

    /// Call site 1 — engine startup (`load_active_plays`).
    ///
    /// Ancestry must be warm before the first event is dispatched. Without
    /// this refresh a base-scoped Play silently misses every subtype event
    /// until some unrelated schema write happens to warm the cache.
    #[tokio::test]
    async fn startup_warms_the_cache_before_any_event() {
        let (engine, svc, _tmp) = test_engine().await;
        seed_bug_extends_ticket(&svc).await;

        // Nothing has refreshed yet, so `bug` still looks unextended.
        assert_eq!(cached_ancestors(&engine, "bug"), ["bug"]);

        engine
            .load_active_plays()
            .await
            .expect("load_active_plays failed");

        assert_eq!(
            cached_ancestors(&engine, "bug"),
            ["bug", "ticket"],
            "startup must warm ancestry before the first event is dispatched"
        );
    }

    /// Call site 2 — `NodeCreated { node_type: "schema" }`.
    ///
    /// A newly created schema may declare `extends`, and creation never
    /// arrives as `NodeUpdated`, so the drift hook would not see it.
    #[tokio::test]
    async fn a_created_schema_refreshes_the_cache() {
        let (engine, svc, _tmp) = test_engine().await;
        engine.refresh_ancestor_cache().await;

        // The edge is written after the cache was last built, so only the
        // event-driven refresh can pick it up.
        seed_bug_extends_ticket(&svc).await;
        assert_eq!(cached_ancestors(&engine, "bug"), ["bug"]);

        let (queue_tx, _queue_rx) = mpsc::channel(EXECUTION_QUEUE_CAPACITY);
        engine
            .handle_event(
                envelope(DomainEvent::NodeCreated {
                    node_id: "bug".to_string(),
                    node_type: "schema".to_string(),
                }),
                &queue_tx,
            )
            .await;

        assert_eq!(
            cached_ancestors(&engine, "bug"),
            ["bug", "ticket"],
            "a created schema must refresh ancestry — creation never arrives as NodeUpdated"
        );
    }

    /// Call site 3 — `NodeDeleted { node_type: "schema" }`.
    ///
    /// A schema deletion may change what `extends` edges exist, and deletion
    /// never arrives as `NodeUpdated` either, so the drift hook would not see
    /// it.
    ///
    /// The schema deleted here is deliberately *not* part of the extends
    /// chain: `schema_has_declarations` refuses to delete either endpoint of
    /// an edge, and `update_schema` has no way to clear one, so a schema that
    /// extends something cannot be deleted through the service at all. What is
    /// reachable — and what this covers — is that the Deleted arm refreshes
    /// regardless of *which* schema went away, which is what keeps the cache
    /// from going stale across a deletion.
    #[tokio::test]
    async fn a_deleted_schema_refreshes_the_cache() {
        let (engine, svc, _tmp) = test_engine().await;
        engine.refresh_ancestor_cache().await;

        // The edge lands after the last refresh, so only the event-driven
        // refresh can pick it up.
        seed_bug_extends_ticket(&svc).await;
        handle_create_schema(
            &svc,
            json!({
                "name": "Standalone",
                "fields": [
                    { "name": "note", "type": "string", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .expect("standalone schema creation failed");
        assert_eq!(cached_ancestors(&engine, "bug"), ["bug"]);

        let standalone = svc
            .get_node("standalone")
            .await
            .expect("get_node failed")
            .expect("standalone schema node should exist");
        svc.delete_node("standalone", standalone.version)
            .await
            .expect("standalone schema delete failed");

        let (queue_tx, _queue_rx) = mpsc::channel(EXECUTION_QUEUE_CAPACITY);
        engine
            .handle_event(
                envelope(DomainEvent::NodeDeleted {
                    id: "standalone".to_string(),
                    node_type: "schema".to_string(),
                }),
                &queue_tx,
            )
            .await;

        assert_eq!(
            cached_ancestors(&engine, "bug"),
            ["bug", "ticket"],
            "a deleted schema must refresh ancestry — deletion never arrives as NodeUpdated"
        );
    }

    /// Call site 4 — `handle_schema_updated`.
    ///
    /// The refresh runs *before* the drift check, so that hook returning early
    /// cannot leave the cache stale. Passing an id that resolves to no schema
    /// node exercises exactly that early-return path.
    #[tokio::test]
    async fn an_updated_schema_refreshes_before_the_drift_check_can_return() {
        let (engine, svc, _tmp) = test_engine().await;
        engine.refresh_ancestor_cache().await;

        seed_bug_extends_ticket(&svc).await;
        assert_eq!(cached_ancestors(&engine, "bug"), ["bug"]);

        engine.handle_schema_updated("no-such-schema-node").await;

        assert_eq!(
            cached_ancestors(&engine, "bug"),
            ["bug", "ticket"],
            "the refresh must precede the drift check, which can return early"
        );
    }

    /// A successful refresh leaves nothing marked dirty.
    #[tokio::test]
    async fn a_successful_refresh_is_not_dirty() {
        let (engine, svc, _tmp) = test_engine().await;
        seed_bug_extends_ticket(&svc).await;

        engine.refresh_ancestor_cache().await;

        assert!(
            !engine.ancestry_dirty.load(Ordering::Relaxed),
            "a refresh that installed a cache must not be marked for retry"
        );
    }

    /// The producer half: a refresh that actually fails keeps the previous
    /// cache **and** marks it for retry.
    ///
    /// Without this, nothing covers the transition into the degraded state —
    /// only the recovery out of it. Both halves matter: a refresh that failed
    /// without setting the flag would never be retried, which is the exact
    /// unbounded-window bug the flag exists to close.
    ///
    /// The failure is real rather than injected: dropping the `relationship`
    /// table makes `get_extends_parent_map`'s SELECT fail the way a genuine
    /// I/O or corruption error would, exercising the actual `Err` arm.
    #[tokio::test]
    async fn a_failed_refresh_keeps_the_previous_cache_and_marks_it_dirty() {
        let (engine, svc, _tmp) = test_engine().await;
        seed_bug_extends_ticket(&svc).await;
        engine.refresh_ancestor_cache().await;

        // Precondition: a good cache, not dirty.
        assert_eq!(cached_ancestors(&engine, "bug"), ["bug", "ticket"]);
        assert!(!engine.ancestry_dirty.load(Ordering::Relaxed));

        svc.store()
            .write()
            .await
            .execute("DROP TABLE relationship", ())
            .await
            .expect("dropping the relationship table should succeed");

        engine.refresh_ancestor_cache().await;

        assert!(
            engine.ancestry_dirty.load(Ordering::Relaxed),
            "a failed refresh must mark the cache for retry, or nothing ever retries it"
        );
        assert_eq!(
            cached_ancestors(&engine, "bug"),
            ["bug", "ticket"],
            "a failed refresh must keep the previous cache — stale ancestry beats \
             dropping every Play's subtype matching"
        );
    }

    /// The dirty flag bounds the degraded window to one event.
    ///
    /// This is the half of the failure path worth pinning. A failed refresh
    /// keeps the previous cache — correct, since stale ancestry beats dropping
    /// every Play's subtype matching — but on its own that is unbounded:
    /// nothing retries, so a transient DB error at startup would leave subtype
    /// matching quietly degraded for the whole process lifetime, after one
    /// warning. `handle_event` re-refreshing on the flag is what turns
    /// "forever" into "until the next event".
    ///
    /// The dirty state is set directly rather than by breaking the store: what
    /// needs pinning is that a dirty cache recovers, and forcing a real I/O
    /// failure would test SQLite's caching behaviour rather than this logic.
    #[tokio::test]
    async fn a_dirty_cache_is_refreshed_before_the_next_event_is_matched() {
        let (engine, svc, _tmp) = test_engine().await;

        // Stand in for a refresh that failed at startup: the edge exists in
        // the store, the cache does not know about it, and the flag says so.
        seed_bug_extends_ticket(&svc).await;
        engine.ancestry_dirty.store(true, Ordering::Relaxed);
        assert_eq!(cached_ancestors(&engine, "bug"), ["bug"]);

        // An ordinary, unrelated event — not a schema event, so none of the
        // four call sites above fires. Only the dirty-flag retry can recover.
        let (queue_tx, _queue_rx) = mpsc::channel(EXECUTION_QUEUE_CAPACITY);
        engine
            .handle_event(
                envelope(DomainEvent::NodeCreated {
                    node_id: "some-task".to_string(),
                    node_type: "task".to_string(),
                }),
                &queue_tx,
            )
            .await;

        assert_eq!(
            cached_ancestors(&engine, "bug"),
            ["bug", "ticket"],
            "a dirty cache must be retried on the next event, not left degraded for the process"
        );
        assert!(
            !engine.ancestry_dirty.load(Ordering::Relaxed),
            "a successful retry must clear the dirty flag"
        );
    }
}
