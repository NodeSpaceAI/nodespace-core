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

use crate::db::events::{chain_depth_of_write, DomainEvent, EventEnvelope};
use crate::models::PlaySuspensionReason;
use crate::playbook::lifecycle::{
    parse_play_rules, trigger_keys_for_event, PlaybookLifecycleManager,
};
use crate::playbook::types::*;
use crate::services::{NodeService, NodeServiceError};
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

        // Step 2-3: Load every play and index the runnable ones
        self.load_plays().await?;

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
                            // A missed event may have been a play's own:
                            // created, switched off, suspended, deleted. The
                            // run state is a cache of the nodes, so rebuild
                            // it from them rather than run on a stale one.
                            warn!(
                                "Play engine lagged, missed {} events; reloading plays",
                                count
                            );
                            if let Err(e) = self.load_plays().await {
                                error!("Failed to reload plays after lagging: {}", e);
                            }
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
    /// Used by `sync_play`, which re-runs save-time validation at load and on
    /// every play create and update.
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
    /// authored against whatever concrete subtype fired it. A wildcard (`*`)
    /// trigger reads the node at its own type. Returns `Ok(None)` when there
    /// is nothing to scope — the node reads at its own type and that type
    /// extends nothing — which is every rule until something declares
    /// `extends`.
    ///
    /// Returns `Err` when a resolver call itself fails (a transient DB error,
    /// not a schema-shape problem). This is deliberately distinct from
    /// `Ok(None)`: folding a resolver error into the same `None` used for
    /// "nothing to scope" would be indistinguishable from those legitimate
    /// cases, and a caller evaluating conditions against the raw,
    /// unprojected node on a base-type-registered rule reads the wrong
    /// property bucket and simply fails to match — silently, with nothing
    /// logged. Callers must not treat `Err` as `Ok(None)`.
    pub(crate) async fn cel_scope_for(
        node_service: &Arc<NodeService>,
        rule: &ParsedRule,
        node: &crate::models::Node,
        scanned_type: Option<&str>,
    ) -> Result<Option<crate::playbook::cel::CelScope>, NodeServiceError> {
        let scope_type = match Self::registered_type(rule, scanned_type) {
            Some(registered) => registered,
            None => &node.node_type,
        };
        crate::playbook::cel::CelScope::resolve(node_service, scope_type, node).await
    }

    /// The type a rule is registered on, or `None` for a rule with no
    /// vocabulary of its own (a wildcard `*` trigger).
    ///
    /// A rule names its type in its trigger's selector. A scheduled rule that
    /// selects through a saved query does not: its type is the query's
    /// `target_type`, which the scan that selected the node read and passes
    /// as `scanned_type`.
    fn registered_type<'a>(rule: &'a ParsedRule, scanned_type: Option<&'a str>) -> Option<&'a str> {
        rule.trigger
            .registered_type()
            .or(scanned_type)
            .filter(|registered| *registered != "*")
    }

    /// The type a rule's graph resolver reads traversed nodes at — see
    /// `GraphResolver::reading_type`.
    ///
    /// The rule's registered type, except for a wildcard (`*`) trigger: that
    /// rule has no vocabulary of its own, so a related node reads at its own
    /// type rather than at whatever type happened to fire it.
    pub(crate) fn reading_type(rule: &ParsedRule, scanned_type: Option<&str>) -> Option<String> {
        Self::registered_type(rule, scanned_type).map(str::to_string)
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

    /// Load every play node and bring the engine's state in line with each
    /// (see [`Self::sync_play`]). A play that does not run is kept with its
    /// reason; one whose rules fail validation is suspended, so the failure
    /// is visible rather than silently skipped. Runs at startup, and again
    /// when the subscriber lags and may have missed a play's event.
    ///
    /// The query leaves archived plays out, as every default query does: an
    /// archived play participates in nothing (ADR-087 §2).
    async fn load_plays(&self) -> anyhow::Result<()> {
        // Ancestry must be warm before any event is dispatched, or a
        // base-scoped Play would silently miss subtype events until the first
        // schema write of the process.
        self.refresh_ancestor_cache().await;

        let nodes = self
            .node_service
            .query_nodes_by_type(crate::models::CoreNodeType::Play.as_str(), false)
            .await?;

        for node in &nodes {
            self.sync_play(node).await;
            // A suspension outlives the run that recorded it, so say so at
            // each start: the play stays off until someone enables it.
            if PlayStatus::of_node(node) == PlayStatus::Suspended {
                let field = |key: &str| {
                    crate::models::PlayFields::stored_field(&node.properties, key)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                };
                warn!(
                    play_id = %node.id,
                    reason = %field(nodespace_types::PLAY_SUSPENDED_REASON_FIELD),
                    suspended_at = %field(nodespace_types::PLAY_SUSPENDED_AT_FIELD),
                    "Play '{}' is suspended and will not run until it is enabled: {}",
                    node.content,
                    field(nodespace_types::PLAY_SUSPENDED_MESSAGE_FIELD)
                );
            }
        }

        let runnable = {
            let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
            // On a reload, a play no longer in the store's load was deleted
            // or archived since. At startup there is nothing to forget.
            lifecycle.retain_plays(&nodes.iter().map(|node| node.id.as_str()).collect());
            lifecycle
                .active_playbooks()
                .values()
                .filter(|play| play.status == PlayStatus::Runnable)
                .count()
        };
        info!(
            "Loaded {} runnable plays ({} total found)",
            runnable,
            nodes.len()
        );
        Ok(())
    }

    /// Bring the engine's state for one play in line with its node.
    ///
    /// The one place the engine decides whether a play runs (ADR-087 §5): it
    /// participates ∧ it is enabled ∧ it is not suspended
    /// ([`PlayStatus::of_node`]) ∧ its rules parse and validate. Startup, a
    /// created play and an updated play all come through here.
    ///
    /// - A play that does not run is parked with its reason, out of the
    ///   indexes. No validation is needed to switch a play off.
    /// - A play whose rules fail to parse, or genuinely fail validation, is
    ///   suspended: the reason is recorded on the node, so it survives a
    ///   restart and every client can see it.
    /// - Otherwise it is indexed afresh from the node.
    ///
    /// Validation here is belt-and-suspenders (the primary gate is in
    /// `NodeService`): a play row may have been written by another device or
    /// an earlier build whose validation differs, and a schema may have
    /// changed under a play since it was saved.
    async fn sync_play(&self, node: &crate::models::Node) {
        // Compiled once, before any lock is taken.
        let parsed = parse_play_rules(node);

        let status = PlayStatus::of_node(node);
        if status != PlayStatus::Runnable {
            let rules = parsed.unwrap_or_default();
            let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
            lifecycle.park_play(node, status, rules);
            return;
        }

        let parsed_rules = match parsed {
            Ok(rules) => rules,
            Err(e) => {
                warn!(
                    play_id = %node.id,
                    error_type = "compile_error",
                    error = %e,
                    "Failed to parse play rules; suspending the play"
                );
                self.suspend_unrunnable(
                    node,
                    Vec::new(),
                    format!("Failed to parse play rules: {e}"),
                )
                .await;
                return;
            }
        };

        if let Err(errors) =
            crate::playbook::validation::validate_play(&parsed_rules, &self.node_service).await
        {
            self.log_validation_errors(&node.id, &errors);
            if crate::playbook::validation::has_genuine_failure(&errors) {
                warn!(
                    "Play {} failed validation with {} error(s); suspending the play",
                    node.id,
                    errors.len()
                );
                let detail = errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                self.suspend_unrunnable(node, parsed_rules, detail).await;
                return;
            }
            // Every error is SchemaResolutionFailed: validation could not
            // reach a verdict (a transient DB error), which is not evidence
            // the play is broken. Suspending on it would take a working
            // automation offline over a hiccup, and skipping the play would
            // leave an edit silently unapplied, so it is indexed anyway. A
            // later schema or play write re-validates it.
            warn!(
                "Play {} validation was inconclusive ({} resolution failure(s)); indexing it \
                 anyway rather than suspending on an unconfirmed verdict",
                node.id,
                errors.len()
            );
        }

        let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
        lifecycle.reindex_play(node, parsed_rules);
    }

    /// Suspend a play whose rules do not parse or validate: park it and
    /// record `validation_failed` on its node.
    async fn suspend_unrunnable(
        &self,
        node: &crate::models::Node,
        rules: Vec<Arc<ParsedRule>>,
        message: String,
    ) {
        {
            let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
            lifecycle.park_play(node, PlayStatus::Suspended, rules);
        }
        record_suspension(
            &self.node_service,
            &node.id,
            PlaySuspensionReason::ValidationFailed,
            &message,
        )
        .await;
    }

    /// Best-effort node fetch for `handle_event`'s two lookups below (a
    /// relationship event's source node, and a matched event's trigger
    /// node): `Ok(Some(_))` becomes `Some`, and both "doesn't exist" and "the
    /// fetch itself failed" become `None`, logged identically either way
    /// `handle_event` needs this lookup. `context` names what the id is, for
    /// the log line (e.g. `"trigger node"`, `"relationship source node"`).
    async fn fetch_node_logged(&self, id: &str, context: &str) -> Option<crate::models::Node> {
        match self.node_service.get_node(id).await {
            Ok(Some(node)) => Some(node),
            Ok(None) => {
                debug!(
                    "{} {} not found (deleted before processing?), skipping",
                    context, id
                );
                None
            }
            Err(e) => {
                error!("Failed to fetch {} {}: {}", context, id, e);
                None
            }
        }
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

        // Lifecycle management: detect play node events. A node of a type
        // extending `play` is a play, so the event's type is resolved through
        // its chain; the startup load finds the same nodes, since a query for
        // `play` returns its subtypes.
        let play_event = match &envelope.event {
            DomainEvent::NodeCreated { node_type, .. }
            | DomainEvent::NodeDeleted { node_type, .. }
            | DomainEvent::NodeUpdated { node_type, .. } => {
                let play = crate::models::CoreNodeType::Play;
                match self.node_service.type_is_a(node_type, play).await {
                    Ok(is_play) => is_play,
                    Err(e) => {
                        tracing::warn!(
                            node_type = %node_type,
                            error = %e,
                            "failed to resolve an event's type chain; matching the type alone"
                        );
                        play.is_exactly(node_type)
                    }
                }
            }
            _ => false,
        };
        match &envelope.event {
            DomainEvent::NodeCreated { node_id, .. } if play_event => {
                self.handle_play_created(node_id).await;
                return;
            }
            DomainEvent::NodeDeleted { id, .. } if play_event => {
                self.handle_play_deleted(id);
                return;
            }
            DomainEvent::NodeUpdated { node_id, .. } if play_event => {
                self.handle_play_updated(node_id).await;
                return;
            }
            // Schema version drift detection
            DomainEvent::NodeUpdated {
                node_type, node_id, ..
            } if crate::models::CoreNodeType::Schema.is_exactly(node_type) => {
                self.handle_schema_updated(node_id).await;
                return;
            }
            // A newly created schema may declare `extends`, and a deleted one
            // may remove an edge — neither arrives as NodeUpdated, so the
            // drift hook above would never see them and the ancestry cache
            // would stay stale until some unrelated schema edit. Refresh, but
            // don't return: schema creation/deletion is not itself drift, and
            // a Play may legitimately trigger on it.
            DomainEvent::NodeCreated { node_type, .. }
                if crate::models::CoreNodeType::Schema.is_exactly(node_type) =>
            {
                self.refresh_ancestor_cache().await;
            }
            DomainEvent::NodeDeleted { node_type, .. }
                if crate::models::CoreNodeType::Schema.is_exactly(node_type) =>
            {
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
        // replicated-apply path (tagged with `REPLICATED_APPLY_CLIENT_ID`, per
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
        if is_replicated_apply(&envelope) {
            debug!(
                node_event = ?trigger_node_id(&envelope.event),
                "Skipping trigger evaluation for replicated-apply event"
            );
            // ADR-060 §7: repair-and-log. This IS the mechanism that makes
            // `RuleClass::Invariant` rules sync-safe for a node received
            // already-committed via sync — architecturally separate from the
            // reactive `ExecutionQueue` above (never enqueued there; run
            // inline, here, against the already-committed node), and
            // deliberately still reached even though trigger evaluation for
            // reactive/invariant firing is skipped for this event. Covers
            // received updates as well as creates: a `property_changed`
            // invariant is violated just as permanently by a remote update
            // as by a remote create.
            self.dispatch_invariant_repair(&envelope.event).await;
            return;
        }

        // Trigger matching for non-lifecycle, locally-originated events.
        //
        // A relationship event carries no node type inline the way
        // NodeCreated/NodeUpdated do (see `relationship_source_id`'s doc), so
        // matching one needs its source node fetched first, here, to resolve
        // that type before `TriggerKey` construction. That fetch is real I/O,
        // unlike every other trigger check this index answers, so it is
        // gated behind `has_relationship_triggers` — an O(1) in-memory check
        // (ADR-078) — first: the overwhelmingly common case is zero installed
        // plays registering a relationship trigger at all, and every
        // `has_child`/`mentions`/`member_of` write in the app (every outline
        // indent/outdent, every mention, every collection add) reaches this
        // point. `lifecycle.lookup_rules` below (the trigger index itself)
        // stays a pure in-memory hash lookup either way, and a matched
        // relationship event reuses this same fetch as its `trigger_node`
        // below rather than fetching the source node twice.
        let relationship_source = match relationship_source_id(&envelope.event) {
            Some(id)
                if self
                    .lifecycle
                    .read()
                    .expect("lifecycle lock poisoned")
                    .has_relationship_triggers() =>
            {
                self.fetch_node_logged(id, "relationship source node").await
            }
            _ => None,
        };

        let keys = trigger_keys_for_event(
            &envelope.event,
            relationship_source.as_ref().map(|n| n.node_type.as_str()),
        );
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

        // The trigger node for a relationship event is the source node
        // already fetched above to resolve its type; for every other event
        // it's fetched here for the first time.
        let trigger_node = if let Some(node) = relationship_source {
            node
        } else {
            let trigger_node_id = match trigger_node_id(&envelope.event) {
                Some(id) => id,
                None => return,
            };

            match self
                .fetch_node_logged(trigger_node_id, "trigger node")
                .await
            {
                Some(node) => node,
                None => return,
            }
        };

        // No rule fires on an archived node (ADR-087 §2).
        if !crate::governance::participates(&trigger_node) {
            debug!(
                node_id = %trigger_node.id,
                "Trigger node is archived; no rule fires on it"
            );
            return;
        }

        // Enqueue the work item
        let work_item = ExecutionWorkItem {
            rules: matched_rules,
            trigger_event: envelope,
            trigger_node,
            scan: None,
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
    /// `event` is the event of a just-applied replicated write. Only
    /// `NodeCreated` and `NodeUpdated` are repaired; any other event is a
    /// no-op. Rules are matched with `trigger_keys_for_event` — the same
    /// derivation the local pre-commit update path uses — so a received
    /// update re-checks only the `property_changed` invariants keyed on a
    /// property it actually changed (exact key or wildcard), and a received
    /// create re-checks the `node_created` invariants.
    ///
    /// For each matched `RuleClass::Invariant` rule whose condition STILL
    /// passes against the node as it now stands (i.e. the effect the rule
    /// would have applied is absent — the node violates an invariant this
    /// device holds), runs the rule's actions as an ordinary write (no
    /// transaction to join — the node already committed on the originating
    /// device) and logs the repair. A rule whose condition now fails is left
    /// alone: the node already carries the required effect (applied by
    /// whichever device originated it, or by an earlier repair — this
    /// device's own or one that already synced in), so re-running would be
    /// redundant at best.
    ///
    /// Loop safety. The repair write goes through `self.node_service`, not
    /// the sync-tagged service, so its own `NodeUpdated` is local-origin and
    /// can never re-enter this sync-only branch on this device. Across
    /// devices, a repair continues the chain only when the received write
    /// was itself a play write (`chain_depth_of_write`, read from the
    /// received event's own committed node and diff), and otherwise starts
    /// one at 0 (ADR-060 §5). Every repair write mints a fresh write id, so
    /// a repair that syncs to another device and triggers a repair there
    /// continues the same count, and once the next hop would exceed
    /// `MAX_CHAIN_DEPTH` the repair is skipped. A distributed ping-pong
    /// between devices whose invariants disagree terminates, while a user's
    /// edit to a node a play once stamped near the limit is still repaired.
    /// Invariant rules the repair's own write triggers in-transaction on this
    /// device continue the same count, so a cycle that passes through them
    /// is bounded too.
    ///
    /// Best-effort: a failure fetching the node or evaluating/executing one
    /// rule is logged and does not block the others, since (unlike the
    /// pre-commit path) there is no write to roll back here — the node is
    /// already durably committed either way.
    async fn dispatch_invariant_repair(&self, event: &DomainEvent) {
        let node_id = match event {
            DomainEvent::NodeCreated { node_id, .. } | DomainEvent::NodeUpdated { node_id, .. } => {
                node_id.as_str()
            }
            _ => return,
        };
        // Keys use the event's `node_type` while conditions below read the
        // freshly fetched node; the two differ only if the node's type
        // changed after this event was emitted.
        let keys = trigger_keys_for_event(event, None);
        if keys.is_empty() {
            return;
        }
        let matched = {
            let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
            lifecycle.lookup_rules(&keys)
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

        // No rule fires on an archived node (ADR-087 §2), repair included.
        if !crate::governance::participates(&node) {
            debug!(node_id, "Repair-and-log: node is archived, skipping");
            return;
        }

        // Whether the received write continues a play chain is decided from
        // the event as received (its committed node and diff), before the
        // event is rebuilt around the re-fetched node below.
        let parent_depth =
            chain_depth_of_write(event, &node.properties, MAX_CHAIN_DEPTH).unwrap_or(0);

        // Conditions and action bindings see the node as it stands now, not
        // the snapshot the event carried: a later write may already have
        // repaired (or re-broken) it. An update keeps its own
        // `changed_properties`, so a condition on the change still reads it.
        let event = match event {
            DomainEvent::NodeUpdated {
                changed_properties, ..
            } => DomainEvent::NodeUpdated {
                node_id: node.id.clone(),
                node_type: node.node_type.clone(),
                node: node.clone(),
                changed_properties: changed_properties.clone(),
            },
            _ => DomainEvent::NodeCreated {
                node_id: node.id.clone(),
                node_type: node.node_type.clone(),
            },
        };

        if exceeds_max_chain_depth(parent_depth) {
            warn!(
                node_id = %node.id,
                depth = parent_depth,
                error_type = "cycle_limit",
                max_chain_depth = MAX_CHAIN_DEPTH,
                "Repair-and-log: cycle depth limit reached; skipping invariant repair"
            );
            return;
        }

        for rule_ref in invariant_rules {
            let cel_scope = match PlaybookEngine::cel_scope_for(
                &self.node_service,
                &rule_ref.rule,
                &node,
                None,
            )
            .await
            {
                Ok(scope) => scope,
                Err(e) => {
                    // Best-effort, same posture as the node-fetch failure
                    // above: a scope resolver error here would otherwise
                    // silently fold into "nothing to scope", evaluating
                    // this rule against the node's raw, unprojected
                    // properties and reporting a false non-violation.
                    // Skip this rule rather than repair (or not-repair)
                    // on a wrong read; the others in this batch are
                    // unaffected.
                    warn!(
                        node_id = %node.id,
                        play_id = %rule_ref.play_id,
                        rule = %rule_ref.rule.name,
                        error = %e,
                        "Repair-and-log: failed to resolve CEL scope for rule; skipping"
                    );
                    continue;
                }
            };
            // The resolver reads related nodes at this rule's scope too, so a
            // traversed node is projected exactly as the trigger node is.
            let mut resolver =
                crate::playbook::graph_resolver::GraphResolver::new(Arc::clone(&self.node_service))
                    .with_reading_type(PlaybookEngine::reading_type(&rule_ref.rule, None));
            let condition_result = crate::playbook::cel::evaluate_conditions_at_scope(
                &rule_ref.rule.conditions,
                &node,
                &event,
                Some(&mut resolver),
                cel_scope.as_ref(),
            )
            .await;

            match condition_result {
                crate::playbook::cel::ConditionResult::Pass => {}
                crate::playbook::cel::ConditionResult::Fail { .. } => continue,
                crate::playbook::cel::ConditionResult::Unresolved { reason } => {
                    // Unknown is not "still violates": skip, as for the scope
                    // failure above, rather than repair on a wrong read.
                    warn!(
                        node_id = %node.id,
                        play_id = %rule_ref.play_id,
                        rule = %rule_ref.rule.name,
                        error = %reason,
                        "Repair-and-log: failed to resolve a condition path for rule; skipping"
                    );
                    continue;
                }
            }

            info!(
                node_id = %node.id,
                play_id = %rule_ref.play_id,
                rule = %rule_ref.rule.name,
                "Repairing invariant violation on node received via sync"
            );

            let execution_context = crate::db::events::PlaybookExecutionContext {
                originating_event_id: uuid::Uuid::new_v4().to_string(),
                depth: parent_depth.saturating_add(1),
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
                // A `reject` rule vetoes the write it guards, but this write
                // already committed on another device and cannot be undone
                // here. The violation is logged; nothing else is possible.
                crate::playbook::actions::ActionResult::Failed(
                    crate::playbook::actions::ActionError::Rejected { message, .. },
                ) => {
                    warn!(
                        node_id = %node.id,
                        play_id = %rule_ref.play_id,
                        rule = %rule_ref.rule.name,
                        reason = %message,
                        error_type = "rejected_remote_write",
                        rule_index = rule_ref.rule_index,
                        "Repair-and-log: node received via sync violates a reject invariant; \
                         the remote write is already committed and cannot be undone"
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

    /// Handle a new play node being created: the same path as startup
    /// (see [`Self::sync_play`]), so a play created disabled, archived or
    /// with rules that do not validate is known to the engine with its reason.
    async fn handle_play_created(&self, node_id: &str) {
        match self.node_service.get_node(node_id).await {
            Ok(Some(node)) => self.sync_play(&node).await,
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

    /// Handle a play node being updated: re-read the node and recompute its
    /// run state (see [`Self::sync_play`]).
    ///
    /// An update that touches only the suspension fields (the engine's own
    /// write) lands here too and changes nothing: the play is already parked.
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

        self.warn_on_seeded_play_change(&node);
        self.sync_play(&node).await;
    }

    /// ADR-060 §8: a seeded play carrying an invariant rule warns, naming the
    /// concrete consequence, when `enabled` goes from `true` to `false` or
    /// its rules are edited (ADR-087 §5).
    ///
    /// Compared against what the engine last read from the node, which is
    /// the play as it was BEFORE the update that just landed: the warning
    /// names the invariant rule(s) the play carried then.
    fn warn_on_seeded_play_change(&self, node: &crate::models::Node) {
        let (action, rule_names) = {
            let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
            // First sight of the play is installation, not an edit.
            let Some(previous) = lifecycle.get_play(&node.id) else {
                return;
            };
            let Some(action) = crate::playbook::seeded::warned_change(previous, node) else {
                return;
            };
            (
                action,
                crate::playbook::seeded::invariant_rule_names(&previous.rules),
            )
        };
        warn!(
            play_id = %node.id,
            error_type = "seeded_play_warning",
            "{}",
            crate::playbook::seeded::edit_or_disable_warning(&node.id, action, &rule_names)
        );
    }

    /// Handle a schema node being updated — check for version drift.
    async fn handle_schema_updated(&self, schema_node_id: &str) {
        // A schema write may have added, re-targeted or removed an `extends`
        // edge, which changes what a base-scoped Play matches. Rebuild the
        // ancestry cache before the drift check below, so this hook cannot
        // return early (the schema node is already gone, say) and leave the
        // cache stale.
        self.refresh_ancestor_cache().await;

        match self.node_service.get_node(schema_node_id).await {
            Ok(Some(node)) => {
                // A schema node's id is the type it defines, and its
                // properties are flat (no bucket).
                let schema_node_type = node.id.as_str();
                let new_version = node
                    .properties
                    .get("schemaVersion")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);

                // Referencing the changed type is not the same as being broken
                // by the change. Disabling every Play that merely mentions it
                // means an additive edit — `add_field_values` adding a status
                // value, blessed by ADR-076 — silently stops core automation
                // that the change provably cannot break.
                //
                // So candidates are re-validated against the NEW schema and
                // only genuinely-broken Plays are suspended. Validation needs
                // store access, so it runs outside the lifecycle lock: gather
                // candidates under a read lock, validate, then suspend.
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
                        play_id = %play_id,
                        error_type = "schema_drift",
                        "Schema '{}' updated to version '{}', suspending play {} — its rules \
                         no longer validate: {}",
                        schema_node_type, new_version, play_id, detail
                    );
                    suspend_play(
                        &self.lifecycle,
                        &self.node_service,
                        play_id,
                        PlaySuspensionReason::SchemaDrift,
                        &format!(
                            "Schema '{schema_node_type}' changed (version {new_version}) and the \
                             play's rules no longer validate: {detail}"
                        ),
                    )
                    .await;
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
// Suspension
// ---------------------------------------------------------------------------

/// Take a running play out of service and record why on its node
/// (ADR-087 §5).
///
/// The play leaves the trigger index and the cron registry at once; the
/// suspension is then written to the node, where it survives a restart. A
/// play that is already out of service is left alone, so a failure that
/// reaches several queued rules of one play records one suspension. The
/// user's `enabled` switch is never touched.
pub(crate) async fn suspend_play(
    lifecycle: &Arc<RwLock<PlaybookLifecycleManager>>,
    node_service: &NodeService,
    play_id: &str,
    reason: PlaySuspensionReason,
    message: &str,
) {
    let was_running = {
        let mut lm = lifecycle.write().expect("lifecycle lock poisoned");
        lm.suspend_play(play_id)
    };
    if was_running {
        record_suspension(node_service, play_id, reason, message).await;
    }
}

/// Write a suspension to the play node. A failed write is logged, not
/// propagated: the play is already out of the indexes for this run, and the
/// diagnostic itself is in the log either way.
async fn record_suspension(
    node_service: &NodeService,
    play_id: &str,
    reason: PlaySuspensionReason,
    message: &str,
) {
    if let Err(e) = node_service
        .record_play_suspension(play_id, reason, message)
        .await
    {
        error!(
            play_id = %play_id,
            reason = %reason,
            error = %e,
            "Failed to record a play's suspension on its node"
        );
    }
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
/// offending plays are suspended, and the limit breach is logged.
///
/// A rule runs only while its play does: a rule of a play that was switched
/// off, archived, suspended or deleted since the work item was queued is
/// skipped, which is also what stops a play's later rules in the same work
/// item once one of its actions fails.
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
                warn!(
                    play_id = %rule_ref.play_id,
                    rule = %rule_ref.rule.name,
                    rule_index = rule_ref.rule_index,
                    trigger_node_id = %work_item.trigger_node.id,
                    error_type = "cycle_limit",
                    max_chain_depth = MAX_CHAIN_DEPTH,
                    "Cycle depth limit exceeded; play suspended"
                );

                // Suspend the play that would have fired
                suspend_play(
                    &lifecycle,
                    &node_service,
                    &rule_ref.play_id,
                    PlaySuspensionReason::CycleLimit,
                    &format!(
                        "Rule '{}' reached the cycle depth limit ({MAX_CHAIN_DEPTH}) on node {}",
                        rule_ref.rule.name, work_item.trigger_node.id
                    ),
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
        // A scheduled scan already resolved its rules' condition paths for
        // every node it selected, in one statement per path. This node's
        // share of that is where its conditions start.
        let scanned_type = work_item
            .scan
            .as_ref()
            .map(|scan| scan.target_type.as_str());
        if let Some(scan) = &work_item.scan {
            resolver.seed(&work_item.trigger_node.id, &scan.paths);
        }

        // Process each matched rule in order
        for rule_ref in &work_item.rules {
            // The rule was matched when the work item was queued. Its play
            // may have gone out of service since: switched off, archived or
            // deleted by the user, or suspended by an earlier rule's failure
            // in this same work item.
            let running = {
                let lm = lifecycle.read().expect("lifecycle lock poisoned");
                lm.is_running(&rule_ref.play_id)
            };
            if !running {
                debug!(
                    "Skipping rule '{}': play {} is no longer running",
                    rule_ref.rule.name, rule_ref.play_id,
                );
                continue;
            }

            debug!(
                "Processing rule '{}' from play {} (index {})",
                rule_ref.rule.name, rule_ref.play_id, rule_ref.rule_index,
            );

            // Evaluate at the rule's registered trigger scope (ADR-078), so
            // a Play on a base type sees that type's fields and vocabulary
            // whatever concrete subtype fired it.
            let cel_scope = match PlaybookEngine::cel_scope_for(
                &node_service,
                &rule_ref.rule,
                &work_item.trigger_node,
                scanned_type,
            )
            .await
            {
                Ok(scope) => scope,
                Err(e) => {
                    // A resolver DB error is distinct from "nothing to
                    // scope" (see `cel_scope_for`'s doc) and must not be
                    // treated as the latter: evaluating this rule's
                    // conditions against the trigger node's raw,
                    // unprojected properties would read the wrong bucket
                    // and silently fail to match. Skip this rule for this
                    // event rather than misevaluate it -- the play stays
                    // running and gets another chance on the next matching
                    // event, unlike the cycle-limit/action-failure cases
                    // below, which suspend the play outright because they
                    // reflect an actual problem with the play itself
                    // rather than a transient resolver failure.
                    warn!(
                        play_id = %rule_ref.play_id,
                        rule = %rule_ref.rule.name,
                        rule_index = rule_ref.rule_index,
                        trigger_node_id = %work_item.trigger_node.id,
                        error_type = "scope_resolution_failed",
                        error = %e,
                        "Failed to resolve CEL scope for rule; skipping this rule for this event"
                    );
                    continue;
                }
            };
            // Each rule in this work item carries its own registered scope, so
            // the shared resolver is re-pointed per rule rather than per item.
            resolver.set_reading_type(PlaybookEngine::reading_type(&rule_ref.rule, scanned_type));
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
                crate::playbook::cel::ConditionResult::Unresolved { reason } => {
                    // Same posture as the scope-resolution failure above: a
                    // failed graph lookup says nothing about the conditions,
                    // so neither fire nor suspend -- skip this event and let
                    // the next matching one re-evaluate.
                    warn!(
                        play_id = %rule_ref.play_id,
                        rule = %rule_ref.rule.name,
                        rule_index = rule_ref.rule_index,
                        trigger_node_id = %work_item.trigger_node.id,
                        error_type = "condition_resolution_failed",
                        error = %reason,
                        "Failed to resolve a condition path for rule; skipping this rule for this event"
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
                // An invariant rule of another Play vetoed this rule's
                // write. That is the graph declining the action, not this
                // rule failing: a roll-up that would complete a parent whose
                // checklist is unfinished is refused each time, and stays
                // ready for the parent it may complete. The Play keeps
                // running.
                //
                // Logged as a warning with the refusing rule: a rule whose
                // write can never be accepted is declined on every firing,
                // and this line is what makes it findable.
                crate::playbook::actions::ActionResult::Failed(
                    crate::playbook::actions::ActionError::RefusedByInvariant {
                        message,
                        refused_by_play,
                        refused_by_rule,
                        ..
                    },
                ) => {
                    warn!(
                        play_id = %rule_ref.play_id,
                        rule = %rule_ref.rule.name,
                        rule_index = rule_ref.rule_index,
                        trigger_node_id = %work_item.trigger_node.id,
                        error_type = "refused_by_invariant",
                        refused_by_play = %refused_by_play,
                        refused_by_rule = %refused_by_rule,
                        "Rule's write was refused by an invariant rule; the action is declined: {}",
                        message
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
                    // ALWAYS errors when reached, so this rule suspends
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
                             have been caught at save time. Suspending the play. Reject's own \
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
                    warn!(
                        play_id = %rule_ref.play_id,
                        rule = %rule_ref.rule.name,
                        rule_index = rule_ref.rule_index,
                        trigger_node_id = %work_item.trigger_node.id,
                        error_type = "action_error",
                        "{}",
                        log_message
                    );
                    // Suspend the play on action failure. Its remaining rules
                    // in this work item are skipped by the running check at
                    // the top of the loop.
                    suspend_play(
                        &lifecycle,
                        &node_service,
                        &rule_ref.play_id,
                        PlaySuspensionReason::ActionFailed,
                        &format!("Rule '{}': {log_message}", rule_ref.rule.name),
                    )
                    .await;
                }
            }
        }
    }

    info!("RuleProcessor shutting down (queue closed)");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// ADR-073 local-origin gate: true when `envelope` is a replicated apply (a
/// write applied from another device) rather than a mutation that originated
/// on this device.
///
/// Deliberately a denylist (match the reserved replicated-apply origin), not
/// an allowlist of known local client ids: local writes arrive tagged with many
/// different client ids (Tauri windows, MCP clients, CLI sessions, or none at
/// all), and enumerating them would be both impractical and the wrong
/// direction to fail in — an unrecognized *local* id would be silently
/// dropped instead of a genuinely replicated one slipping through. This
/// mirrors the existing `passes_origin_filter` precedent in
/// `services::node_service` (`excluded_event_origin`), which excludes by
/// origin match for the same reason.
pub(crate) fn is_replicated_apply(envelope: &EventEnvelope) -> bool {
    envelope.metadata.source_client_id.as_deref()
        == Some(crate::db::events::REPLICATED_APPLY_CLIENT_ID)
}

/// The chain depth to enforce `MAX_CHAIN_DEPTH` against for `work_item`
/// (ADR-060 §5).
///
/// Prefers the in-process `PlaybookExecutionContext` carried on the
/// triggering event — present whenever this hop's mutation was produced by
/// this same running process. Without it, the triggering write continues a
/// chain only if it was itself a play write, which `chain_depth_of_write`
/// decides from the per-write id the write stamped. A user's or MCP client's
/// edit never changes that id, so it starts a fresh chain at 0 rather than
/// continuing from whatever depth a play last left on the node — which
/// would otherwise let a user's own edit trip the cycle limit and disable
/// the play.
///
/// Defaults to 0 when neither source applies: a write that is not a play
/// hop, or a persisted depth `persisted_chain_depth` rejects as out of
/// range — indistinguishable here from a node never touched by a play,
/// which is a known, accepted limitation of a best-effort persisted signal
/// with no write protection (see `persisted_chain_depth`'s doc).
///
/// The in-process context is bounded to `0..=MAX_CHAIN_DEPTH` by
/// construction (only ever assigned `depth.saturating_add(1)` after
/// `exceeds_max_chain_depth` already passed), but the persisted property is
/// not similarly trustworthy — it is ordinary node data any
/// `create_node`/`update_node` caller can write — so it is bounded
/// explicitly against `MAX_CHAIN_DEPTH` here rather than trusting the
/// stored value's own range.
pub(crate) fn effective_chain_depth(work_item: &ExecutionWorkItem) -> u8 {
    work_item
        .trigger_event
        .metadata
        .playbook_context
        .as_ref()
        .map(|ctx| ctx.depth)
        .unwrap_or_else(|| {
            chain_depth_of_write(
                &work_item.trigger_event.event,
                &work_item.trigger_node.properties,
                MAX_CHAIN_DEPTH,
            )
            .unwrap_or(0)
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
/// Returns `None` for events that don't carry a node_id directly — a
/// relationship event's trigger node is its source node, resolved instead
/// via [`relationship_source_id`] plus a lookup, since `handle_event` needs
/// that lookup earlier anyway (to resolve the source's type for `TriggerKey`
/// matching) and reuses it here rather than fetching the same node twice.
pub(crate) fn trigger_node_id(event: &DomainEvent) -> Option<&str> {
    match event {
        DomainEvent::NodeCreated { node_id, .. } => Some(node_id.as_str()),
        DomainEvent::NodeUpdated { node_id, .. } => Some(node_id.as_str()),
        _ => None,
    }
}

/// The bare id of a relationship event's forward source node — the node
/// `relationship_added`/`relationship_removed` triggers match against (docs:
/// "the source node matches `node_type`") — or `None` for any other event
/// variant. `from_id` is stored in the `node:<id>`-prefixed form every
/// `RelationshipEvent`/`RelationshipDeleted` carries (`db::events::node_thing`);
/// `bare_node_id` strips it back to the form `NodeService::get_node` expects.
pub(crate) fn relationship_source_id(event: &DomainEvent) -> Option<&str> {
    match event {
        DomainEvent::RelationshipCreated { relationship } => {
            Some(crate::db::events::bare_node_id(&relationship.from_id))
        }
        DomainEvent::RelationshipDeleted { from_id, .. } => {
            Some(crate::db::events::bare_node_id(from_id))
        }
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
                    { "name": "severity", "type": "text", "protection": "user", "indexed": false }
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
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": node_type } },
            "conditions": [{ "expr": condition, "description": "Test condition" }],
            "actions": []
        }))
        .expect("rule definition should parse");
        parse_rule(&def).expect("rule should compile")
    }

    async fn eval(svc: &Arc<NodeService>, rule: &ParsedRule, node: &crate::models::Node) -> bool {
        let scope = PlaybookEngine::cel_scope_for(svc, rule, node, None)
            .await
            .expect("scope resolution should not fail against a healthy store");
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

    async fn make_ticket(svc: &Arc<NodeService>) -> crate::models::Node {
        let id = svc
            .create_node(crate::models::Node::new(
                "ticket".to_string(),
                "a ticket".to_string(),
                json!({ "state": "open" }),
            ))
            .await
            .expect("ticket creation failed");
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

    /// `ticket.state` is a plain enum `bug` inherits without extending, so a
    /// bug's `state` stays in the `ticket` bucket — unlike [`seed_chain`],
    /// where `add_field_values` materializes it onto `bug`'s own.
    async fn seed_inheriting_chain(svc: &Arc<NodeService>) {
        handle_create_schema(
            svc,
            json!({
                "name": "Ticket",
                "fields": [{
                    "name": "state",
                    "type": "enum",
                    "protection": "user",
                    "indexed": false,
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
            json!({ "name": "Bug", "extends": "ticket", "fields": [] }),
        )
        .await
        .expect("bug schema creation failed");
    }

    #[tokio::test]
    async fn own_scoped_condition_reads_an_inherited_field() {
        let (svc, _tmp) = test_service().await;
        seed_inheriting_chain(&svc).await;
        let done = make_bug(&svc, json!({ "state": "done" })).await;
        let open = make_bug(&svc, json!({ "state": "open" })).await;
        assert!(
            done.properties["ticket"]["state"] == "done",
            "precondition: the inherited field must live in the ancestor's bucket, got {}",
            done.properties
        );

        for trigger_type in ["bug", "*"] {
            let rule = rule_on(trigger_type, "node.state == 'done'");
            assert!(
                eval(&svc, &rule, &done).await,
                "a `{trigger_type}` Play must read a bug's inherited `state`"
            );
            assert!(
                !eval(&svc, &rule, &open).await,
                "a `{trigger_type}` Play must not match an open bug"
            );
        }
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

    /// `type_chain` is the node's own chain at whatever scope it is read, so
    /// a base-scoped Play can ask whether a node is the base or a subtype.
    #[tokio::test]
    async fn type_chain_reads_the_nodes_own_chain_at_every_scope() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;
        let bug = make_bug(&svc, json!({ "state": "open", "severity": "high" })).await;
        let ticket = make_ticket(&svc).await;

        for scope in ["ticket", "bug"] {
            for condition in [
                "'bug' in node.type_chain",
                "'ticket' in node.type_chain",
                "node.type_chain[0] == 'bug'",
            ] {
                assert!(
                    eval(&svc, &rule_on(scope, condition), &bug).await,
                    "{condition} at {scope} scope"
                );
            }
        }
        assert!(
            eval(
                &svc,
                &rule_on("ticket", "node.type_chain == ['ticket']"),
                &ticket
            )
            .await
        );
        assert!(
            !eval(
                &svc,
                &rule_on("ticket", "'bug' in node.type_chain"),
                &ticket
            )
            .await
        );
    }

    #[tokio::test]
    async fn an_unextended_type_gets_no_scope_at_all() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;
        let node = make_ticket(&svc).await;

        // A rule registered against the node's own type, when that type
        // extends nothing, needs no projection or resolution: its own bucket
        // is its whole view.
        let rule = rule_on("ticket", "node.state == 'open'");
        assert!(
            PlaybookEngine::cel_scope_for(&svc, &rule, &node, None)
                .await
                .expect("the node's-own-type short-circuit must not error")
                .is_none(),
            "a rule on the node's own type resolves no scope"
        );
    }

    /// A trigger that is not type-scoped at all (`node_type: "*"`) reads the
    /// node at its own type, so on an unextended type it needs no scope
    /// either — same short-circuit as the node's-own-type case above.
    #[tokio::test]
    async fn a_wildcard_trigger_gets_no_scope_at_all() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;
        let node = make_ticket(&svc).await;

        let rule = rule_on("*", "node.state == 'open'");
        assert!(
            PlaybookEngine::cel_scope_for(&svc, &rule, &node, None)
                .await
                .expect("the wildcard-trigger short-circuit must not error")
                .is_none(),
            "a trigger that is not type-scoped resolves no scope"
        );
    }

    /// A resolver DB error must surface as `Err`, never fold into the same
    /// `Ok(None)` the two tests above use for "nothing to scope" — see
    /// `cel_scope_for`'s doc comment. Forces a real failure (dropping the
    /// `relationship` table, same technique as
    /// `a_failed_refresh_keeps_the_previous_cache_and_marks_it_dirty` in
    /// `ancestry_cache_tests`) rather than injecting one, so this exercises
    /// the actual `Err` arm of `resolve_type_chain`'s `get_extends_parent_map`
    /// query, not a stand-in for it.
    #[tokio::test]
    async fn a_resolver_db_error_is_propagated_not_folded_into_none() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;
        let node = make_bug(&svc, json!({ "state": "backlog" })).await;

        // A base-scoped rule on a subtype node: past the "already the
        // registered type" and wildcard short-circuits, so this actually
        // reaches the resolver calls below.
        let rule = rule_on("ticket", "node.state == 'open'");

        svc.store()
            .write()
            .await
            .execute("DROP TABLE relationship", ())
            .await
            .expect("dropping the relationship table should succeed");

        let result = PlaybookEngine::cel_scope_for(&svc, &rule, &node, None).await;

        assert!(
            result.is_err(),
            "a resolver DB error must surface as Err, not silently collapse into \
             the same None the doc comment reserves for 'nothing to scope'"
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
        let scope = PlaybookEngine::cel_scope_for(&svc, &rule, &parent, None)
            .await
            .expect("scope resolution should not fail against a healthy store");
        let mut resolver = crate::playbook::graph_resolver::GraphResolver::new(Arc::clone(&svc))
            .with_reading_type(PlaybookEngine::reading_type(&rule, None));
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

    /// Evaluate `rule` against `node` with a graph resolver, as dispatch does.
    async fn eval_resolved(
        svc: &Arc<NodeService>,
        rule: &ParsedRule,
        node: &crate::models::Node,
    ) -> bool {
        let scope = PlaybookEngine::cel_scope_for(svc, rule, node, None)
            .await
            .expect("scope resolution should not fail against a healthy store");
        let mut resolver = crate::playbook::graph_resolver::GraphResolver::new(Arc::clone(svc))
            .with_reading_type(PlaybookEngine::reading_type(rule, None));
        let event = DomainEvent::NodeCreated {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
        };
        matches!(
            evaluate_conditions_at_scope(
                &rule.conditions,
                node,
                &event,
                Some(&mut resolver),
                scope.as_ref(),
            )
            .await,
            ConditionResult::Pass
        )
    }

    async fn make_text(svc: &Arc<NodeService>) -> crate::models::Node {
        let id = svc
            .create_node(crate::models::Node::new(
                "text".to_string(),
                "a note".to_string(),
                json!({}),
            ))
            .await
            .expect("text creation failed");
        svc.get_node(&id)
            .await
            .expect("get_node failed")
            .expect("node should exist")
    }

    /// A related subtype node reached from a Play on an unrelated type is read
    /// at its own type — with its inherited fields, which live in its
    /// ancestor's bucket — not narrowed to its own bucket alone.
    #[tokio::test]
    async fn a_related_subtype_collection_reads_inherited_fields() {
        let (svc, _tmp) = test_service().await;
        seed_inheriting_chain(&svc).await;

        let parent = make_text(&svc).await;
        let first = make_bug(&svc, json!({ "state": "done" })).await;
        assert!(
            first.properties["ticket"]["state"] == "done",
            "precondition: the inherited field must live in the ancestor's bucket, got {}",
            first.properties
        );
        svc.create_relationship(&parent.id, "has_child", &first.id, json!({}))
            .await
            .expect("relationship creation failed");

        let rule = rule_on("text", "node.has_child.all(c, c.state == 'done')");
        assert!(
            eval_resolved(&svc, &rule, &parent).await,
            "a done bug child must read as done"
        );

        let second = make_bug(&svc, json!({ "state": "open" })).await;
        svc.create_relationship(&parent.id, "has_child", &second.id, json!({}))
            .await
            .expect("relationship creation failed");
        assert!(
            !eval_resolved(&svc, &rule, &parent).await,
            "an open bug child must fail the `.all`"
        );
    }

    /// The scalar half: a dot-path walked to a related subtype node reads an
    /// inherited field of it.
    #[tokio::test]
    async fn a_related_subtype_scalar_path_reads_an_inherited_field() {
        let (svc, _tmp) = test_service().await;
        seed_inheriting_chain(&svc).await;

        let done = make_bug(&svc, json!({ "state": "done" })).await;
        let open = make_bug(&svc, json!({ "state": "open" })).await;
        assert!(
            done.properties["ticket"]["state"] == "done",
            "precondition: the inherited field must live in the ancestor's bucket, got {}",
            done.properties
        );
        let rule = rule_on("text", "node.child_of.state == 'done'");

        for (parent, expected) in [(&done, true), (&open, false)] {
            let child = make_text(&svc).await;
            svc.create_relationship(&parent.id, "has_child", &child.id, json!({}))
                .await
                .expect("relationship creation failed");
            assert_eq!(
                eval_resolved(&svc, &rule, &child).await,
                expected,
                "parent properties were {}",
                parent.properties
            );
        }
    }

    /// A scalar path into a related subtype node reads it exactly as the node
    /// itself is read at the rule's type: `maps_to`-resolved and projected.
    /// `node.child_of.state` and `node.child_of` must agree about one node.
    #[tokio::test]
    async fn a_scalar_path_into_a_related_subtype_is_read_at_the_rules_type() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;

        let parent = make_bug(&svc, json!({ "state": "backlog", "severity": "high" })).await;
        let child = make_ticket(&svc).await;
        svc.create_relationship(&parent.id, "has_child", &child.id, json!({}))
            .await
            .expect("relationship creation failed");

        assert!(
            eval_resolved(
                &svc,
                &rule_on("ticket", "node.child_of.state == 'open'"),
                &child
            )
            .await,
            "a `backlog` parent reads as `open` at ticket scope"
        );
        assert!(
            !eval_resolved(
                &svc,
                &rule_on("ticket", "node.child_of.state == 'backlog'"),
                &child
            )
            .await,
            "the raw extended value must not reach a ticket-scoped condition"
        );
        assert!(
            !eval_resolved(
                &svc,
                &rule_on("ticket", "node.child_of.severity == 'high'"),
                &child
            )
            .await,
            "a bug-only field must not resolve at ticket scope"
        );
    }

    /// A related subtype is read at the rule's registered type whatever fired
    /// it: a Play on `ticket` fired by a plain ticket — which needs no scope of
    /// its own — still reads a `bug` child through ticket's vocabulary.
    #[tokio::test]
    async fn a_related_subtype_is_read_at_the_rules_type_not_the_triggers_scope() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;

        let parent = make_ticket(&svc).await;
        let child = make_bug(&svc, json!({ "state": "backlog" })).await;
        svc.create_relationship(&parent.id, "has_child", &child.id, json!({}))
            .await
            .expect("relationship creation failed");

        let rule = rule_on("ticket", "node.has_child.all(c, c.state == 'open')");
        assert!(
            PlaybookEngine::cel_scope_for(&svc, &rule, &parent, None)
                .await
                .expect("scope resolution should not fail")
                .is_none(),
            "precondition: the trigger itself needs no scope"
        );
        assert!(
            eval_resolved(&svc, &rule, &parent).await,
            "a `backlog` child reads as `open` at ticket scope"
        );
    }

    /// A wildcard rule has no vocabulary of its own, so a related node reads
    /// at its own type — never projected to whatever type fired the rule.
    #[tokio::test]
    async fn a_wildcard_rule_reads_related_nodes_at_their_own_type() {
        let (svc, _tmp) = test_service().await;
        seed_chain(&svc).await;

        let parent = make_ticket(&svc).await;
        let child = make_bug(&svc, json!({ "state": "backlog", "severity": "high" })).await;
        svc.create_relationship(&parent.id, "has_child", &child.id, json!({}))
            .await
            .expect("relationship creation failed");

        let rule = rule_on(
            "*",
            "node.has_child.all(c, c.state == 'backlog' && c.severity == 'high')",
        );
        assert_eq!(PlaybookEngine::reading_type(&rule, None), None);
        assert_eq!(
            PlaybookEngine::reading_type(&rule_on("ticket", "true"), None),
            Some("ticket".to_string())
        );
        assert!(
            eval_resolved(&svc, &rule, &parent).await,
            "a wildcard rule must see the child's own value and own field"
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
        let scope = PlaybookEngine::cel_scope_for(&svc, &rule, &parent, None)
            .await
            .expect("scope resolution should not fail against a healthy store");
        let mut resolver = crate::playbook::graph_resolver::GraphResolver::new(Arc::clone(&svc))
            .with_reading_type(PlaybookEngine::reading_type(&rule, None));
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
                    { "name": "state", "type": "text", "protection": "user", "indexed": false }
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
                    { "name": "severity", "type": "text", "protection": "user", "indexed": false }
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

    /// Call site 1 — engine startup (`load_plays`).
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

        engine.load_plays().await.expect("load_plays failed");

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
                    { "name": "note", "type": "text", "protection": "user", "indexed": false }
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

/// When a play runs (ADR-087 §5): the user's `enabled` switch, the engine's
/// suspensions, and the one check that reads both.
///
/// These drive the engine's handlers and the rule processor directly, against
/// a real `NodeService`, so each state change is observed without waiting on
/// the event loop.
#[cfg(test)]
mod run_state_tests {
    use super::*;
    use crate::db::events::{EventMetadata, PlaybookExecutionContext};
    use crate::db::SqliteStore;
    use crate::models::{Node, NodeUpdate, PlayFields, PlayNodeUpdate};
    use crate::schema::{handle_create_schema, handle_update_schema};
    use serde_json::{json, Value};
    use tempfile::TempDir;

    const WIDGET: &str = "widget";

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
        handle_create_schema(
            &node_service,
            json!({
                "name": "Widget",
                "fields": [
                    { "name": "state", "type": "text", "protection": "user", "indexed": false },
                    { "name": "marker", "type": "text", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .expect("widget schema creation failed");
        let engine = PlaybookEngine::new(Arc::clone(&node_service));
        (engine, node_service, temp_dir)
    }

    /// One rule: when a widget is created, stamp `marker` on it.
    fn marking_rule(name: &str) -> Value {
        json!({
            "name": name,
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": WIDGET } },
            "conditions": [],
            "actions": [{
                "description": "Test action",
                "action_type": "update_node",
                "params": { "node_id": "{trigger.node.id}", "properties": { "marker": "set" } }
            }]
        })
    }

    /// One rule whose action always fails: it updates a node that is not there.
    fn failing_rule(name: &str) -> Value {
        json!({
            "name": name,
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": WIDGET } },
            "conditions": [],
            "actions": [{
                "description": "Test action",
                "action_type": "update_node",
                "params": { "node_id": "no-such-node", "properties": { "marker": "set" } }
            }]
        })
    }

    /// Rules that decode but never validate: `reject` is for invariant rules.
    fn invalid_rules() -> Value {
        json!([{
            "name": "reject-on-reactive",
            "class": "reactive",
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": WIDGET } },
            "conditions": [],
            "actions": [{ "description": "Test action", "action_type": "reject", "params": { "message": "no" } }]
        }])
    }

    /// Create a play through the service and tell the engine about it.
    async fn install_play(engine: &PlaybookEngine, svc: &NodeService, play: Value) -> String {
        let node = Node::new("play".to_string(), "A play".to_string(), play);
        let id = node.id.clone();
        svc.create_node(node).await.expect("play creation failed");
        engine.handle_play_created(&id).await;
        id
    }

    /// Store a play with invalid rules around the service's save-time gate, as
    /// a row written by another device or an earlier build would be.
    async fn store_invalid_play(svc: &NodeService) -> String {
        let node = Node::new(
            "play".to_string(),
            "A broken play".to_string(),
            json!({ "play": { "rules": invalid_rules() } }),
        );
        let id = node.id.clone();
        svc.store()
            .create_node(node, None, None)
            .await
            .expect("direct play insert failed");
        id
    }

    async fn play_node(svc: &NodeService, id: &str) -> Node {
        svc.get_node(id)
            .await
            .expect("get_node failed")
            .expect("the play exists")
    }

    async fn update_play(
        svc: &NodeService,
        id: &str,
        update: Value,
    ) -> Result<Node, NodeServiceError> {
        let update: PlayNodeUpdate = serde_json::from_value(update).expect("a PlayNodeUpdate");
        let version = play_node(svc, id).await.version;
        svc.update_play_node(id, version, update).await
    }

    fn status(engine: &PlaybookEngine, id: &str) -> Option<PlayStatus> {
        let lifecycle = engine.lifecycle.read().expect("lifecycle lock poisoned");
        lifecycle.get_play(id).map(|play| play.status)
    }

    /// The rules the engine would fire for a newly created widget.
    fn rules_for_a_new_widget(engine: &PlaybookEngine) -> Vec<OrderedRuleRef> {
        let lifecycle = engine.lifecycle.read().expect("lifecycle lock poisoned");
        lifecycle.lookup_rules(&[TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: WIDGET.to_string(),
            property_key: None,
        }])
    }

    fn suspension(node: &Node) -> (Option<String>, Option<String>, Option<String>) {
        let field = |key: &str| {
            PlayFields::stored_field(&node.properties, key)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        (
            field("suspended_reason"),
            field("suspended_message"),
            field("suspended_at"),
        )
    }

    async fn create_widget(svc: &NodeService) -> Node {
        let node = Node::new(WIDGET.to_string(), "a widget".to_string(), json!({}));
        let id = svc.create_node(node).await.expect("widget creation failed");
        svc.get_node(&id).await.unwrap().unwrap()
    }

    fn marker(node: &Node) -> Option<&Value> {
        node.properties
            .get(WIDGET)
            .and_then(|bucket| bucket.get("marker"))
    }

    /// Run the rule processor over one work item for `widget`, to completion.
    async fn process(
        engine: &PlaybookEngine,
        svc: &Arc<NodeService>,
        rules: Vec<OrderedRuleRef>,
        widget: &Node,
        playbook_context: Option<PlaybookExecutionContext>,
    ) {
        let (tx, rx) = mpsc::channel::<ExecutionWorkItem>(4);
        tx.send(ExecutionWorkItem {
            rules,
            trigger_event: EventEnvelope {
                event: DomainEvent::NodeCreated {
                    node_id: widget.id.clone(),
                    node_type: widget.node_type.clone(),
                },
                metadata: EventMetadata {
                    source_client_id: None,
                    playbook_context,
                },
            },
            trigger_node: widget.clone(),
            scan: None,
        })
        .await
        .expect("queue send failed");
        drop(tx);
        rule_processor_loop(rx, Arc::clone(&engine.lifecycle), Arc::clone(svc)).await;
    }

    #[tokio::test]
    async fn a_runnable_play_is_indexed_and_fires() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;
        assert_eq!(status(&engine, &id), Some(PlayStatus::Runnable));

        let widget = create_widget(&svc).await;
        process(
            &engine,
            &svc,
            rules_for_a_new_widget(&engine),
            &widget,
            None,
        )
        .await;
        assert_eq!(
            marker(&play_node(&svc, &widget.id).await),
            Some(&json!("set"))
        );
    }

    /// The user's switch: a disabled play is known, with its reason, and has
    /// no rules in the index. Switching it on indexes it.
    #[tokio::test]
    async fn a_disabled_play_does_not_run_until_it_is_enabled() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(
            &engine,
            &svc,
            json!({ "rules": [marking_rule("mark")], "enabled": false }),
        )
        .await;
        assert_eq!(status(&engine, &id), Some(PlayStatus::Disabled));
        assert!(rules_for_a_new_widget(&engine).is_empty());

        update_play(&svc, &id, json!({ "enabled": true }))
            .await
            .unwrap();
        engine.handle_play_updated(&id).await;
        assert_eq!(status(&engine, &id), Some(PlayStatus::Runnable));
        assert_eq!(rules_for_a_new_widget(&engine).len(), 1);

        update_play(&svc, &id, json!({ "enabled": false }))
            .await
            .unwrap();
        engine.handle_play_updated(&id).await;
        assert_eq!(status(&engine, &id), Some(PlayStatus::Disabled));
        assert!(rules_for_a_new_widget(&engine).is_empty());
    }

    /// A rule matched before its play was switched off does not run: the
    /// processor asks again when it reaches the rule.
    #[tokio::test]
    async fn a_queued_rule_of_a_play_disabled_since_does_not_run() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;
        let widget = create_widget(&svc).await;
        let queued = rules_for_a_new_widget(&engine);
        assert_eq!(queued.len(), 1);

        update_play(&svc, &id, json!({ "enabled": false }))
            .await
            .unwrap();
        engine.handle_play_updated(&id).await;

        process(&engine, &svc, queued, &widget, None).await;
        assert_eq!(marker(&play_node(&svc, &widget.id).await), None);
    }

    /// Archiving is governance, not the play's switch: an archived play does
    /// not run, its `enabled` stays as the user left it, and a restart does
    /// not bring it back.
    #[tokio::test]
    async fn an_archived_play_does_not_run() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;

        let version = play_node(&svc, &id).await.version;
        svc.update_node(
            &id,
            version,
            NodeUpdate::default().with_lifecycle_status("archived".to_string()),
        )
        .await
        .unwrap();
        engine.handle_play_updated(&id).await;

        assert_eq!(status(&engine, &id), Some(PlayStatus::Archived));
        assert!(rules_for_a_new_widget(&engine).is_empty());
        assert!(PlayFields::enabled_in(
            &play_node(&svc, &id).await.properties
        ));

        let restarted = PlaybookEngine::new(Arc::clone(&svc));
        restarted.load_plays().await.unwrap();
        assert!(rules_for_a_new_widget(&restarted).is_empty());
    }

    /// A play created with rules that do not validate is suspended, with the
    /// reason on its node, rather than left unknown to the engine.
    #[tokio::test]
    async fn a_play_created_with_invalid_rules_is_suspended() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = store_invalid_play(&svc).await;
        let version = play_node(&svc, &id).await.version;

        engine.handle_play_created(&id).await;

        assert_eq!(status(&engine, &id), Some(PlayStatus::Suspended));
        assert!(rules_for_a_new_widget(&engine).is_empty());
        let node = play_node(&svc, &id).await;
        let (reason, message, at) = suspension(&node);
        assert_eq!(reason.as_deref(), Some("validation_failed"));
        assert!(message.is_some_and(|m| !m.is_empty()));
        assert!(at.is_some());
        assert!(
            PlayFields::enabled_in(&node.properties),
            "the switch is the user's"
        );
        assert_eq!(node.version, version, "a suspension bumps no version");
    }

    /// The same at startup: a play whose rules fail validation at load is
    /// suspended rather than silently skipped.
    #[tokio::test]
    async fn a_play_with_invalid_rules_is_suspended_at_load() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = store_invalid_play(&svc).await;

        engine.load_plays().await.unwrap();

        assert_eq!(status(&engine, &id), Some(PlayStatus::Suspended));
        assert_eq!(
            suspension(&play_node(&svc, &id).await).0.as_deref(),
            Some("validation_failed")
        );
    }

    /// Rules that stop parsing suspend a running play; the play is not
    /// dropped from the engine.
    #[tokio::test]
    async fn rules_that_no_longer_parse_suspend_the_play() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;

        // Written around the service, which would refuse it.
        svc.store()
            .set_property_strings(
                &id,
                &[("$.play.rules".to_string(), "not rules".to_string())],
            )
            .await
            .unwrap();
        engine.handle_play_updated(&id).await;

        assert_eq!(status(&engine, &id), Some(PlayStatus::Suspended));
        assert!(rules_for_a_new_widget(&engine).is_empty());
        let (reason, message, _) = suspension(&play_node(&svc, &id).await);
        assert_eq!(reason.as_deref(), Some("validation_failed"));
        assert!(message.unwrap().contains("parse"));
    }

    /// An action failure suspends the play: the failure is on the node, the
    /// play's later rules in the same work item do not run, and the user's
    /// switch is untouched.
    #[tokio::test]
    async fn an_action_failure_suspends_the_play_and_skips_its_later_rules() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(
            &engine,
            &svc,
            json!({ "rules": [failing_rule("fails"), marking_rule("mark")] }),
        )
        .await;
        let version = play_node(&svc, &id).await.version;
        let widget = create_widget(&svc).await;
        let rules = rules_for_a_new_widget(&engine);
        assert_eq!(rules.len(), 2);

        process(&engine, &svc, rules, &widget, None).await;

        assert_eq!(
            marker(&play_node(&svc, &widget.id).await),
            None,
            "the play's later rule must not run in the same work item"
        );
        assert_eq!(status(&engine, &id), Some(PlayStatus::Suspended));
        assert!(rules_for_a_new_widget(&engine).is_empty());
        let node = play_node(&svc, &id).await;
        let (reason, message, at) = suspension(&node);
        assert_eq!(reason.as_deref(), Some("action_failed"));
        assert!(
            message.unwrap().contains("fails"),
            "the message names the rule"
        );
        assert!(at.is_some());
        assert!(PlayFields::enabled_in(&node.properties));
        assert_eq!(node.version, version);
    }

    /// Another play's rules in the same work item still run.
    #[tokio::test]
    async fn one_plays_failure_does_not_stop_another_plays_rules() {
        let (engine, svc, _tmp) = test_engine().await;
        let failing =
            install_play(&engine, &svc, json!({ "rules": [failing_rule("fails")] })).await;
        let marking = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;
        let widget = create_widget(&svc).await;

        process(
            &engine,
            &svc,
            rules_for_a_new_widget(&engine),
            &widget,
            None,
        )
        .await;

        assert_eq!(status(&engine, &failing), Some(PlayStatus::Suspended));
        assert_eq!(status(&engine, &marking), Some(PlayStatus::Runnable));
        assert_eq!(
            marker(&play_node(&svc, &widget.id).await),
            Some(&json!("set"))
        );
    }

    #[tokio::test]
    async fn the_cycle_limit_suspends_the_play() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;
        let widget = create_widget(&svc).await;

        process(
            &engine,
            &svc,
            rules_for_a_new_widget(&engine),
            &widget,
            Some(PlaybookExecutionContext {
                originating_event_id: "evt".to_string(),
                depth: MAX_CHAIN_DEPTH,
                source_playbook_id: id.clone(),
            }),
        )
        .await;

        assert_eq!(marker(&play_node(&svc, &widget.id).await), None);
        assert_eq!(status(&engine, &id), Some(PlayStatus::Suspended));
        assert_eq!(
            suspension(&play_node(&svc, &id).await).0.as_deref(),
            Some("cycle_limit")
        );
    }

    /// A schema change that breaks a play's rules suspends it with
    /// `schema_drift`; one that does not break them leaves it running.
    #[tokio::test]
    async fn a_schema_change_that_breaks_the_rules_suspends_the_play() {
        let (engine, svc, _tmp) = test_engine().await;
        // The rule pins the schema version it was written against.
        let rule = json!({
            "name": "pinned",
            "description": "Test rule",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": WIDGET } },
            "conditions": [{ "expr": "node.state == 'ready'", "description": "Test condition" }],
            "actions": [{
                "description": "Test action",
                "action_type": "create_node",
                "params": { "node_type": WIDGET, "version": 1, "content": "another" }
            }]
        });
        let id = install_play(&engine, &svc, json!({ "rules": [rule] })).await;
        assert_eq!(status(&engine, &id), Some(PlayStatus::Runnable));

        // An additive change: the play still validates.
        handle_update_schema(
            &svc,
            json!({
                "schema_id": WIDGET,
                "add_fields": [{ "name": "colour", "type": "text", "indexed": false }]
            }),
        )
        .await
        .expect("schema update failed");
        engine.handle_schema_updated(WIDGET).await;
        assert_eq!(status(&engine, &id), Some(PlayStatus::Runnable));
        assert_eq!(suspension(&play_node(&svc, &id).await), (None, None, None));

        // The schema moves to a version the rule was not written against.
        let schema = play_node(&svc, WIDGET).await;
        svc.update_node(
            WIDGET,
            schema.version,
            NodeUpdate::default().with_properties(json!({ "schemaVersion": 2 })),
        )
        .await
        .expect("schema version bump failed");
        engine.handle_schema_updated(WIDGET).await;

        assert_eq!(status(&engine, &id), Some(PlayStatus::Suspended));
        assert!(rules_for_a_new_widget(&engine).is_empty());
        let (reason, message, _) = suspension(&play_node(&svc, &id).await);
        assert_eq!(reason.as_deref(), Some("schema_drift"));
        assert!(message.unwrap().contains(WIDGET));
    }

    /// A suspension is on the node, so a restart does not undo it, and it is
    /// recorded once however many times the engine reads the play.
    #[tokio::test]
    async fn a_suspension_survives_a_restart_and_leaves_the_switch_on() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;
        suspend_play(
            &engine.lifecycle,
            &svc,
            &id,
            PlaySuspensionReason::ActionFailed,
            "boom",
        )
        .await;
        let recorded = suspension(&play_node(&svc, &id).await);
        assert_eq!(recorded.0.as_deref(), Some("action_failed"));
        assert_eq!(recorded.1.as_deref(), Some("boom"));

        let restarted = PlaybookEngine::new(Arc::clone(&svc));
        restarted.load_plays().await.unwrap();

        assert_eq!(status(&restarted, &id), Some(PlayStatus::Suspended));
        assert!(rules_for_a_new_widget(&restarted).is_empty());
        let node = play_node(&svc, &id).await;
        assert_eq!(
            suspension(&node),
            recorded,
            "the suspension is not rewritten"
        );
        assert!(PlayFields::enabled_in(&node.properties));

        // The engine's own write arrives as an update and changes nothing.
        restarted.handle_play_updated(&id).await;
        assert_eq!(status(&restarted, &id), Some(PlayStatus::Suspended));
        assert_eq!(suspension(&play_node(&svc, &id).await), recorded);
    }

    /// The suspension write is seen by watchers as a node update.
    #[tokio::test]
    async fn recording_a_suspension_emits_a_node_updated_event() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;
        let mut events = svc.subscribe_to_events();

        suspend_play(
            &engine.lifecycle,
            &svc,
            &id,
            PlaySuspensionReason::ActionFailed,
            "boom",
        )
        .await;

        let envelope = events.try_recv().expect("a node-updated event");
        match envelope.event {
            DomainEvent::NodeUpdated {
                node_id,
                node,
                changed_properties,
                ..
            } => {
                assert_eq!(node_id, id);
                assert!(PlayFields::suspended_in(&node.properties));
                assert!(!changed_properties.is_empty());
            }
            other => panic!("expected NodeUpdated, got {other:?}"),
        }
    }

    /// Setting `enabled` to `true`, even when it already is, clears a
    /// suspension; the engine re-validates and the play runs again.
    #[tokio::test]
    async fn enabling_a_suspended_play_clears_the_suspension() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;
        suspend_play(
            &engine.lifecycle,
            &svc,
            &id,
            PlaySuspensionReason::ActionFailed,
            "boom",
        )
        .await;
        engine.handle_play_updated(&id).await;
        assert_eq!(status(&engine, &id), Some(PlayStatus::Suspended));

        // Another field's edit leaves the suspension in place.
        update_play(&svc, &id, json!({ "description": "still broken" }))
            .await
            .unwrap();
        engine.handle_play_updated(&id).await;
        assert_eq!(status(&engine, &id), Some(PlayStatus::Suspended));

        let enabled = update_play(&svc, &id, json!({ "enabled": true }))
            .await
            .unwrap();
        assert_eq!(suspension(&enabled), (None, None, None));
        engine.handle_play_updated(&id).await;

        assert_eq!(status(&engine, &id), Some(PlayStatus::Runnable));
        assert_eq!(rules_for_a_new_widget(&engine).len(), 1);
    }

    /// Enabling a play whose problem remains suspends it again, with a new
    /// diagnostic.
    #[tokio::test]
    async fn enabling_a_play_that_is_still_broken_suspends_it_again() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = store_invalid_play(&svc).await;
        engine.handle_play_created(&id).await;
        let first = suspension(&play_node(&svc, &id).await);

        let enabled = update_play(&svc, &id, json!({ "enabled": true }))
            .await
            .unwrap();
        assert_eq!(suspension(&enabled), (None, None, None));
        engine.handle_play_updated(&id).await;

        assert_eq!(status(&engine, &id), Some(PlayStatus::Suspended));
        let second = suspension(&play_node(&svc, &id).await);
        assert_eq!(second.0.as_deref(), Some("validation_failed"));
        assert_ne!(second.2, first.2, "the suspension is recorded afresh");
    }

    /// Saving fixed rules clears a suspension.
    #[tokio::test]
    async fn saving_fixed_rules_clears_a_suspension() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = store_invalid_play(&svc).await;
        engine.handle_play_created(&id).await;
        assert_eq!(status(&engine, &id), Some(PlayStatus::Suspended));

        let fixed = update_play(&svc, &id, json!({ "rules": [marking_rule("mark")] }))
            .await
            .unwrap();
        assert_eq!(suspension(&fixed), (None, None, None));
        engine.handle_play_updated(&id).await;

        assert_eq!(status(&engine, &id), Some(PlayStatus::Runnable));
        assert_eq!(rules_for_a_new_widget(&engine).len(), 1);
    }

    /// Rules are validated only when they change, so a write of the switch or
    /// the description succeeds for a play whose rules no longer validate.
    #[tokio::test]
    async fn a_switch_only_write_succeeds_for_a_play_with_invalid_rules() {
        let (_engine, svc, _tmp) = test_engine().await;
        let id = store_invalid_play(&svc).await;

        let off = update_play(&svc, &id, json!({ "enabled": false }))
            .await
            .expect("switching a play off validates only the switch");
        assert!(!PlayFields::enabled_in(&off.properties));
        update_play(&svc, &id, json!({ "description": "to be fixed" }))
            .await
            .expect("a description edit does not validate the rules");
        update_play(&svc, &id, json!({ "enabled": true }))
            .await
            .expect("switching it back on does not validate the rules either");

        // Changing the rules does validate them.
        let mut still_invalid = invalid_rules();
        still_invalid[0]["name"] = json!("renamed");
        let err = update_play(&svc, &id, json!({ "rules": still_invalid }))
            .await
            .expect_err("new rules are validated");
        assert!(
            matches!(err, NodeServiceError::PlayValidationFailed { .. }),
            "{err}"
        );
    }

    /// A rule matched before its play was deleted does not run either.
    #[tokio::test]
    async fn a_queued_rule_of_a_play_deleted_since_does_not_run() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;
        let widget = create_widget(&svc).await;
        let queued = rules_for_a_new_widget(&engine);
        assert_eq!(queued.len(), 1);

        engine.handle_play_deleted(&id);

        process(&engine, &svc, queued, &widget, None).await;
        assert_eq!(marker(&play_node(&svc, &widget.id).await), None);
    }

    /// A reload (the subscriber lagged and may have missed a play's event)
    /// rebuilds the run state from the nodes: a play switched off, archived
    /// or deleted in the gap stops running, and one created in it starts.
    #[tokio::test]
    async fn a_reload_rebuilds_the_run_state_from_the_nodes() {
        let (engine, svc, _tmp) = test_engine().await;
        let switched_off =
            install_play(&engine, &svc, json!({ "rules": [marking_rule("a")] })).await;
        let archived = install_play(&engine, &svc, json!({ "rules": [marking_rule("b")] })).await;
        let deleted = install_play(&engine, &svc, json!({ "rules": [marking_rule("c")] })).await;
        assert_eq!(rules_for_a_new_widget(&engine).len(), 3);

        // Everything below happens without the engine hearing of it.
        update_play(&svc, &switched_off, json!({ "enabled": false }))
            .await
            .unwrap();
        let version = play_node(&svc, &archived).await.version;
        svc.update_node(
            &archived,
            version,
            NodeUpdate::default().with_lifecycle_status("archived".to_string()),
        )
        .await
        .unwrap();
        let version = play_node(&svc, &deleted).await.version;
        svc.delete_node(&deleted, version).await.unwrap();
        let created = Node::new(
            "play".to_string(),
            "Created in the gap".to_string(),
            json!({ "rules": [marking_rule("d")] }),
        );
        let created = svc.create_node(created).await.unwrap();

        engine.load_plays().await.unwrap();

        assert_eq!(status(&engine, &switched_off), Some(PlayStatus::Disabled));
        assert_eq!(status(&engine, &archived), None);
        assert_eq!(status(&engine, &deleted), None);
        assert_eq!(status(&engine, &created), Some(PlayStatus::Runnable));
        let running: Vec<String> = rules_for_a_new_widget(&engine)
            .into_iter()
            .map(|rule| rule.play_id)
            .collect();
        assert_eq!(running, vec![created]);
    }

    /// A play cannot be created already suspended.
    #[tokio::test]
    async fn a_play_cannot_be_created_with_a_suspension() {
        let (_engine, svc, _tmp) = test_engine().await;
        for play in [
            json!({ "rules": [], "suspended_at": "2026-10-02T10:00:00Z" }),
            json!({ "rules": [], "suspended_reason": "action_failed" }),
            json!({ "play": { "rules": [], "suspended_message": "x" } }),
        ] {
            let node = Node::new("play".to_string(), "A play".to_string(), play.clone());
            let err = svc
                .create_node(node.clone())
                .await
                .expect_err("a new play carries no suspension");
            assert!(err.to_string().contains("new play"), "{play}: {err}");

            let err = svc
                .bulk_create(vec![node])
                .await
                .expect_err("nor does one created in a batch");
            assert!(err.to_string().contains("new play"), "{play}: {err}");
        }
    }

    /// The suspension is settled on every update path, not only the
    /// version-checked one a client uses: the path a play's own `update_node`
    /// action takes, the unchecked one and the bulk one all refuse a write
    /// that changes a suspension field, and all clear one on `enabled: true`.
    #[tokio::test]
    async fn every_update_path_settles_the_suspension() {
        let (engine, svc, _tmp) = test_engine().await;
        let forge = json!({ "suspended_reason": "cycle_limit" });
        let enable = json!({ "enabled": true });

        // (path name, a closure-free dispatch by index)
        for path in ["unchecked", "in_tx", "bulk"] {
            let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;
            let write = |properties: Value| {
                let svc = Arc::clone(&svc);
                let id = id.clone();
                async move {
                    let update = NodeUpdate::default().with_properties(properties);
                    match path {
                        "unchecked" => svc.update_node_unchecked(&id, update).await.map(|_| ()),
                        "in_tx" => {
                            let inner = Arc::clone(&svc);
                            let id = id.clone();
                            svc.with_transaction(move |tx| {
                                Box::pin(async move {
                                    inner.update_node_in_tx(tx, &id, update).await.map(|_| ())
                                })
                            })
                            .await
                        }
                        _ => svc.bulk_update(vec![(id.clone(), update)]).await,
                    }
                }
            };

            let err = write(forge.clone())
                .await
                .expect_err("a suspension field is the engine's");
            assert!(
                err.to_string().contains("suspended_reason"),
                "{path}: {err}"
            );
            assert_eq!(
                suspension(&play_node(&svc, &id).await),
                (None, None, None),
                "{path}"
            );

            svc.record_play_suspension(&id, PlaySuspensionReason::ActionFailed, "boom")
                .await
                .unwrap();
            // Carrying the stored value back unchanged is not a change.
            write(json!({ "suspended_reason": "action_failed", "description": "kept" }))
                .await
                .unwrap_or_else(|e| panic!("{path}: an unchanged suspension field: {e}"));
            assert!(
                PlayFields::suspended_in(&play_node(&svc, &id).await.properties),
                "{path}"
            );

            write(enable.clone())
                .await
                .unwrap_or_else(|e| panic!("{path}: enabling: {e}"));
            assert_eq!(
                suspension(&play_node(&svc, &id).await),
                (None, None, None),
                "{path}: enabling clears the suspension"
            );
        }
    }

    /// A type extending `play` is a play: its rules are validated when they
    /// change, and saving fixed rules clears its suspension.
    #[tokio::test]
    async fn a_subtype_of_play_is_held_to_the_same_rules() {
        let (engine, svc, _tmp) = test_engine().await;
        handle_create_schema(
            &svc,
            json!({
                "name": "Subplay",
                "extends": "play",
                "fields": [
                    { "name": "owner", "type": "text", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .expect("a schema extending play");
        let node = Node::new(
            "subplay".to_string(),
            "A subplay".to_string(),
            json!({ "rules": [marking_rule("mark")], "owner": "ada" }),
        );
        let id = svc
            .create_node(node)
            .await
            .expect("subplay creation failed");
        engine.handle_play_created(&id).await;
        assert_eq!(status(&engine, &id), Some(PlayStatus::Runnable));

        // New rules that do not validate are refused, as for a plain play.
        let version = play_node(&svc, &id).await.version;
        let err = svc
            .update_node(
                &id,
                version,
                NodeUpdate::default().with_properties(json!({ "rules": invalid_rules() })),
            )
            .await
            .expect_err("a subtype's new rules are validated");
        assert!(
            matches!(err, NodeServiceError::PlayValidationFailed { .. }),
            "{err}"
        );

        // Saving different rules clears a suspension.
        svc.record_play_suspension(&id, PlaySuspensionReason::ActionFailed, "boom")
            .await
            .unwrap();
        let version = play_node(&svc, &id).await.version;
        let fixed = svc
            .update_node(
                &id,
                version,
                NodeUpdate::default().with_properties(json!({ "rules": [marking_rule("fixed")] })),
            )
            .await
            .unwrap();
        assert_eq!(suspension(&fixed), (None, None, None));
        assert_eq!(
            PlayFields::from_properties(&fixed.properties)
                .unwrap()
                .rules[0]
                .name,
            "fixed",
            "the rules live in the play bucket"
        );
    }

    /// The suspension fields are the engine's: no client write changes one,
    /// in either the flat or the bucketed shape, and that includes clearing
    /// one by hand instead of enabling the play.
    #[tokio::test]
    async fn a_client_cannot_write_a_suspension_field() {
        let (engine, svc, _tmp) = test_engine().await;
        let id = install_play(&engine, &svc, json!({ "rules": [marking_rule("mark")] })).await;
        let write = |patch: Value| {
            let svc = Arc::clone(&svc);
            let id = id.clone();
            async move {
                let version = play_node(&svc, &id).await.version;
                svc.update_node(&id, version, NodeUpdate::default().with_properties(patch))
                    .await
            }
        };

        for patch in [
            json!({ "suspended_reason": "action_failed" }),
            json!({ "suspended_at": "2026-10-02T10:00:00Z" }),
            json!({ "play": { "suspended_message": "x" } }),
        ] {
            let err = write(patch.clone())
                .await
                .expect_err("a suspension field is not client-writable");
            assert!(err.to_string().contains("suspended_"), "{patch}: {err}");
        }
        assert_eq!(suspension(&play_node(&svc, &id).await), (None, None, None));

        // Clearing a field that holds nothing changes nothing.
        write(json!({ "suspended_at": null })).await.unwrap();

        // Clearing a recorded suspension by hand is refused: enabling the
        // play is how it is cleared.
        svc.record_play_suspension(&id, PlaySuspensionReason::ActionFailed, "boom")
            .await
            .unwrap();
        let recorded = suspension(&play_node(&svc, &id).await);
        for patch in [
            json!({ "suspended_at": null }),
            json!({ "suspended_reason": null, "suspended_message": null, "suspended_at": null }),
            json!({ "suspended_message": "rewritten" }),
        ] {
            let err = write(patch.clone())
                .await
                .expect_err("a recorded suspension is not client-writable");
            assert!(err.to_string().contains("suspended_"), "{patch}: {err}");
        }
        assert_eq!(suspension(&play_node(&svc, &id).await), recorded);

        // A write that enables the play clears the suspension, so carrying
        // the fields as cleared alongside it is not a contradiction.
        let enabled = write(json!({
            "enabled": true,
            "suspended_reason": null,
            "suspended_message": null,
            "suspended_at": null
        }))
        .await
        .expect("enabling clears the suspension, with or without the nulls");
        assert_eq!(suspension(&enabled), (None, None, None));
    }
}
