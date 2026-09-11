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
//! - Phase 6: Cycle detection (max depth 10) + log node deduplication
//! - Phase 7: Save-time validation before play activation

use crate::db::events::{DomainEvent, EventEnvelope};
use crate::playbook::lifecycle::{trigger_keys_for_event, PlaybookLifecycleManager};
use crate::playbook::logging::{create_or_update_log_node, PlayErrorType, MAX_CHAIN_DEPTH};
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
/// Runs in-process alongside NodeService. Subscribes to the broadcast channel
/// as a second subscriber (alongside DomainEventForwarder).
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
}

impl PlaybookEngine {
    /// Create a new PlaybookEngine.
    ///
    /// Does NOT start the event subscription — call `start()` to begin processing.
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self {
            lifecycle: Arc::new(RwLock::new(PlaybookLifecycleManager::new())),
            node_service,
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

    /// Load all active play nodes from the database and activate them.
    async fn load_active_plays(&self) -> anyhow::Result<()> {
        let nodes = self
            .node_service
            .query_nodes_by_type("play", Some("active"))
            .await?;

        let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
        let mut loaded = 0;
        for node in &nodes {
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
            return;
        }

        // Trigger matching for non-lifecycle, locally-originated events
        let keys = trigger_keys_for_event(&envelope.event);
        if keys.is_empty() {
            return;
        }

        let matched_rules = {
            let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
            lifecycle.lookup_rules(&keys)
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

    /// Handle a new play node being created — validate, then parse and activate.
    ///
    /// Phase 7: runs save-time validation before activation. If validation fails,
    /// the play is disabled and a log node is created for each error.
    async fn handle_play_created(&self, node_id: &str) {
        match self.node_service.get_node(node_id).await {
            Ok(Some(node)) if node.lifecycle_status == "active" => {
                // Parse rules first for validation
                let parsed_rules = match parse_rules_for_validation(&node) {
                    Ok(rules) => rules,
                    Err(e) => {
                        warn!("Failed to parse play {} for validation: {}", node_id, e);
                        let _ = create_or_update_log_node(
                            &self.node_service,
                            node_id,
                            "parse",
                            0,
                            PlayErrorType::CompileError,
                            &format!("Failed to parse play rules: {}", e),
                            "n/a",
                        )
                        .await;
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
                    warn!(
                        "Play {} failed save-time validation with {} error(s)",
                        node_id,
                        errors.len()
                    );
                    for err in &errors {
                        warn!("  Validation error: {}", err);
                        let _ = create_or_update_log_node(
                            &self.node_service,
                            node_id,
                            "validation",
                            0,
                            PlayErrorType::CompileError,
                            &err.to_string(),
                            "n/a",
                        )
                        .await;
                    }
                    // Disable the play — do not activate
                    let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
                    lifecycle.disable_play(node_id);
                    return;
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

        // Read current status (short lock)
        let current_status = {
            let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
            lifecycle.get_play(node_id).map(|pb| pb.status.clone())
        };

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
                    warn!(
                        "Play {} failed validation on update with {} error(s)",
                        node_id,
                        errors.len()
                    );
                    for err in &errors {
                        warn!("  Validation error: {}", err);
                        let _ = create_or_update_log_node(
                            &self.node_service,
                            node_id,
                            "validation",
                            0,
                            PlayErrorType::CompileError,
                            &err.to_string(),
                            "n/a",
                        )
                        .await;
                    }
                    let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
                    lifecycle.disable_play(node_id);
                    return;
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

                let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
                let disabled = lifecycle.handle_schema_update(schema_node_type, new_version);

                if !disabled.is_empty() {
                    warn!(
                        "Schema drift: {} plays disabled due to schema '{}' update: {:?}",
                        disabled.len(),
                        schema_node_type,
                        disabled
                    );
                    // Phase 6 will create log nodes for each disabled play
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
/// Enforces cycle detection: when `depth + 1 > MAX_CHAIN_DEPTH`, the work
/// item is skipped, offending plays are disabled, and log nodes are
/// created with fingerprint-based deduplication.
pub(crate) async fn rule_processor_loop(
    mut rx: mpsc::Receiver<ExecutionWorkItem>,
    lifecycle: Arc<RwLock<PlaybookLifecycleManager>>,
    node_service: Arc<NodeService>,
) {
    info!("RuleProcessor started, waiting for work items...");

    while let Some(work_item) = rx.recv().await {
        let depth = work_item
            .trigger_event
            .metadata
            .playbook_context
            .as_ref()
            .map(|ctx| ctx.depth)
            .unwrap_or(0);

        // Cycle detection: if the next execution would exceed MAX_CHAIN_DEPTH,
        // skip this work item, disable offending plays, and create log nodes.
        if depth + 1 > MAX_CHAIN_DEPTH {
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

                // Create (or deduplicate) a log node for this error
                let _ = create_or_update_log_node(
                    &node_service,
                    &rule_ref.play_id,
                    &rule_ref.rule.name,
                    rule_ref.rule_index,
                    PlayErrorType::CycleLimit,
                    &format!(
                        "Cycle depth limit ({}) exceeded for rule '{}' in play {}",
                        MAX_CHAIN_DEPTH, rule_ref.rule.name, rule_ref.play_id,
                    ),
                    &work_item.trigger_node.id,
                )
                .await;
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

        // Process each matched rule in order
        for rule_ref in &work_item.rules {
            debug!(
                "Processing rule '{}' from play {} (index {})",
                rule_ref.rule.name, rule_ref.play_id, rule_ref.rule_index,
            );

            // Phase 3: Evaluate CEL conditions with graph resolver
            let mut resolver =
                crate::playbook::graph_resolver::GraphResolver::new(Arc::clone(&node_service));
            let condition_result = crate::playbook::cel::evaluate_conditions(
                &rule_ref.rule.conditions,
                &work_item.trigger_node,
                &work_item.trigger_event.event,
                Some(&mut resolver),
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
                depth: depth + 1,
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
                    warn!(
                        "Rule '{}' (play {}) action failed: {}",
                        rule_ref.rule.name, rule_ref.play_id, err,
                    );
                    // Disable the play on action failure (per spec)
                    {
                        let mut lm = lifecycle.write().expect("lifecycle lock poisoned");
                        lm.disable_play(&rule_ref.play_id);
                    }
                    let _ = create_or_update_log_node(
                        &node_service,
                        &rule_ref.play_id,
                        &rule_ref.rule.name,
                        rule_ref.rule_index,
                        PlayErrorType::ActionError,
                        &format!("Action execution failed: {}", err),
                        &work_item.trigger_node.id,
                    )
                    .await;
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
