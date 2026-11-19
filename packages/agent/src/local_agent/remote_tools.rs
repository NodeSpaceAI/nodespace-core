//! Remote tools: the gate that decides which of them the model may be offered
//! and may call, and the host that dispatches a call to the runner of its kind.
//!
//! A remote tool is a node of a subtype of `tool` that is not native
//! (`tool-mcp`, `tool-rest`, `tool-cli`, or a user type extending one). It is
//! offered to the model and called only when **all** of these hold, read from
//! this device's own node on every offering and every call:
//!
//! - its nearest core type is one of the three executable kinds (a bare
//!   `extends tool` subtype has nothing to run it);
//! - its name is a plain identifier and not a built-in tool's name;
//! - `enabled` is exactly `true` in the `tool` bucket (absent means off);
//! - `approved_binding` in the `tool` bucket equals the fingerprint of the
//!   node's binding as it is now (absent, or any difference, means off);
//! - its `parameter_schema` has a closed root;
//! - a runner for its kind is installed, so a tool is never offered that
//!   nothing would run;
//! - no other tool that passes the same gate has its name (both are dropped:
//!   a name two nodes share is never resolved by picking one).
//!
//! `enabled` and `approved_binding` are declared local-only, but this gate does
//! not depend on anything keeping a peer's value out: it reads what is stored on
//! this device and treats anything but a matching approval as off.
//!
//! A call is also validated against the tool's closed argument schema, confirmed
//! with the user when the kind needs it (every `tool-cli` and `tool-mcp` call,
//! every `tool-rest` call that is not a `GET`), and checked against the binding
//! again after the confirmation, so a binding changed while the question was
//! open is not run.
//!
//! The host runs nothing itself. The runners (one per kind) own the transport,
//! the credential injection and the redaction.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nodespace_core::behaviors::{tool_binding_hash, ToolOrigin};
use nodespace_core::models::{CoreNodeType, Node};
use nodespace_core::services::NodeService;
use nodespace_types::{check_closed_root, is_tool_name, MAX_RESPONSE_BYTES};
use serde_json::{json, Value};

use nodespace_nlp_engine::ToolDefinition;

use crate::agent_types::{AgentToolExecutor, SkillRetrieval, ToolError, ToolResult};
use crate::local_agent::tools::Tool;

/// Most arguments a call may carry.
pub const MAX_ARGUMENTS: usize = 64;
/// Longest single string argument, in bytes.
pub const MAX_ARGUMENT_STRING_BYTES: usize = 16 * 1024;
/// Largest the arguments of one call may be, serialised, in bytes.
pub const MAX_ARGUMENTS_BYTES: usize = 64 * 1024;
/// Longest error text a runner's failure may carry back to the model.
const MAX_ERROR_BYTES: usize = 2 * 1024;

// ---------------------------------------------------------------------------
// Kinds and the per-node view
// ---------------------------------------------------------------------------

/// The three kinds a remote tool can run as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteToolKind {
    Mcp,
    Rest,
    Cli,
}

impl RemoteToolKind {
    /// The executable kind in `chain` (nearest scope first), or `None` for a
    /// type that is no tool or has no kind to run it.
    pub fn of_chain<S: AsRef<str>>(chain: &[S]) -> Option<Self> {
        let core = CoreNodeType::nearest(chain)?;
        if core.is_a(CoreNodeType::ToolMcp) {
            Some(Self::Mcp)
        } else if core.is_a(CoreNodeType::ToolRest) {
            Some(Self::Rest)
        } else if core.is_a(CoreNodeType::ToolCli) {
            Some(Self::Cli)
        } else {
            None
        }
    }

    /// The node type whose bucket holds the kind's binding fields.
    pub fn bucket(self) -> &'static str {
        match self {
            Self::Mcp => CoreNodeType::ToolMcp.as_str(),
            Self::Rest => CoreNodeType::ToolRest.as_str(),
            Self::Cli => CoreNodeType::ToolCli.as_str(),
        }
    }
}

/// A remote tool that passed the gate, as stored on this device.
#[derive(Debug, Clone)]
pub struct RemoteTool {
    pub node_id: String,
    /// The name the model calls it by.
    pub name: String,
    pub kind: RemoteToolKind,
    pub description: String,
    /// A closed-root object schema.
    pub parameter_schema: Value,
    /// The node's stored properties, bucketed by type.
    pub properties: Value,
    /// The binding fingerprint that matched `approved_binding`.
    pub fingerprint: String,
}

impl RemoteTool {
    /// The binding fields of the tool's kind: the whole bucket, for the runner
    /// to read through the typed readers.
    pub fn binding(&self) -> &Value {
        self.properties
            .get(self.kind.bucket())
            .unwrap_or(&Value::Null)
    }

    /// The definition the model is shown.
    pub fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters_schema: self.parameter_schema.clone(),
        }
    }

    /// Whether a call is confirmed with the user before it runs: every `cli`
    /// and `mcp` call, and every `rest` call that is not a `GET`.
    pub fn needs_confirmation(&self) -> bool {
        match self.kind {
            RemoteToolKind::Cli | RemoteToolKind::Mcp => true,
            RemoteToolKind::Rest => {
                self.binding().get("method").and_then(Value::as_str) != Some("GET")
            }
        }
    }
}

/// Why a node is not a remote tool this device may offer or run. Logged with
/// the node id; never carries binding values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotARemoteTool,
    NoExecutableKind,
    BadName,
    TakesABuiltInName,
    NotEnabled,
    NotApproved,
    BadParameterSchema,
}

/// Judges one stored tool node. `chain` is the node type's chain, nearest scope
/// first; `properties` are the node's stored properties.
///
/// Only the `tool` bucket is read for `enabled` and `approved_binding`: a flat
/// value left at the top level is not this device's decision.
pub fn assess<S: AsRef<str>>(
    node_id: &str,
    name: &str,
    chain: &[S],
    properties: &Value,
) -> Result<RemoteTool, Refusal> {
    if ToolOrigin::of(chain) != Some(ToolOrigin::External) {
        return Err(Refusal::NotARemoteTool);
    }
    let kind = RemoteToolKind::of_chain(chain).ok_or(Refusal::NoExecutableKind)?;
    if !is_tool_name(name) {
        return Err(Refusal::BadName);
    }
    if Tool::from_name(name).is_some() {
        return Err(Refusal::TakesABuiltInName);
    }
    let base = CoreNodeType::Tool.as_str();
    let tool_bucket = properties.get(base);
    if tool_bucket.and_then(|b| b.get("enabled")) != Some(&Value::Bool(true)) {
        return Err(Refusal::NotEnabled);
    }
    let fingerprint = tool_binding_hash(chain, properties);
    let approved = tool_bucket
        .and_then(|b| b.get("approved_binding"))
        .and_then(Value::as_str);
    if approved != Some(fingerprint.as_str()) {
        return Err(Refusal::NotApproved);
    }
    let schema = tool_bucket
        .and_then(|b| b.get("parameter_schema"))
        .filter(|schema| !schema.is_null())
        .ok_or(Refusal::BadParameterSchema)?;
    if check_closed_root(schema).is_err() {
        return Err(Refusal::BadParameterSchema);
    }
    Ok(RemoteTool {
        node_id: node_id.to_string(),
        name: name.to_string(),
        kind,
        description: tool_bucket
            .and_then(|b| b.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        parameter_schema: schema.clone(),
        properties: properties.clone(),
        fingerprint,
    })
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

/// Checks a call's arguments against the tool's closed schema: an object, only
/// declared properties, every `required` one present, each of the declared
/// simple type, and bounded in count and size. Returns the reason on failure.
///
/// This is a gate on shape and size, not a full JSON Schema validator: nested
/// values are only checked for being of the declared container type.
pub fn validate_arguments(schema: &Value, args: &Value) -> Result<(), String> {
    let Some(args) = args.as_object() else {
        return Err("the arguments must be a JSON object".to_string());
    };
    if args.len() > MAX_ARGUMENTS {
        return Err(format!("at most {MAX_ARGUMENTS} arguments are allowed"));
    }
    let size = serde_json::to_string(args)
        .map(|t| t.len())
        .unwrap_or(usize::MAX);
    if size > MAX_ARGUMENTS_BYTES {
        return Err("the arguments are too large".to_string());
    }
    let empty = serde_json::Map::new();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for (key, value) in args {
        let Some(declared) = properties.get(key) else {
            return Err(format!("`{key}` is not a parameter of this tool"));
        };
        if let Some(kind) = declared.get("type").and_then(Value::as_str) {
            let ok = match kind {
                "string" => value.is_string(),
                "integer" => value.is_i64() || value.is_u64(),
                "number" => value.is_number(),
                "boolean" => value.is_boolean(),
                "array" => value.is_array(),
                "object" => value.is_object(),
                _ => false,
            };
            if !ok {
                return Err(format!("`{key}` must be of type {kind}"));
            }
        }
        if let Some(text) = value.as_str() {
            if text.len() > MAX_ARGUMENT_STRING_BYTES {
                return Err(format!("`{key}` is too long"));
            }
        }
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !args.contains_key(name) {
                return Err(format!("`{name}` is required"));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Runners and confirmation
// ---------------------------------------------------------------------------

/// What a runner returns.
#[derive(Debug, Clone)]
pub struct RemoteOutput {
    pub text: String,
    /// The runner cut the output at the binding's cap.
    pub truncated: bool,
}

/// A failure a runner reports to the model. The runner has already removed
/// every credential value from the message.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct RemoteCallError(pub String);

/// Runs the calls of one kind.
#[async_trait]
pub trait RemoteToolRunner: Send + Sync {
    fn kind(&self) -> RemoteToolKind;

    /// Where the call goes, for the confirmation question. Holds no credential.
    fn destination(&self, tool: &RemoteTool, args: &Value) -> String;

    /// Runs one validated, confirmed call.
    async fn run(&self, tool: &RemoteTool, args: &Value) -> Result<RemoteOutput, RemoteCallError>;
}

/// What the user is asked before a call.
#[derive(Debug, Clone)]
pub struct ConfirmationRequest {
    pub tool_name: String,
    pub kind: RemoteToolKind,
    pub destination: String,
    pub arguments: Value,
}

/// Asks the user whether a call may run.
#[async_trait]
pub trait CallConfirmer: Send + Sync {
    async fn confirm(&self, request: &ConfirmationRequest) -> bool;
}

// ---------------------------------------------------------------------------
// The host
// ---------------------------------------------------------------------------

/// Wraps the executor that serves the built-in tools and adds the remote tools
/// that pass the gate. Built-in names always go to the inner executor.
pub struct RemoteToolHost {
    inner: Arc<dyn AgentToolExecutor>,
    node_service: Arc<NodeService>,
    runners: HashMap<RemoteToolKind, Arc<dyn RemoteToolRunner>>,
    confirmer: Option<Arc<dyn CallConfirmer>>,
}

impl RemoteToolHost {
    /// A host with no runner: it offers and runs no remote tool.
    pub fn new(inner: Arc<dyn AgentToolExecutor>, node_service: Arc<NodeService>) -> Self {
        Self {
            inner,
            node_service,
            runners: HashMap::new(),
            confirmer: None,
        }
    }

    /// Installs the runner of its kind, replacing an earlier one.
    pub fn with_runner(mut self, runner: Arc<dyn RemoteToolRunner>) -> Self {
        self.runners.insert(runner.kind(), runner);
        self
    }

    /// Installs the question put to the user before a call. With none, a call
    /// that needs confirmation is refused.
    pub fn with_confirmer(mut self, confirmer: Arc<dyn CallConfirmer>) -> Self {
        self.confirmer = Some(confirmer);
        self
    }

    /// Every remote tool the model may be offered right now, sorted by name.
    async fn eligible(&self) -> Result<Vec<RemoteTool>, ToolError> {
        if self.runners.is_empty() {
            return Ok(Vec::new());
        }
        let nodes = self
            .node_service
            .query_nodes_by_type(CoreNodeType::Tool.as_str(), false)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("could not read tool nodes: {e}")))?;
        let mut chains: HashMap<String, Vec<String>> = HashMap::new();
        let mut passed: Vec<RemoteTool> = Vec::new();
        for node in &nodes {
            if !chains.contains_key(&node.node_type) {
                // An unresolved chain is no tool's chain: a read failure never
                // widens the surface.
                let chain = self
                    .node_service
                    .resolve_type_chain(&node.node_type)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!(node_type = %node.node_type, error = %e, "remote tools: type chain unresolved");
                        Vec::new()
                    });
                chains.insert(node.node_type.clone(), chain);
            }
            match assess_node(node, &chains[&node.node_type]) {
                Ok(tool) => {
                    if self.runners.contains_key(&tool.kind) {
                        passed.push(tool);
                    } else {
                        tracing::debug!(node_id = %node.id, "remote tools: no runner for this kind");
                    }
                }
                Err(Refusal::NotARemoteTool) => {}
                Err(reason) => {
                    tracing::debug!(node_id = %node.id, ?reason, "remote tools: not offered");
                }
            }
        }
        // A name two tools share is never resolved by picking one.
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for tool in &passed {
            *counts.entry(tool.name.as_str()).or_default() += 1;
        }
        let shared: Vec<String> = counts
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(name, _)| name.to_string())
            .collect();
        for name in &shared {
            tracing::warn!(name = %name, "remote tools: several approved tools share a name; none is offered");
        }
        passed.retain(|tool| !shared.contains(&tool.name));
        passed.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(passed)
    }

    fn failure(name: &str, message: String) -> ToolResult {
        let mut message = message;
        if message.len() > MAX_ERROR_BYTES {
            let mut end = MAX_ERROR_BYTES;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        ToolResult {
            tool_call_id: format!("call_{name}"),
            name: name.to_string(),
            result: json!({ "error": message }),
            is_error: true,
        }
    }

    async fn call_remote(&self, tool: RemoteTool, args: Value) -> Result<ToolResult, ToolError> {
        let name = tool.name.clone();
        if let Err(reason) = validate_arguments(&tool.parameter_schema, &args) {
            return Err(ToolError::InvalidArguments { tool: name, reason });
        }
        let runner = self
            .runners
            .get(&tool.kind)
            .cloned()
            .ok_or_else(|| ToolError::UnknownTool(name.clone()))?;
        if tool.needs_confirmation() {
            let Some(confirmer) = &self.confirmer else {
                return Ok(Self::failure(
                    &name,
                    "this call needs the user's confirmation and none can be asked".to_string(),
                ));
            };
            let request = ConfirmationRequest {
                tool_name: name.clone(),
                kind: tool.kind,
                destination: runner.destination(&tool, &args),
                arguments: args.clone(),
            };
            if !confirmer.confirm(&request).await {
                return Ok(Self::failure(
                    &name,
                    "the user did not confirm the call".to_string(),
                ));
            }
            // The binding may have changed while the question was open: run
            // only what was confirmed.
            let still = self.eligible().await?;
            if !still
                .iter()
                .any(|t| t.node_id == tool.node_id && t.fingerprint == tool.fingerprint)
            {
                return Ok(Self::failure(
                    &name,
                    "the tool changed or was switched off before it could run".to_string(),
                ));
            }
        }
        match runner.run(&tool, &args).await {
            Ok(mut output) => {
                let cap = MAX_RESPONSE_BYTES as usize;
                if output.text.len() > cap {
                    let mut end = cap;
                    while !output.text.is_char_boundary(end) {
                        end -= 1;
                    }
                    output.text.truncate(end);
                    output.truncated = true;
                }
                Ok(ToolResult {
                    tool_call_id: format!("call_{name}"),
                    name,
                    result: json!({
                        "source": "external",
                        "output": output.text,
                        "truncated": output.truncated,
                    }),
                    is_error: false,
                })
            }
            Err(error) => Ok(Self::failure(&name, error.0)),
        }
    }
}

/// [`assess`] a stored node.
fn assess_node(node: &Node, chain: &[String]) -> Result<RemoteTool, Refusal> {
    assess(&node.id, &node.content, chain, &node.properties)
}

#[async_trait]
impl AgentToolExecutor for RemoteToolHost {
    async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
        let mut tools = self.inner.available_tools().await?;
        tools.extend(self.eligible().await?.iter().map(RemoteTool::definition));
        Ok(tools)
    }

    async fn execute(&self, name: &str, args: Value) -> Result<ToolResult, ToolError> {
        // A built-in name is the inner executor's, whatever a node says.
        if Tool::from_name(name).is_some() {
            return self.inner.execute(name, args).await;
        }
        let tool = self.eligible().await?.into_iter().find(|t| t.name == name);
        match tool {
            Some(tool) => self.call_remote(tool, args).await,
            None => self.inner.execute(name, args).await,
        }
    }

    async fn routing_available(&self) -> bool {
        self.inner.routing_available().await
    }

    async fn retrieve_skills(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<SkillRetrieval, ToolError> {
        self.inner.retrieve_skills(query, limit).await
    }

    async fn skill_names(&self) -> Vec<String> {
        self.inner.skill_names().await
    }

    async fn user_type_names(&self) -> Vec<String> {
        self.inner.user_type_names().await
    }

    async fn node_type(&self, id: &str) -> Result<Option<String>, ToolError> {
        self.inner.node_type(id).await
    }

    async fn task_status_values(&self) -> Option<Vec<String>> {
        self.inner.task_status_values().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_agent::tools::GraphToolExecutor;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::NodeUpdate;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;
    use tokio::sync::RwLock;

    fn closed_schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": { "id": { "type": "string" }, "q": { "type": "string" } },
        })
    }

    fn binding(kind: &str) -> Value {
        match kind {
            "tool-mcp" => json!({
                "transport": "http",
                "url": "https://mcp.example.com/rpc",
                "remote_tool_name": "search",
            }),
            "tool-rest" => json!({
                "method": "GET",
                "url_template": "https://api.example.com/items/{id}",
            }),
            _ => json!({ "program": "git", "args": ["log", "{q}"] }),
        }
    }

    fn draft(kind: &str) -> Value {
        let mut p = binding(kind);
        p["description"] = json!("Looks a thing up");
        p["parameter_schema"] = closed_schema();
        p["enabled"] = json!(false);
        p
    }

    // -- the pure gate ---------------------------------------------------

    /// Stored properties of a tool of `kind` that is enabled and approved.
    fn approved(kind: &str) -> (Vec<String>, Value) {
        let chain = vec![kind.to_string(), "tool".to_string()];
        let mut properties = json!({
            "tool": {
                "description": "d",
                "parameter_schema": closed_schema(),
                "enabled": true,
            },
            kind: binding(kind),
        });
        let hash = tool_binding_hash(&chain, &properties);
        properties["tool"]["approved_binding"] = json!(hash);
        (chain, properties)
    }

    #[test]
    fn an_enabled_approved_tool_of_each_kind_passes() {
        for kind in ["tool-mcp", "tool-rest", "tool-cli"] {
            let (chain, props) = approved(kind);
            let tool =
                assess("id", "lookup", &chain, &props).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
            assert_eq!(tool.name, "lookup");
            assert_eq!(tool.definition().parameters_schema, closed_schema());
        }
    }

    #[test]
    fn enabled_absent_false_or_not_true_means_off() {
        let (chain, props) = approved("tool-cli");
        for off in [json!(false), json!("true"), json!(1), Value::Null] {
            let mut p = props.clone();
            p["tool"]["enabled"] = off.clone();
            assert_eq!(
                assess("i", "lookup", &chain, &p).unwrap_err(),
                Refusal::NotEnabled,
                "{off}"
            );
        }
        let mut p = props.clone();
        p["tool"].as_object_mut().unwrap().remove("enabled");
        assert_eq!(
            assess("i", "lookup", &chain, &p).unwrap_err(),
            Refusal::NotEnabled
        );
    }

    #[test]
    fn a_flat_enabled_outside_the_tool_bucket_is_not_this_devices_decision() {
        let (chain, mut props) = approved("tool-cli");
        props["tool"].as_object_mut().unwrap().remove("enabled");
        props["enabled"] = json!(true);
        assert_eq!(
            assess("i", "lookup", &chain, &props).unwrap_err(),
            Refusal::NotEnabled
        );
    }

    #[test]
    fn an_absent_or_wrong_approval_means_off() {
        let (chain, props) = approved("tool-cli");
        let mut absent = props.clone();
        absent["tool"]
            .as_object_mut()
            .unwrap()
            .remove("approved_binding");
        assert_eq!(
            assess("i", "lookup", &chain, &absent).unwrap_err(),
            Refusal::NotApproved
        );
        let mut other = props.clone();
        other["tool"]["approved_binding"] = json!("0".repeat(64));
        assert_eq!(
            assess("i", "lookup", &chain, &other).unwrap_err(),
            Refusal::NotApproved
        );
        let mut flat = props.clone();
        let hash = flat["tool"]["approved_binding"].clone();
        flat["tool"]
            .as_object_mut()
            .unwrap()
            .remove("approved_binding");
        flat["approved_binding"] = hash;
        assert_eq!(
            assess("i", "lookup", &chain, &flat).unwrap_err(),
            Refusal::NotApproved
        );
    }

    #[test]
    fn a_changed_binding_or_schema_after_approval_means_off_but_a_reworded_description_does_not() {
        let (chain, props) = approved("tool-cli");
        let mut program = props.clone();
        program["tool-cli"]["program"] = json!("rm");
        assert_eq!(
            assess("i", "lookup", &chain, &program).unwrap_err(),
            Refusal::NotApproved
        );
        let mut args = props.clone();
        args["tool-cli"]["args"] = json!(["log", "{q}", "--all"]);
        assert_eq!(
            assess("i", "lookup", &chain, &args).unwrap_err(),
            Refusal::NotApproved
        );
        let mut schema = props.clone();
        schema["tool"]["parameter_schema"]["properties"]["extra"] = json!({"type":"string"});
        assert_eq!(
            assess("i", "lookup", &chain, &schema).unwrap_err(),
            Refusal::NotApproved
        );
        let mut reworded = props.clone();
        reworded["tool"]["description"] = json!("something else");
        assert!(assess("i", "lookup", &chain, &reworded).is_ok());
    }

    #[test]
    fn a_tool_with_no_executable_kind_a_bad_name_or_a_built_in_name_is_refused() {
        let (_, props) = approved("tool-cli");
        assert_eq!(
            assess("i", "lookup", &["remote-thing", "tool"], &props).unwrap_err(),
            Refusal::NoExecutableKind
        );
        let chain = vec!["tool-cli".to_string(), "tool".to_string()];
        for bad in ["", "9lives", "has space", "a/b", &"n".repeat(65)] {
            assert_eq!(
                assess("i", bad, &chain, &props).unwrap_err(),
                Refusal::BadName,
                "{bad}"
            );
        }
        assert_eq!(
            assess("i", "get_node", &chain, &props).unwrap_err(),
            Refusal::TakesABuiltInName
        );
        // A native tool and a non-tool are not remote tools.
        assert_eq!(
            assess("i", "lookup", &["tool-native", "tool"], &props).unwrap_err(),
            Refusal::NotARemoteTool
        );
        assert_eq!(
            assess("i", "lookup", &["text"], &props).unwrap_err(),
            Refusal::NotARemoteTool
        );
    }

    #[test]
    fn a_user_subtype_of_a_remote_kind_runs_as_that_kind() {
        let (_, props) = approved("tool-rest");
        let chain = vec![
            "my-rest".to_string(),
            "tool-rest".to_string(),
            "tool".to_string(),
        ];
        // The user type's own bucket joins the fingerprint, so approve again.
        let mut p = props.clone();
        p["my-rest"] = json!({});
        let hash = tool_binding_hash(&chain, &p);
        p["tool"]["approved_binding"] = json!(hash);
        let tool = assess("i", "lookup", &chain, &p).unwrap();
        assert_eq!(tool.kind, RemoteToolKind::Rest);
    }

    #[test]
    fn an_open_or_missing_parameter_schema_is_refused() {
        let (chain, props) = approved("tool-cli");
        for schema in [
            json!({"type":"object","properties":{}}),
            json!({"type":"object","additionalProperties":true,"properties":{}}),
            Value::Null,
        ] {
            let mut p = props.clone();
            p["tool"]["parameter_schema"] = schema.clone();
            let hash = tool_binding_hash(&chain, &p);
            p["tool"]["approved_binding"] = json!(hash);
            assert_eq!(
                assess("i", "lookup", &chain, &p).unwrap_err(),
                Refusal::BadParameterSchema,
                "{schema}"
            );
        }
    }

    #[test]
    fn confirmation_is_needed_for_cli_mcp_and_any_rest_that_is_not_get() {
        let cli = assess("i", "t", &approved("tool-cli").0, &approved("tool-cli").1).unwrap();
        let mcp = assess("i", "t", &approved("tool-mcp").0, &approved("tool-mcp").1).unwrap();
        let rest_get =
            assess("i", "t", &approved("tool-rest").0, &approved("tool-rest").1).unwrap();
        assert!(cli.needs_confirmation());
        assert!(mcp.needs_confirmation());
        assert!(!rest_get.needs_confirmation());
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            let (chain, mut p) = approved("tool-rest");
            p["tool-rest"]["method"] = json!(method);
            let hash = tool_binding_hash(&chain, &p);
            p["tool"]["approved_binding"] = json!(hash);
            assert!(
                assess("i", "t", &chain, &p).unwrap().needs_confirmation(),
                "{method}"
            );
        }
        // A rest tool that does not say its method is confirmed.
        let (chain, mut p) = approved("tool-rest");
        p["tool-rest"].as_object_mut().unwrap().remove("method");
        let hash = tool_binding_hash(&chain, &p);
        p["tool"]["approved_binding"] = json!(hash);
        assert!(assess("i", "t", &chain, &p).unwrap().needs_confirmation());
    }

    // -- arguments -------------------------------------------------------

    #[test]
    fn arguments_must_be_a_closed_declared_typed_bounded_object() {
        let schema = json!({
            "type": "object", "additionalProperties": false,
            "properties": { "q": {"type":"string"}, "n": {"type":"integer"} },
            "required": ["q"],
        });
        assert!(validate_arguments(&schema, &json!({"q":"x","n":3})).is_ok());
        assert!(validate_arguments(&schema, &json!("x")).is_err());
        assert!(validate_arguments(&schema, &json!(["q"])).is_err());
        assert!(validate_arguments(&schema, &json!({"q":"x","other":1})).is_err());
        assert!(validate_arguments(&schema, &json!({"n":3})).is_err());
        assert!(validate_arguments(&schema, &json!({"q":5})).is_err());
        assert!(validate_arguments(&schema, &json!({"q":"x","n":1.5})).is_err());
        let long = "x".repeat(MAX_ARGUMENT_STRING_BYTES + 1);
        assert!(validate_arguments(&schema, &json!({"q": long})).is_err());
        let max = "x".repeat(MAX_ARGUMENT_STRING_BYTES);
        assert!(validate_arguments(&schema, &json!({"q": max})).is_ok());
    }

    #[test]
    fn the_number_and_total_size_of_arguments_are_bounded() {
        let mut props = serde_json::Map::new();
        for i in 0..=MAX_ARGUMENTS {
            props.insert(format!("p{i}"), json!({"type":"string"}));
        }
        let schema = json!({"type":"object","additionalProperties":false,"properties":props});
        let mut args = serde_json::Map::new();
        for i in 0..MAX_ARGUMENTS {
            args.insert(format!("p{i}"), json!("v"));
        }
        assert!(validate_arguments(&schema, &Value::Object(args.clone())).is_ok());
        args.insert(format!("p{MAX_ARGUMENTS}"), json!("v"));
        assert!(validate_arguments(&schema, &Value::Object(args)).is_err());
        // Total size: arrays are not bounded per item, so the total is.
        let schema = json!({"type":"object","additionalProperties":false,"properties":{"a":{"type":"array"}}});
        let big = json!({"a": vec!["x".repeat(1000); 100]});
        assert!(validate_arguments(&schema, &big).is_err());
    }

    // -- the host, over a real database ----------------------------------

    struct FakeRunner {
        kind: RemoteToolKind,
        runs: Arc<AtomicUsize>,
        output: String,
    }

    #[async_trait]
    impl RemoteToolRunner for FakeRunner {
        fn kind(&self) -> RemoteToolKind {
            self.kind
        }
        fn destination(&self, tool: &RemoteTool, _args: &Value) -> String {
            format!("fake destination of {}", tool.name)
        }
        async fn run(
            &self,
            _tool: &RemoteTool,
            _args: &Value,
        ) -> Result<RemoteOutput, RemoteCallError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(RemoteOutput {
                text: self.output.clone(),
                truncated: false,
            })
        }
    }

    struct Confirmer {
        answer: bool,
        asked: Arc<AtomicUsize>,
        /// Run once the question is asked, to change the world under the call.
        during: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    #[async_trait]
    impl CallConfirmer for Confirmer {
        async fn confirm(&self, _request: &ConfirmationRequest) -> bool {
            self.asked.fetch_add(1, Ordering::SeqCst);
            if let Some(f) = &self.during {
                f();
            }
            self.answer
        }
    }

    struct Fixture {
        ns: Arc<NodeService>,
        inner: Arc<dyn AgentToolExecutor>,
        _tmp: TempDir,
    }

    async fn fixture() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let mut store: Arc<SqliteStore> =
            Arc::new(SqliteStore::new(tmp.path().join("t.db")).await.unwrap());
        let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
        let inner: Arc<dyn AgentToolExecutor> = Arc::new(GraphToolExecutor {
            node_service: Some(ns.clone()),
            embedding_service: Arc::new(RwLock::new(None)),
            inference_engine: None,
            playbook_lifecycle: None,
        });
        Fixture {
            ns,
            inner,
            _tmp: tmp,
        }
    }

    impl Fixture {
        /// Creates a draft tool, then enables and approves it as this device
        /// would. Returns the node id.
        async fn tool(&self, kind: &str, name: &str, approve: bool) -> String {
            let id = self
                .ns
                .create_node(Node::new(kind.to_string(), name.to_string(), draft(kind)))
                .await
                .unwrap();
            if approve {
                self.approve(&id).await;
            }
            id
        }

        async fn approve(&self, id: &str) {
            let node = self.ns.get_node(id).await.unwrap().unwrap();
            let chain = self.ns.resolve_type_chain(&node.node_type).await.unwrap();
            let hash = tool_binding_hash(&chain, &node.properties);
            self.ns
                .update_node(
                    id,
                    node.version,
                    NodeUpdate::new()
                        .with_properties(json!({"enabled": true, "approved_binding": hash})),
                )
                .await
                .unwrap();
        }

        fn host(&self, kinds: &[RemoteToolKind], runs: &Arc<AtomicUsize>) -> RemoteToolHost {
            let mut host = RemoteToolHost::new(self.inner.clone(), self.ns.clone());
            for kind in kinds {
                host = host.with_runner(Arc::new(FakeRunner {
                    kind: *kind,
                    runs: runs.clone(),
                    output: "ok".into(),
                }));
            }
            host
        }
    }

    /// The remote tools offered: the built-in tools are left out.
    async fn names(host: &RemoteToolHost) -> Vec<String> {
        host.available_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .filter(|name| Tool::from_name(name).is_none())
            .collect()
    }

    async fn all_names(host: &RemoteToolHost) -> Vec<String> {
        host.available_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect()
    }

    fn all_kinds() -> Vec<RemoteToolKind> {
        vec![
            RemoteToolKind::Mcp,
            RemoteToolKind::Rest,
            RemoteToolKind::Cli,
        ]
    }

    #[tokio::test]
    async fn an_enabled_approved_tool_is_offered_and_dispatched_and_the_rest_are_neither() {
        let f = fixture().await;
        f.tool("tool-rest", "approved_one", true).await;
        f.tool("tool-rest", "draft_one", false).await;
        let runs = Arc::new(AtomicUsize::new(0));
        let host = f.host(&all_kinds(), &runs);
        assert_eq!(names(&host).await, vec!["approved_one"]);

        let ok = host
            .execute("approved_one", json!({"id": "1"}))
            .await
            .unwrap();
        assert!(!ok.is_error, "{ok:?}");
        assert_eq!(ok.result["output"], "ok");
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // Not offered means not dispatched.
        let err = host.execute("draft_one", json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::UnknownTool(_)));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    /// The tool the model is offered is the tool a call reaches, and the other
    /// way round: with no runner for a kind the tool is in neither.
    #[tokio::test]
    async fn a_tool_is_dispatched_or_not_offered_never_offered_and_inert() {
        let f = fixture().await;
        for (kind, name) in [
            ("tool-mcp", "m_tool"),
            ("tool-rest", "r_tool"),
            ("tool-cli", "c_tool"),
        ] {
            f.tool(kind, name, true).await;
        }
        let runs = Arc::new(AtomicUsize::new(0));
        let confirmer = Arc::new(Confirmer {
            answer: true,
            asked: Arc::new(AtomicUsize::new(0)),
            during: None,
        });
        for installed in [
            vec![],
            vec![RemoteToolKind::Rest],
            vec![RemoteToolKind::Cli, RemoteToolKind::Mcp],
            all_kinds(),
        ] {
            let host = f.host(&installed, &runs).with_confirmer(confirmer.clone());
            let offered = names(&host).await;
            for (kind, name) in [
                (RemoteToolKind::Mcp, "m_tool"),
                (RemoteToolKind::Rest, "r_tool"),
                (RemoteToolKind::Cli, "c_tool"),
            ] {
                let has_runner = installed.contains(&kind);
                assert_eq!(
                    offered.contains(&name.to_string()),
                    has_runner,
                    "{name} offered with {installed:?}"
                );
                let dispatched = host.execute(name, json!({"q": "x"})).await;
                match dispatched {
                    Ok(r) => assert!(has_runner && !r.is_error, "{name}: {r:?}"),
                    Err(ToolError::UnknownTool(_)) => {
                        assert!(!has_runner, "{name} offered but not dispatched")
                    }
                    Err(other) => panic!("{name}: {other:?}"),
                }
            }
        }
    }

    #[tokio::test]
    async fn a_host_with_no_runner_offers_only_the_built_in_tools() {
        let f = fixture().await;
        f.tool("tool-cli", "c_tool", true).await;
        let host = f.host(&[], &Arc::new(AtomicUsize::new(0)));
        let inner = f
            .inner
            .available_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect::<Vec<_>>();
        assert_eq!(all_names(&host).await, inner);
    }

    #[tokio::test]
    async fn editing_the_binding_after_approval_turns_the_tool_off_until_it_is_approved_again() {
        let f = fixture().await;
        let id = f.tool("tool-cli", "c_tool", true).await;
        let runs = Arc::new(AtomicUsize::new(0));
        let host = f
            .host(&all_kinds(), &runs)
            .with_confirmer(Arc::new(Confirmer {
                answer: true,
                asked: Arc::new(AtomicUsize::new(0)),
                during: None,
            }));
        assert_eq!(names(&host).await, vec!["c_tool"]);

        let node = f.ns.get_node(&id).await.unwrap().unwrap();
        f.ns.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({"program": "rm"})),
        )
        .await
        .unwrap();
        assert!(names(&host).await.is_empty());
        assert!(matches!(
            host.execute("c_tool", json!({"q":"x"})).await,
            Err(ToolError::UnknownTool(_))
        ));
        assert_eq!(runs.load(Ordering::SeqCst), 0);

        f.approve(&id).await;
        assert_eq!(names(&host).await, vec!["c_tool"]);
    }

    #[tokio::test]
    async fn an_archived_approved_tool_is_not_offered_or_run() {
        let f = fixture().await;
        let mut node = Node::new(
            "tool-cli".to_string(),
            "arch".to_string(),
            draft("tool-cli"),
        );
        node.lifecycle_status = "archived".to_string();
        let id = f.ns.create_node(node).await.unwrap();
        f.approve(&id).await;
        let runs = Arc::new(AtomicUsize::new(0));
        let host = f.host(&all_kinds(), &runs);
        assert!(names(&host).await.is_empty());
        assert!(matches!(
            host.execute("arch", json!({"q":"x"})).await,
            Err(ToolError::UnknownTool(_))
        ));
    }

    #[tokio::test]
    async fn two_approved_tools_with_one_name_are_both_dropped() {
        let f = fixture().await;
        f.tool("tool-cli", "twin", true).await;
        f.tool("tool-rest", "twin", true).await;
        f.tool("tool-cli", "solo", true).await;
        let host = f.host(&all_kinds(), &Arc::new(AtomicUsize::new(0)));
        assert_eq!(names(&host).await, vec!["solo"]);
        assert!(matches!(
            host.execute("twin", json!({})).await,
            Err(ToolError::UnknownTool(_))
        ));
    }

    #[tokio::test]
    async fn an_unapproved_twin_does_not_remove_an_approved_tool() {
        let f = fixture().await;
        f.tool("tool-cli", "twin", true).await;
        f.tool("tool-cli", "twin", false).await;
        let host = f.host(&all_kinds(), &Arc::new(AtomicUsize::new(0)));
        assert_eq!(names(&host).await, vec!["twin"]);
    }

    #[tokio::test]
    async fn a_remote_tool_never_replaces_a_built_in_name_and_built_ins_go_to_the_inner_executor() {
        let f = fixture().await;
        f.tool("tool-cli", "get_node", true).await;
        let runs = Arc::new(AtomicUsize::new(0));
        let host = f.host(&all_kinds(), &runs);
        let built_in = f
            .inner
            .available_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect::<Vec<_>>();
        assert_eq!(all_names(&host).await, built_in);
        let _ = host
            .execute("get_node", json!({"node_id": "missing"}))
            .await;
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn arguments_that_break_the_schema_are_refused_before_anything_runs() {
        let f = fixture().await;
        f.tool("tool-rest", "r_tool", true).await;
        let runs = Arc::new(AtomicUsize::new(0));
        let host = f.host(&all_kinds(), &runs);
        for bad in [json!({"undeclared": 1}), json!({"q": 5}), json!("text")] {
            let err = host.execute("r_tool", bad.clone()).await.unwrap_err();
            assert!(
                matches!(err, ToolError::InvalidArguments { .. }),
                "{bad}: {err:?}"
            );
        }
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_call_that_needs_confirmation_does_not_run_without_a_yes() {
        let f = fixture().await;
        f.tool("tool-cli", "c_tool", true).await;
        f.tool("tool-rest", "r_get", true).await;
        let runs = Arc::new(AtomicUsize::new(0));

        // No confirmer: refused, nothing runs. A GET rest call needs none.
        let host = f.host(&all_kinds(), &runs);
        let refused = host.execute("c_tool", json!({"q": "x"})).await.unwrap();
        assert!(refused.is_error);
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        assert!(!host.execute("r_get", json!({})).await.unwrap().is_error);
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // A "no" refuses too.
        let asked = Arc::new(AtomicUsize::new(0));
        let host = f
            .host(&all_kinds(), &runs)
            .with_confirmer(Arc::new(Confirmer {
                answer: false,
                asked: asked.clone(),
                during: None,
            }));
        assert!(
            host.execute("c_tool", json!({"q": "x"}))
                .await
                .unwrap()
                .is_error
        );
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // A "yes" runs it once.
        let host = f
            .host(&all_kinds(), &runs)
            .with_confirmer(Arc::new(Confirmer {
                answer: true,
                asked: asked.clone(),
                during: None,
            }));
        assert!(
            !host
                .execute("c_tool", json!({"q": "x"}))
                .await
                .unwrap()
                .is_error
        );
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    /// The binding changes while the question is open: once it is switched off
    /// by the change, and once it is approved again for the new binding (so it
    /// is still eligible, with a different fingerprint). Neither may run.
    #[tokio::test]
    async fn a_binding_changed_while_the_question_is_open_is_not_run() {
        for reapprove in [false, true] {
            let f = Arc::new(fixture().await);
            let id = f.tool("tool-cli", "c_tool", true).await;
            let runs = Arc::new(AtomicUsize::new(0));
            let ns = f.ns.clone();
            let during: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                let ns = ns.clone();
                let id = id.clone();
                // The question is asked on this task; edit from a thread of its own.
                std::thread::spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(async {
                            let node = ns.get_node(&id).await.unwrap().unwrap();
                            ns.update_node(
                                &id,
                                node.version,
                                NodeUpdate::new().with_properties(json!({"program": "rm"})),
                            )
                            .await
                            .unwrap();
                            if reapprove {
                                let node = ns.get_node(&id).await.unwrap().unwrap();
                                let chain = ns.resolve_type_chain(&node.node_type).await.unwrap();
                                let hash = tool_binding_hash(&chain, &node.properties);
                                ns.update_node(
                                    &id,
                                    node.version,
                                    NodeUpdate::new()
                                        .with_properties(json!({"approved_binding": hash})),
                                )
                                .await
                                .unwrap();
                            }
                        });
                })
                .join()
                .unwrap();
            });
            let host = f
                .host(&all_kinds(), &runs)
                .with_confirmer(Arc::new(Confirmer {
                    answer: true,
                    asked: Arc::new(AtomicUsize::new(0)),
                    during: Some(during),
                }));
            let result = host.execute("c_tool", json!({"q": "x"})).await.unwrap();
            assert!(result.is_error, "reapprove={reapprove}: {result:?}");
            assert_eq!(runs.load(Ordering::SeqCst), 0, "reapprove={reapprove}");
        }
    }

    #[tokio::test]
    async fn the_confirmation_names_the_destination_and_a_result_is_marked_external() {
        struct Capture(Arc<std::sync::Mutex<Vec<ConfirmationRequest>>>);
        #[async_trait]
        impl CallConfirmer for Capture {
            async fn confirm(&self, r: &ConfirmationRequest) -> bool {
                self.0.lock().unwrap().push(r.clone());
                true
            }
        }
        let f = fixture().await;
        f.tool("tool-cli", "c_tool", true).await;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let host = f
            .host(&all_kinds(), &Arc::new(AtomicUsize::new(0)))
            .with_confirmer(Arc::new(Capture(seen.clone())));
        let result = host.execute("c_tool", json!({"q": "x"})).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].destination, "fake destination of c_tool");
        assert_eq!(seen[0].arguments, json!({"q": "x"}));
        assert_eq!(result.result["source"], "external");
    }

    #[tokio::test]
    async fn a_runner_failure_is_a_bounded_error_result_and_oversize_output_is_cut() {
        struct Failing(String);
        #[async_trait]
        impl RemoteToolRunner for Failing {
            fn kind(&self) -> RemoteToolKind {
                RemoteToolKind::Rest
            }
            fn destination(&self, _: &RemoteTool, _: &Value) -> String {
                String::new()
            }
            async fn run(
                &self,
                _: &RemoteTool,
                _: &Value,
            ) -> Result<RemoteOutput, RemoteCallError> {
                if self.0.is_empty() {
                    Ok(RemoteOutput {
                        text: "x".repeat(MAX_RESPONSE_BYTES as usize + 10),
                        truncated: false,
                    })
                } else {
                    Err(RemoteCallError(self.0.clone()))
                }
            }
        }
        let f = fixture().await;
        f.tool("tool-rest", "r_tool", true).await;
        let host = RemoteToolHost::new(f.inner.clone(), f.ns.clone())
            .with_runner(Arc::new(Failing("e".repeat(10_000))));
        let r = host.execute("r_tool", json!({})).await.unwrap();
        assert!(r.is_error);
        assert!(r.result["error"].as_str().unwrap().len() <= MAX_ERROR_BYTES);
        let host = RemoteToolHost::new(f.inner.clone(), f.ns.clone())
            .with_runner(Arc::new(Failing(String::new())));
        let r = host.execute("r_tool", json!({})).await.unwrap();
        assert_eq!(r.result["truncated"], true);
        assert_eq!(
            r.result["output"].as_str().unwrap().len(),
            MAX_RESPONSE_BYTES as usize
        );
    }
}
