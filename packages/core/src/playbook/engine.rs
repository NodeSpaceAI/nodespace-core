//! Playbook Engine
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
//! - Phase 7: Save-time validation before playbook activation

use crate::db::events::{DomainEvent, EventEnvelope};
use crate::playbook::lifecycle::{trigger_keys_for_event, PlaybookLifecycleManager};
use crate::playbook::logging::{create_or_update_log_node, PlaybookErrorType, MAX_CHAIN_DEPTH};
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

/// The playbook engine — subscribes to domain events and manages playbook lifecycle.
///
/// Runs in-process alongside NodeService. Subscribes to the broadcast channel
/// as a second subscriber (alongside DomainEventForwarder).
pub struct PlaybookEngine {
    /// Lifecycle manager behind RwLock for concurrent access.
    /// Read: event subscriber (frequent). Write: lifecycle ops (infrequent).
    lifecycle: Arc<RwLock<PlaybookLifecycleManager>>,
    /// NodeService for fetching playbook nodes and (later) executing actions.
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

    /// Initialize the engine: load active playbooks, build indexes, start event loop.
    ///
    /// Follows the startup sequence from the spec:
    /// 1. Subscribe to event broadcast channel FIRST (to avoid race)
    /// 2. Query all active playbook nodes
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
        info!("Playbook engine subscribed to event channel");

        // Step 2-3: Load active playbooks and build indexes
        self.load_active_playbooks().await?;

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

        info!("Playbook engine started, processing events...");

        // Step 6: Process events
        let result = loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(envelope) => {
                            self.handle_event(envelope, &queue_tx).await;
                        }
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            warn!("Playbook engine lagged, missed {} events", count);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Event channel closed, playbook engine shutting down");
                            break Ok(());
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Playbook engine received shutdown signal");
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

    /// Load all active playbook nodes from the database and activate them.
    async fn load_active_playbooks(&self) -> anyhow::Result<()> {
        let nodes = self
            .node_service
            .query_nodes_by_type("playbook", Some("active"))
            .await?;

        let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
        let mut loaded = 0;
        for node in &nodes {
            match lifecycle.activate_playbook(node) {
                Ok(()) => loaded += 1,
                Err(e) => {
                    warn!("Failed to parse playbook {}: {}", node.id, e);
                }
            }
        }

        info!(
            "Loaded {} active playbooks ({} total found)",
            loaded,
            nodes.len()
        );
        Ok(())
    }

    /// Handle a single event from the broadcast channel.
    ///
    /// Performs lifecycle management (detect playbook/schema CRUD), then trigger
    /// matching. Matched rules are bundled with the pre-fetched trigger node
    /// into an ExecutionWorkItem and sent to the RuleProcessor queue.
    async fn handle_event(
        &self,
        envelope: EventEnvelope,
        queue_tx: &mpsc::Sender<ExecutionWorkItem>,
    ) {
        // Lifecycle management: detect playbook node events
        match &envelope.event {
            DomainEvent::NodeCreated { node_type, node_id } if node_type == "playbook" => {
                self.handle_playbook_created(node_id).await;
                return;
            }
            DomainEvent::NodeDeleted { node_type, id } if node_type == "playbook" => {
                self.handle_playbook_deleted(id);
                return;
            }
            DomainEvent::NodeUpdated {
                node_type, node_id, ..
            } if node_type == "playbook" => {
                self.handle_playbook_updated(node_id).await;
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

        // Trigger matching for non-lifecycle events
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
                .map(|r| format!("{}[{}]", r.playbook_id, r.rule_index))
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

    /// Handle a new playbook node being created — validate, then parse and activate.
    ///
    /// Phase 7: runs save-time validation before activation. If validation fails,
    /// the playbook is disabled and a log node is created for each error.
    async fn handle_playbook_created(&self, node_id: &str) {
        match self.node_service.get_node(node_id).await {
            Ok(Some(node)) if node.lifecycle_status == "active" => {
                // Parse rules first for validation
                let parsed_rules = match parse_rules_for_validation(&node) {
                    Ok(rules) => rules,
                    Err(e) => {
                        warn!("Failed to parse playbook {} for validation: {}", node_id, e);
                        let _ = create_or_update_log_node(
                            &self.node_service,
                            node_id,
                            "parse",
                            0,
                            PlaybookErrorType::CompileError,
                            &format!("Failed to parse playbook rules: {}", e),
                            "n/a",
                        )
                        .await;
                        let mut lifecycle =
                            self.lifecycle.write().expect("lifecycle lock poisoned");
                        lifecycle.disable_playbook(node_id);
                        return;
                    }
                };

                // Phase 7: Save-time validation (belt-and-suspenders — primary gate is in NodeService)
                if let Err(errors) = crate::playbook::validation::validate_playbook(
                    &parsed_rules,
                    &*self.node_service,
                )
                .await
                {
                    warn!(
                        "Playbook {} failed save-time validation with {} error(s)",
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
                            PlaybookErrorType::CompileError,
                            &err.to_string(),
                            "n/a",
                        )
                        .await;
                    }
                    // Disable the playbook — do not activate
                    let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
                    lifecycle.disable_playbook(node_id);
                    return;
                }

                let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
                if let Err(e) = lifecycle.activate_playbook(&node) {
                    warn!("Failed to activate new playbook {}: {}", node_id, e);
                }
            }
            Ok(Some(_)) => {
                debug!("New playbook {} is not active, skipping", node_id);
            }
            Ok(None) => {
                warn!("Playbook {} not found after NodeCreated event", node_id);
            }
            Err(e) => {
                error!("Failed to fetch playbook {}: {}", node_id, e);
            }
        }
    }

    /// Handle a playbook node being deleted — remove from all indexes.
    fn handle_playbook_deleted(&self, playbook_id: &str) {
        let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
        lifecycle.deactivate_playbook(playbook_id);
    }

    /// Handle a playbook node being updated — detect status transitions.
    ///
    /// If lifecycle_status changed from disabled→active, re-enable (with validation).
    /// If rules changed, re-parse (with validation).
    /// Phase 7: validates before (re-)activation.
    async fn handle_playbook_updated(&self, node_id: &str) {
        let node = match self.node_service.get_node(node_id).await {
            Ok(Some(n)) => n,
            Ok(None) => {
                warn!("Playbook {} not found after NodeUpdated event", node_id);
                return;
            }
            Err(e) => {
                error!("Failed to fetch playbook {}: {}", node_id, e);
                return;
            }
        };

        // Read current status (short lock)
        let current_status = {
            let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
            lifecycle.get_playbook(node_id).map(|pb| pb.status.clone())
        };

        let needs_activation = matches!(
            (&current_status, node.lifecycle_status.as_str()),
            (Some(PlaybookStatus::Disabled), "active")
                | (Some(PlaybookStatus::Active), "active")
                | (None, "active")
        );

        // Active → Non-active: just disable, no validation needed
        if matches!(
            (&current_status, node.lifecycle_status.as_str()),
            (Some(PlaybookStatus::Active), status) if status != "active"
        ) {
            let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
            lifecycle.disable_playbook(node_id);
            return;
        }

        if needs_activation {
            // Phase 7: validate before (re-)activation (belt-and-suspenders)
            if let Ok(parsed_rules) = parse_rules_for_validation(&node) {
                if let Err(errors) = crate::playbook::validation::validate_playbook(
                    &parsed_rules,
                    &*self.node_service,
                )
                .await
                {
                    warn!(
                        "Playbook {} failed validation on update with {} error(s)",
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
                            PlaybookErrorType::CompileError,
                            &err.to_string(),
                            "n/a",
                        )
                        .await;
                    }
                    let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
                    lifecycle.disable_playbook(node_id);
                    return;
                }
            }

            let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
            match &current_status {
                Some(PlaybookStatus::Disabled) | Some(PlaybookStatus::Active) => {
                    if let Err(e) = lifecycle.reenable_playbook(&node) {
                        warn!("Failed to re-enable/update playbook {}: {}", node_id, e);
                    }
                }
                None => {
                    if let Err(e) = lifecycle.activate_playbook(&node) {
                        warn!("Failed to activate playbook {}: {}", node_id, e);
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
                        "Schema drift: {} playbooks disabled due to schema '{}' update: {:?}",
                        disabled.len(),
                        schema_node_type,
                        disabled
                    );
                    // Phase 6 will create log nodes for each disabled playbook
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

/// Parse a playbook node's rules into `Vec<Arc<ParsedRule>>` for validation.
///
/// This is used by the engine before activation to feed the validator.
/// It mirrors the parsing in `PlaybookLifecycleManager::activate_playbook`.
fn parse_rules_for_validation(
    node: &crate::models::Node,
) -> Result<Vec<Arc<ParsedRule>>, PlaybookParseError> {
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
/// item is skipped, offending playbooks are disabled, and log nodes are
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
        // skip this work item, disable offending playbooks, and create log nodes.
        if depth + 1 > MAX_CHAIN_DEPTH {
            warn!(
                "Cycle limit reached (depth {}), skipping work item for node {}",
                depth, work_item.trigger_node.id,
            );

            for rule_ref in &work_item.rules {
                // Disable the playbook that would have fired
                {
                    let mut lm = lifecycle.write().expect("lifecycle lock poisoned");
                    lm.disable_playbook(&rule_ref.playbook_id);
                }

                // Create (or deduplicate) a log node for this error
                let _ = create_or_update_log_node(
                    &node_service,
                    &rule_ref.playbook_id,
                    &rule_ref.rule.name,
                    rule_ref.rule_index,
                    PlaybookErrorType::CycleLimit,
                    &format!(
                        "Cycle depth limit ({}) exceeded for rule '{}' in playbook {}",
                        MAX_CHAIN_DEPTH, rule_ref.rule.name, rule_ref.playbook_id,
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
                "Processing rule '{}' from playbook {} (index {})",
                rule_ref.rule.name, rule_ref.playbook_id, rule_ref.rule_index,
            );

            // Phase 3 + #1010: Evaluate CEL conditions with graph resolver
            let mut resolver =
                crate::playbook::graph_resolver::GraphResolver::new(Arc::clone(&node_service));
            let condition_result = crate::playbook::cel::evaluate_conditions(
                &rule_ref.rule.conditions,
                &work_item.trigger_node,
                &work_item.trigger_event.event,
                Some(&mut resolver),
            );

            match condition_result {
                crate::playbook::cel::ConditionResult::Pass => {
                    debug!(
                        "Rule '{}' (playbook {}) conditions passed",
                        rule_ref.rule.name, rule_ref.playbook_id,
                    );
                }
                crate::playbook::cel::ConditionResult::Fail { condition_index } => {
                    debug!(
                        "Rule '{}' (playbook {}) skipped: condition[{}] evaluated to false",
                        rule_ref.rule.name, rule_ref.playbook_id, condition_index,
                    );
                    continue;
                }
                crate::playbook::cel::ConditionResult::CompileError {
                    condition_index,
                    message,
                } => {
                    warn!(
                        "Rule '{}' (playbook {}) has compile error in condition[{}]: {}",
                        rule_ref.rule.name, rule_ref.playbook_id, condition_index, message,
                    );
                    // Compile errors indicate a structural issue — disable the playbook
                    {
                        let mut lm = lifecycle.write().expect("lifecycle lock poisoned");
                        lm.disable_playbook(&rule_ref.playbook_id);
                    }
                    let _ = create_or_update_log_node(
                        &node_service,
                        &rule_ref.playbook_id,
                        &rule_ref.rule.name,
                        rule_ref.rule_index,
                        PlaybookErrorType::CompileError,
                        &format!(
                            "CEL compile error in condition[{}]: {}",
                            condition_index, message
                        ),
                        &work_item.trigger_node.id,
                    )
                    .await;
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
                source_playbook_id: rule_ref.playbook_id.clone(),
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
                        "Rule '{}' (playbook {}) executed successfully",
                        rule_ref.rule.name, rule_ref.playbook_id,
                    );
                }
                crate::playbook::actions::ActionResult::Failed(err) => {
                    warn!(
                        "Rule '{}' (playbook {}) action failed: {}",
                        rule_ref.rule.name, rule_ref.playbook_id, err,
                    );
                    // Disable the playbook on action failure (per spec)
                    {
                        let mut lm = lifecycle.write().expect("lifecycle lock poisoned");
                        lm.disable_playbook(&rule_ref.playbook_id);
                    }
                    let _ = create_or_update_log_node(
                        &node_service,
                        &rule_ref.playbook_id,
                        &rule_ref.rule.name,
                        rule_ref.rule_index,
                        PlaybookErrorType::ActionError,
                        &format!("Action execution failed: {}", err),
                        &work_item.trigger_node.id,
                    )
                    .await;
                    // Skip remaining rules from this playbook in the current batch
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

/// Extract the trigger node ID from a domain event.
///
/// Returns the node_id for events that can trigger playbook rules.
/// Returns `None` for events that don't carry a node_id relevant to triggers
/// (e.g., RelationshipCreated — those need source node lookup, deferred to Phase 2+).
pub(crate) fn trigger_node_id(event: &DomainEvent) -> Option<&str> {
    match event {
        DomainEvent::NodeCreated { node_id, .. } => Some(node_id.as_str()),
        DomainEvent::NodeUpdated { node_id, .. } => Some(node_id.as_str()),
        _ => None,
    }
}
