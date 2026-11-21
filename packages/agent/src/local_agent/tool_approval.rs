//! Approving a remote tool on this device.
//!
//! A remote tool (a `tool-cli`, `tool-mcp` or `tool-rest` node, or a type that
//! extends one) is offered to the model only when this device has approved it:
//! `approved_binding` holds a keyed MAC, under a key that stays on the device,
//! of the node, its name, its description and its binding (see
//! [`super::remote_tools`]). This module is the one place that writes an
//! approval, in two steps the user sees:
//!
//! 1. [`preview`] reads the node and returns EXACTLY what would be approved:
//!    the name, the description, the kind, every binding field (program and
//!    arguments, or URL, method and headers), the credential references and the
//!    parameter schema, plus a digest of it. Nothing is written.
//! 2. [`approve`] takes that digest back. It reads the node again, and writes the
//!    approval only if the node still produces the same digest: a node edited
//!    between the two steps is refused, so the user never approves what they
//!    did not see. The device key is created on first use and never replaced
//!    (`SecretStore::provision_approval_key`).
//!
//! Neither step can be reached by a program the daemon started for a tool: that
//! is the daemon's peer gate on the RPCs that call this module, not something
//! checked here.

use nodespace_core::behaviors::{tool_binding_hash, ToolOrigin};
use nodespace_core::models::{CoreNodeType, Node, NodeUpdate};
use nodespace_core::secrets::SecretStore;
use nodespace_core::services::{NodeService, NodeServiceError};
use nodespace_types::is_tool_name;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::remote_tools::{self, assess, RemoteToolKind};
use super::tools::Tool;

/// Longest description, binding or schema text a preview shows. A node with
/// more is refused: what cannot be shown whole cannot be approved.
pub const MAX_PREVIEW_JSON_BYTES: usize = 16 * 1024;

/// What approving a node would approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPreview {
    pub node_id: String,
    pub name: String,
    pub description: String,
    pub kind: RemoteToolKind,
    /// Every binding field of the node's kind (and of any type between it and
    /// `tool`), as indented JSON.
    pub binding_json: String,
    /// The arguments the model may pass, as indented JSON.
    pub parameter_schema_json: String,
    /// The credential references the tool uses, one per entry. References only.
    pub credentials: Vec<String>,
    /// Digest of what is shown (and of the binding it stands for).
    pub digest: String,
    /// Whether the tool is enabled and approved on this device already.
    pub approved: bool,
}

/// Why a preview or an approval was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApprovalError {
    #[error("no such node")]
    NotFound,
    #[error("the tool could not be read: {0}")]
    Read(String),
    #[error("this node is not a remote tool")]
    NotARemoteTool,
    #[error("this tool has no kind to run it (it must extend tool-cli, tool-mcp or tool-rest)")]
    NoExecutableKind,
    #[error("the tool's name is not a plain identifier")]
    BadName,
    #[error("the tool's name is a built-in tool's")]
    TakesABuiltInName,
    #[error("the tool's parameter schema is missing or not closed")]
    BadParameterSchema,
    #[error("the tool's description, binding or schema is too large to be shown whole, so it cannot be approved")]
    TooLarge,
    #[error("the tool changed since it was shown: review it again")]
    Changed,
    #[error("this device's approval key could not be used: {0}")]
    Key(String),
    #[error("the tool was edited while it was being approved: review it again")]
    Conflict,
    #[error("the approval could not be written: {0}")]
    Write(String),
}

fn text(properties: &Value, bucket: &str, field: &str) -> String {
    properties
        .get(bucket)
        .and_then(|b| b.get(field))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// The credential references a binding bucket uses: each `env` entry as
/// `NAME from <reference>` and the `auth` secret as `<scheme> from <reference>`.
/// References only; no value is read.
fn credentials_of(binding: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(entries) = binding.get("env").and_then(Value::as_array) {
        for entry in entries {
            if let (Some(name), Some(secret)) = (
                entry.get("name").and_then(Value::as_str),
                entry.get("secret").and_then(Value::as_str),
            ) {
                out.push(format!("{name} from {secret}"));
            }
        }
    }
    if let Some(auth) = binding.get("auth") {
        let scheme = auth.get("scheme").and_then(Value::as_str).unwrap_or("");
        if scheme != "none" {
            if let Some(secret) = auth.get("secret").and_then(Value::as_str) {
                out.push(
                    match (scheme, auth.get("header_name").and_then(Value::as_str)) {
                        ("header", Some(name)) => format!("header {name} from {secret}"),
                        _ => format!("{scheme} from {secret}"),
                    },
                );
            }
        }
    }
    out
}

fn digest_of(node_id: &str, name: &str, description: &str, binding_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"tool-approval-preview-v1");
    for field in [node_id, name, description, binding_hash] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn pretty(value: &Value) -> Result<String, ApprovalError> {
    let text = serde_json::to_string_pretty(value).map_err(|_| ApprovalError::TooLarge)?;
    if text.len() > MAX_PREVIEW_JSON_BYTES {
        return Err(ApprovalError::TooLarge);
    }
    Ok(text)
}

async fn load(
    node_service: &NodeService,
    node_id: &str,
) -> Result<(Node, Vec<String>), ApprovalError> {
    let node = node_service
        .get_node(node_id)
        .await
        .map_err(|e| ApprovalError::Read(e.to_string()))?
        .ok_or(ApprovalError::NotFound)?;
    let chain = node_service
        .resolve_type_chain(&node.node_type)
        .await
        .map_err(|_| ApprovalError::NotARemoteTool)?;
    Ok((node, chain))
}

/// The checks every remote tool passes before it can be offered, other than
/// being enabled and approved.
fn check_shape(node: &Node, chain: &[String]) -> Result<RemoteToolKind, ApprovalError> {
    if ToolOrigin::of(chain) != Some(ToolOrigin::External) {
        return Err(ApprovalError::NotARemoteTool);
    }
    let kind = RemoteToolKind::of_chain(chain).ok_or(ApprovalError::NoExecutableKind)?;
    if !is_tool_name(&node.content) {
        return Err(ApprovalError::BadName);
    }
    if Tool::from_name(&node.content).is_some() {
        return Err(ApprovalError::TakesABuiltInName);
    }
    Ok(kind)
}

/// The preview of `node`; `approved` is left false for the caller to set.
fn build_preview(node: &Node, chain: &[String]) -> Result<ApprovalPreview, ApprovalError> {
    let kind = check_shape(node, chain)?;
    let base = CoreNodeType::Tool.as_str();
    let description = text(&node.properties, base, "description");
    if description.len() > MAX_PREVIEW_JSON_BYTES {
        return Err(ApprovalError::TooLarge);
    }
    let schema = node
        .properties
        .get(base)
        .and_then(|b| b.get("parameter_schema"))
        .filter(|s| !s.is_null())
        .ok_or(ApprovalError::BadParameterSchema)?;
    if nodespace_types::check_closed_root(schema).is_err() {
        return Err(ApprovalError::BadParameterSchema);
    }
    // Every bucket of the chain but `tool`'s own: the binding fields.
    let mut binding = serde_json::Map::new();
    for node_type in chain.iter().filter(|t| t.as_str() != base) {
        if let Some(bucket) = node.properties.get(node_type) {
            binding.insert(node_type.clone(), bucket.clone());
        }
    }
    let credentials = credentials_of(node.properties.get(kind.bucket()).unwrap_or(&Value::Null));
    let digest = digest_of(
        &node.id,
        &node.content,
        &description,
        &tool_binding_hash(chain, &node.properties),
    );
    Ok(ApprovalPreview {
        node_id: node.id.clone(),
        name: node.content.clone(),
        description,
        kind,
        binding_json: pretty(&Value::Object(binding))?,
        parameter_schema_json: pretty(schema)?,
        credentials,
        digest,
        approved: false,
    })
}

/// What approving `node_id` would approve. Writes nothing.
pub async fn preview(
    node_service: &NodeService,
    secrets: &SecretStore,
    node_id: &str,
) -> Result<ApprovalPreview, ApprovalError> {
    let (node, chain) = load(node_service, node_id).await?;
    let mut preview = build_preview(&node, &chain)?;
    // Already approved: the offering gate passes it under this device's key.
    // No key is created for a preview, and an unreadable key is no key.
    preview.approved = matches!(secrets.approval_key(), Ok(Some(key))
        if assess(&node.id, &node.content, &chain, &node.properties, Some(&key)).is_ok());
    Ok(preview)
}

/// Approves `node_id` on this device if it still produces `digest`: enables it
/// and writes the approval tag. Creates the device key if there is none (never
/// replaces one).
pub async fn approve(
    node_service: &NodeService,
    secrets: &SecretStore,
    node_id: &str,
    digest: &str,
) -> Result<(), ApprovalError> {
    let (node, chain) = load(node_service, node_id).await?;
    let shown = build_preview(&node, &chain)?;
    if !constant_time_eq(&shown.digest, digest) {
        return Err(ApprovalError::Changed);
    }
    let key = secrets
        .provision_approval_key()
        .map_err(|e| ApprovalError::Key(e.to_string()))?;
    let tag = remote_tools::approve(&node, &chain, &key);
    // The tool must pass the gate as it will be after this write.
    let mut after = node.properties.clone();
    after[CoreNodeType::Tool.as_str()]["enabled"] = json!(true);
    after[CoreNodeType::Tool.as_str()]["approved_binding"] = json!(tag);
    if assess(&node.id, &node.content, &chain, &after, Some(&key)).is_err() {
        return Err(ApprovalError::NotARemoteTool);
    }
    node_service
        .update_node(
            &node.id,
            node.version,
            NodeUpdate::new().with_properties(json!({"enabled": true, "approved_binding": tag})),
        )
        .await
        .map(|_| ())
        .map_err(|e| match e {
            NodeServiceError::VersionConflict { .. } => ApprovalError::Conflict,
            other => ApprovalError::Write(other.to_string()),
        })
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() != b.len()) as u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::secrets::InMemorySecretBackend;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn rig() -> (Arc<NodeService>, SecretStore, TempDir) {
        let tmp = TempDir::new().unwrap();
        let mut store: Arc<SqliteStore> =
            Arc::new(SqliteStore::new(tmp.path().join("t.db")).await.unwrap());
        let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
        let secrets = SecretStore::new(Arc::new(InMemorySecretBackend::default()));
        (ns, secrets, tmp)
    }

    fn draft() -> Value {
        json!({
            "program": "git",
            "args": ["log", "{q}"],
            "env": [{"name": "TOKEN", "secret": "keychain:tok"}],
            "description": "Looks a thing up",
            "parameter_schema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {"q": {"type": "string"}}
            },
            "enabled": false
        })
    }

    async fn tool(ns: &NodeService, name: &str) -> String {
        ns.create_node(Node::new("tool-cli".to_string(), name.to_string(), draft()))
            .await
            .unwrap()
    }

    async fn offered(ns: &NodeService, secrets: &SecretStore, id: &str) -> bool {
        let Some(key) = secrets.approval_key().unwrap() else {
            return false;
        };
        let (node, chain) = load(ns, id).await.unwrap();
        assess(
            &node.id,
            &node.content,
            &chain,
            &node.properties,
            Some(&key),
        )
        .is_ok()
    }

    #[tokio::test]
    async fn the_preview_shows_everything_that_is_approved_and_writes_nothing() {
        let (ns, secrets, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let before = ns.get_node(&id).await.unwrap().unwrap();
        let p = preview(&ns, &secrets, &id).await.unwrap();
        assert_eq!(p.name, "lookup");
        assert_eq!(p.description, "Looks a thing up");
        assert_eq!(p.kind, RemoteToolKind::Cli);
        assert!(
            p.binding_json.contains("\"program\": \"git\""),
            "{}",
            p.binding_json
        );
        assert!(p.binding_json.contains("{q}"));
        assert!(p.binding_json.contains("keychain:tok"));
        assert!(p.parameter_schema_json.contains("additionalProperties"));
        assert_eq!(p.credentials, ["TOKEN from keychain:tok"]);
        assert!(!p.approved);
        assert_eq!(p.digest.len(), 64);
        // Nothing was written, and no key was made for a look.
        let after = ns.get_node(&id).await.unwrap().unwrap();
        assert_eq!(before.version, after.version);
        assert!(secrets.approval_key().unwrap().is_none());
    }

    #[tokio::test]
    async fn approving_with_the_shown_digest_enables_and_approves_and_the_gate_then_offers_it() {
        let (ns, secrets, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        assert!(!offered(&ns, &secrets, &id).await);
        let p = preview(&ns, &secrets, &id).await.unwrap();
        approve(&ns, &secrets, &id, &p.digest).await.unwrap();
        assert!(offered(&ns, &secrets, &id).await);
        assert!(preview(&ns, &secrets, &id).await.unwrap().approved);
    }

    #[tokio::test]
    async fn a_wrong_digest_approves_nothing() {
        let (ns, secrets, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        for bad in ["", "wrong", &"0".repeat(64)] {
            assert_eq!(
                approve(&ns, &secrets, &id, bad).await,
                Err(ApprovalError::Changed)
            );
        }
        assert!(!offered(&ns, &secrets, &id).await);
        // Refused before a key is made.
        assert!(secrets.approval_key().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_tool_edited_after_it_was_shown_is_not_approved() {
        let (ns, secrets, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let shown = preview(&ns, &secrets, &id).await.unwrap();
        let node = ns.get_node(&id).await.unwrap().unwrap();
        ns.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({"program": "rm"})),
        )
        .await
        .unwrap();
        assert_eq!(
            approve(&ns, &secrets, &id, &shown.digest).await,
            Err(ApprovalError::Changed)
        );
        assert!(!offered(&ns, &secrets, &id).await);
    }

    #[tokio::test]
    async fn a_credential_reference_edit_or_a_rename_also_changes_what_is_shown() {
        let (ns, secrets, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let first = preview(&ns, &secrets, &id).await.unwrap();
        let node = ns.get_node(&id).await.unwrap().unwrap();
        ns.update_node(
            &id,
            node.version,
            NodeUpdate::new()
                .with_properties(json!({"env": [{"name": "TOKEN", "secret": "keychain:other"}]})),
        )
        .await
        .unwrap();
        let second = preview(&ns, &secrets, &id).await.unwrap();
        assert_ne!(first.digest, second.digest);
        assert_eq!(second.credentials, ["TOKEN from keychain:other"]);
        let node = ns.get_node(&id).await.unwrap().unwrap();
        ns.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_content("renamed".to_string()),
        )
        .await
        .unwrap();
        assert_ne!(
            preview(&ns, &secrets, &id).await.unwrap().digest,
            second.digest
        );
    }

    #[tokio::test]
    async fn an_existing_device_key_is_never_replaced_and_a_missing_one_is_made_once() {
        let (ns, secrets, _tmp) = rig().await;
        let existing = secrets.provision_approval_key().unwrap();
        let id = tool(&ns, "lookup").await;
        let p = preview(&ns, &secrets, &id).await.unwrap();
        approve(&ns, &secrets, &id, &p.digest).await.unwrap();
        assert_eq!(
            secrets.approval_key().unwrap().unwrap().as_bytes().to_vec(),
            existing.as_bytes().to_vec()
        );
        // A second tool, a second approval: still the same key.
        let id2 = tool(&ns, "other").await;
        let p2 = preview(&ns, &secrets, &id2).await.unwrap();
        approve(&ns, &secrets, &id2, &p2.digest).await.unwrap();
        assert_eq!(
            secrets.approval_key().unwrap().unwrap().as_bytes().to_vec(),
            existing.as_bytes().to_vec()
        );
        assert!(offered(&ns, &secrets, &id).await && offered(&ns, &secrets, &id2).await);
    }

    #[tokio::test]
    async fn a_changed_binding_after_approval_turns_the_tool_off_again() {
        let (ns, secrets, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let p = preview(&ns, &secrets, &id).await.unwrap();
        approve(&ns, &secrets, &id, &p.digest).await.unwrap();
        let node = ns.get_node(&id).await.unwrap().unwrap();
        ns.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({"program": "rm"})),
        )
        .await
        .unwrap();
        assert!(!offered(&ns, &secrets, &id).await);
        assert!(!preview(&ns, &secrets, &id).await.unwrap().approved);
    }

    #[tokio::test]
    async fn only_a_remote_tool_with_a_plain_unclaimed_name_can_be_approved() {
        let (ns, secrets, _tmp) = rig().await;
        let text = ns
            .create_node(Node::new(
                "text".to_string(),
                "hello".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(
            preview(&ns, &secrets, &text).await.unwrap_err(),
            ApprovalError::NotARemoteTool
        );
        assert_eq!(
            approve(&ns, &secrets, &text, "x").await.unwrap_err(),
            ApprovalError::NotARemoteTool
        );
        assert_eq!(
            preview(&ns, &secrets, "no-such-node").await.unwrap_err(),
            ApprovalError::NotFound
        );
        let builtin = tool(&ns, "get_node").await;
        assert_eq!(
            preview(&ns, &secrets, &builtin).await.unwrap_err(),
            ApprovalError::TakesABuiltInName
        );
        assert!(secrets.approval_key().unwrap().is_none());
    }

    /// A node as it is stored (properties in their owners' buckets), for shapes
    /// the write path already refuses: the approval refuses them on its own and
    /// does not rely on that check.
    fn stored(name: &str, schema: Value) -> (Node, Vec<String>) {
        let node = Node::new(
            "tool-cli".to_string(),
            name.to_string(),
            json!({
                "tool": {
                    "description": "Looks a thing up",
                    "parameter_schema": schema,
                    "enabled": false
                },
                "tool-cli": {"program": "git", "args": ["log"]}
            }),
        );
        (node, vec!["tool-cli".to_string(), "tool".to_string()])
    }

    fn closed_schema() -> Value {
        json!({"type": "object", "additionalProperties": false, "properties": {}})
    }

    #[test]
    fn a_name_that_is_not_a_plain_identifier_cannot_be_approved() {
        let (node, chain) = stored("lookup", closed_schema());
        assert!(build_preview(&node, &chain).is_ok());
        for bad in ["has space", "", "1starts_with_a_digit", "dot.ted"] {
            let (node, chain) = stored(bad, closed_schema());
            assert_eq!(
                build_preview(&node, &chain).unwrap_err(),
                ApprovalError::BadName,
                "{bad:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_binding_too_large_to_show_whole_cannot_be_approved() {
        let (ns, secrets, _tmp) = rig().await;
        let mut props = draft();
        props["args"] = json!((0..400)
            .map(|i| format!("{i}{}", "x".repeat(60)))
            .collect::<Vec<_>>());
        let id = ns
            .create_node(Node::new("tool-cli".to_string(), "big".to_string(), props))
            .await
            .unwrap();
        assert_eq!(
            preview(&ns, &secrets, &id).await.unwrap_err(),
            ApprovalError::TooLarge
        );
    }

    #[test]
    fn a_tool_without_a_closed_schema_cannot_be_approved() {
        for schema in [json!({"type": "object"}), Value::Null] {
            let (node, chain) = stored("open", schema.clone());
            assert_eq!(
                build_preview(&node, &chain).unwrap_err(),
                ApprovalError::BadParameterSchema,
                "{schema}"
            );
        }
        let (node, chain) = stored("open", closed_schema());
        assert!(build_preview(&node, &chain).is_ok());
    }
}
