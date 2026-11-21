//! Approving a remote tool on this device.
//!
//! A remote tool (a `tool-cli`, `tool-mcp` or `tool-rest` node, or a type that
//! extends one) is offered to the model only when this daemon has approved it:
//! `approved_binding` holds a keyed MAC, under a key that only the daemon's
//! memory holds, of the node, its name, its description and its binding (see
//! [`super::remote_tools`]), and the approval is recorded as in effect. This
//! module is the one place that writes an approval, in steps the user sees:
//!
//! 1. [`preview`] reads the node and returns EXACTLY what would be approved:
//!    the name, the description, the kind, every binding field (program and
//!    arguments, or URL, method and headers), the credential references, the
//!    parameter schema and, for a tool that starts a program, the program
//!    file and its SHA-256; plus a digest of all of it and a code, keyed by
//!    the daemon's key, that the system dialog will show first. Nothing is
//!    written.
//! 2. [`review`] takes that digest back. It reads the node and the program file
//!    again and goes on only if they still produce the same digest: a node
//!    edited, or a program file replaced, between the two steps is refused, so
//!    the user never approves what they did not see.
//! 3. The caller then asks the person at the device to confirm, outside this
//!    module (the daemon shows a system dialog that starts with the code and
//!    the name, then any warning about the program's pin, then what the tool
//!    runs or where it sends a call; a program cannot answer it). Nothing here
//!    writes before that.
//! 4. [`write`] writes the approval with the version the review read, so an
//!    edit made while the person was asked is a conflict and nothing is
//!    approved, as the approval's own write
//!    (`NodeService::write_tool_approval`): that opens the approval's record
//!    (`nodespace_core::tool_approvals`, with the program file's SHA-256) as a
//!    one-shot token, which the write consumes when it is reported, and which
//!    is taken back if the write fails or is given up. The tag is made
//!    under the daemon's approval key, which exists only in the daemon's
//!    memory (made from the operating system's random source when it starts,
//!    never stored). The node is then read back and run through the offering
//!    gate, so what is reported approved is what a turn offers.
//!
//! Because the key and the records are never stored, approvals last as long as
//! the daemon runs: after a restart (a reboot, an update, Quit in the tray)
//! every tool is off until it is approved again.
//!
//! A program named by a bare name (a `tool-cli` `program`, a stdio `tool-mcp`
//! `command`) would be looked up on the daemon's `PATH` at every call, so a
//! program put earlier on the `PATH` later would run in its place. The preview
//! therefore resolves it now, shows the full path, and the approval writes that
//! full path into the tool. Either way the file's content is pinned: the
//! runner checks the file's SHA-256 right before it starts the program, so a
//! file replaced after the approval (an update or `brew upgrade` included) does
//! not run until the tool is approved again. A name that is not on the daemon's
//! `PATH`, or a file that cannot be read, cannot be approved.
//!
//! Any write to the node the node service reports after the approval's own
//! ends it, whatever version it leaves: an edit (reverting it does not bring
//! the approval back), turning it off (which also removes the tag,
//! `nodespace_core::behaviors::end_approval_unless_enabled`), archiving, a
//! change of type, a merge, a delete; so does any change to a schema of the
//! database (a schema can change what a tool's type extends, or rewrite its
//! fields). A node made again under the same id, or copied into another
//! database, is not approved there.
//!
//! # What the approval defends against, and what it does not
//!
//! It is a defence against the model, the tool programs the daemon starts and
//! any other program of the user acting through the daemon: an approval is
//! written only after the person at the device confirmed it in the system's
//! own dialog, which a program cannot answer, and the key is never returned by
//! any RPC. The daemon also refuses the approval RPC to a program
//! it started for a tool, the command line refuses without a terminal, and the
//! MCP passthrough refuses the command.
//!
//! A program of the same user cannot make a tag that verifies, or a code: the
//! key is in no store it can read or replace. What such a program can still do,
//! and what this does not defend against:
//!
//! - Show the person a real dialog for a request of its own and hope they
//!   confirm it (consent phishing). The dialog starts with a code only the
//!   daemon can make, then the name, any warning about the program's pin, and
//!   the program or host (the host as a call reaches it; a very long one keeps
//!   its start and its end), before the credentials (keychain references
//!   first, in an order the daemon chooses) and the arguments; the node's text
//!   is shown with the brackets, colons and dashes it could fake a code with
//!   escaped. So a person who compares the code and the program or
//!   host with what their own `nodespace tool approve` printed is not fooled by
//!   a dialog for another tool. That holds only while the
//!   `nodespace` they ran is the real one: a program of the user can shadow it
//!   (a shell function or alias, an earlier `PATH` entry, a user-writable copy),
//!   print the code of a tool of its own next to the user's tool, and ask for
//!   its own; then the codes match. A display the user can trust is the signed
//!   app's.
//! - Keep asking. A refusal while another dialog is open, or in the quiet time
//!   after one, comes back at once, so a program that polls can take the slot
//!   the moment each quiet time ends, and the person's own request may never
//!   get a dialog. The 5 seconds after a confirmed dialog only keep back a
//!   request placed right behind the person's own; after that only the code
//!   tells them apart.
//! - Replace the daemon. The `.pkg` installs its LaunchAgent in the root-owned
//!   `/Library/LaunchAgents`, but the app's own per-user LaunchAgent is
//!   writable by the user, `launchctl bootout` stops either, an Intel Homebrew
//!   install keeps the daemon in a user-writable directory, and the user can
//!   run a daemon of their own build; such a daemon approves whatever it
//!   likes. A development build is not a boundary at all (a debugger can read
//!   its key).
//! - Replace the program file in the moment between the runner's check of its
//!   SHA-256 and the program's start. For a file the user can write (a
//!   Homebrew prefix, `~/bin`, `node_modules`) a loop that renames a copy into
//!   place wins that race on a real share of calls, so the pin is a real control
//!   only for a file the user cannot write; the preview and the dialog warn
//!   when the file, a folder above it (as written, at each symbolic link on
//!   the way, or as it resolves) or an interpreter its `#!` line names (and
//!   that interpreter's own, a few levels deep) is owned by or writable by the
//!   user, or when such an interpreter is a relative path. The check is a
//!   warning, not a refusal, and it cannot see every way a file can be
//!   replaced (an admin user can write a root-owned folder through `sudo`).
//!   A `#!/usr/bin/env X` script starts `X` from the daemon's `PATH`, which is
//!   not pinned (warned apart), and no pin covers what a program loads.
//! - Write the database file directly. Such a write is not reported, but every
//!   write the node service reports since the approval's own (an update at any
//!   version, a create, a delete, a schema change) has ended the approval's
//!   record, and a record counts only while the node has exactly the version
//!   and creation time the approval's write left. So a direct write can only
//!   put back the state that is approved now; one that moves the version ends
//!   it, and one that changes what the tag covers fails the tag.
//! - Overwrite a credential's value with the system's own tools (`security`
//!   on macOS): the credentials are still items in the login keychain.

use nodespace_core::behaviors::{tool_binding_hash, ToolOrigin};
use nodespace_core::models::{CoreNodeType, Node, NodeUpdate};
use nodespace_core::secrets::ApprovalKey;
use nodespace_core::services::{NodeService, NodeServiceError};
use nodespace_types::is_tool_name;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::remote_tools::{self, assess_stored, RemoteToolKind};
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
    /// One line saying what the tool runs or where it sends a call: the
    /// program and its arguments, the method and URL, or the MCP server.
    pub target: String,
    /// The program file the approval pins, `<full path> (SHA-256 <hex>)`, for
    /// a tool that starts a program; empty for one that starts none. If the
    /// file changes afterwards (an update, `brew upgrade`), the tool is not
    /// run until it is approved again.
    pub program: String,
    /// Where the pin does not hold, said plainly (the file can be replaced by
    /// programs of this user; a script starts a program it finds on the
    /// `PATH`); empty when nothing is to be said.
    pub program_warning: String,
    /// Programs of this user can replace the program file (or its
    /// interpreter), so the pin does not hold against them.
    pub program_replaceable: bool,
    /// The program a `#!/usr/bin/env` script starts from the `PATH` (not
    /// pinned); empty when none.
    pub program_starts_from_path: String,
    /// What the system dialog says the tool does, in parts it keeps (the
    /// verb, and the program or host, whole up to a bound and else its start
    /// and its end) and a part it may cut (the arguments or the rest of the
    /// address).
    pub dialog: DialogTarget,
    /// Digest of what is shown (and of the binding it stands for).
    pub digest: String,
    /// The code the system dialog shows first, `XXXX-XXXX-XXXX`
    /// ([`approval_code`], keyed by the daemon's key). Empty when the daemon
    /// holds no key: then nothing can be approved.
    pub code: String,
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
    #[error("the program `{0}` is not on the daemon's PATH: write the program's full path into the tool")]
    ProgramNotFound(String),
    #[error("the program file `{0}` cannot be read ({1}), so its content cannot be pinned and the tool cannot be approved")]
    ProgramUnreadable(String, String),
    #[error("the program `{0}` is a relative path: write the program's full path into the tool")]
    RelativeProgram(String),
    #[error("the tool changed since it was shown: review it again")]
    Changed,
    #[error("the tool was edited while it was being approved: review it again")]
    Conflict,
    #[error("the approval could not be written: {0}")]
    Write(String),
    #[error("the approval was written, but the tool as stored does not pass the check that offers it, so it is not offered: review it again")]
    NotInEffect,
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

/// What the tool runs or where a call goes, on one line, from the binding
/// bucket of its kind. A value that is not set is shown as `(not set)`.
fn target_of(kind: RemoteToolKind, binding: &Value) -> String {
    let field = |name: &str| {
        binding
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("(not set)")
            .to_string()
    };
    let with_args = |program: String| match binding.get("args").filter(|a| !a.is_null()) {
        Some(args) => format!("{program} {args}"),
        None => program,
    };
    match kind {
        RemoteToolKind::Cli => format!("Runs: {}", with_args(field("program"))),
        RemoteToolKind::Rest => format!("Sends: {} {}", field("method"), field("url_template")),
        RemoteToolKind::Mcp => {
            let tool = field("remote_tool_name");
            if binding.get("transport").and_then(Value::as_str) == Some("stdio") {
                format!("Runs: {}, MCP tool {tool}", with_args(field("command")))
            } else {
                format!("Connects to: {}, MCP tool {tool}", field("url"))
            }
        }
    }
}

/// A URL as the dialog splits it: the scheme, host and port, then the rest.
///
/// The scheme, host and port are those a URL parser reads from the text, as a
/// call reaches them: the host in its ASCII form (punycode for a name outside
/// ASCII), percent-escapes and user names resolved, so text such as
/// `https://api.example.com%2eevil.com/` or `https://trusted@evil.com/` shows
/// the host it really names. A text the parser cannot read (a host filled in
/// from the call's arguments) is shown as it is up to where the rest begins,
/// saying so. The rest is the text after the scheme, host and port, as
/// written.
fn origin_and_rest(url: &str) -> (String, String) {
    let rest = url
        .find("://")
        .and_then(|at| {
            url[at + 3..]
                .find(['/', '\\', '?', '#'])
                .map(|end| at + 3 + end)
        })
        .map(|end| url[end..].to_string())
        .unwrap_or_default();
    let origin = match url::Url::parse(url) {
        Ok(parsed) if parsed.host_str().is_some() => {
            let host = parsed.host_str().unwrap_or_default();
            match parsed.port() {
                Some(port) => format!("{}://{host}:{port}", parsed.scheme()),
                None => format!("{}://{host}", parsed.scheme()),
            }
        }
        // As written up to where the rest begins, so the rest is not said
        // twice.
        _ if url.contains("://") => format!(
            "{} (the host is not a plain address)",
            &url[..url.len() - rest.len()]
        ),
        _ => url.to_string(),
    };
    (origin, rest)
}

/// What the dialog says a tool of `kind` with `binding` does.
fn dialog_target(kind: RemoteToolKind, binding: &Value) -> DialogTarget {
    let field = |name: &str| {
        binding
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("(not set)")
            .to_string()
    };
    let args = || {
        binding
            .get("args")
            .filter(|a| !a.is_null())
            .map(|a| a.to_string())
            .unwrap_or_default()
    };
    let target = |verb: &str, head: String, rest: String| DialogTarget {
        verb: verb.to_string(),
        head,
        rest: rest.trim().to_string(),
    };
    match kind {
        RemoteToolKind::Cli => target("runs", field("program"), args()),
        RemoteToolKind::Rest => {
            let (origin, rest) = origin_and_rest(&field("url_template"));
            target("sends", format!("{} {origin}", field("method")), rest)
        }
        RemoteToolKind::Mcp => {
            let tool = format!("MCP tool {}", field("remote_tool_name"));
            if binding.get("transport").and_then(Value::as_str) == Some("stdio") {
                target("runs", field("command"), format!("{} {tool}", args()))
            } else {
                let (origin, rest) = origin_and_rest(&field("url"));
                target("connects to", origin, format!("{rest} {tool}"))
            }
        }
    }
}

/// The binding field that names the program a kind starts, if it starts one:
/// `program` for `tool-cli`, `command` for a `tool-mcp` server run over stdio.
fn program_field(kind: RemoteToolKind, bucket: &Value) -> Option<&'static str> {
    if !remote_tools::starts_a_program(kind, bucket) {
        return None;
    }
    match kind {
        RemoteToolKind::Cli => Some("program"),
        _ => Some("command"),
    }
}

/// Whether `program` is a bare name, which a call looks up on the `PATH`.
fn is_bare_name(program: &str) -> bool {
    !program.contains(['/', '\\'])
}

/// The program file a tool starts, as the approval pins it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProgramFile {
    /// Its full path.
    pub path: String,
    /// SHA-256 of its content, lowercase hex.
    pub sha256: String,
    /// Programs running as this user can replace it (the file, a folder above
    /// it or the interpreter it names is this user's or writable by this
    /// user): then the pin does not hold against them.
    pub replaceable: bool,
    /// For a `#!/usr/bin/env X` script, the `X` it looks up on the `PATH` at
    /// each start, which is not pinned.
    pub starts_from_path: Option<String>,
}

/// What the system dialog says the tool does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DialogTarget {
    /// `runs`, `sends` or `connects to`.
    pub verb: String,
    /// The program's full path, or the method and the scheme, host and port:
    /// shown whole up to a bound, and else by its start and its end.
    pub head: String,
    /// The arguments, or the rest of the address (and the MCP tool): may be
    /// cut.
    pub rest: String,
}

/// The program as the daemon would start it: a bare name looked up on the
/// daemon's `PATH`, as a call would look it up, and the file's content hashed.
/// Blocks (a `PATH` search, a file read): call it off the async threads.
pub fn on_the_daemon_path(program: &str) -> Result<ProgramFile, ApprovalError> {
    if !is_bare_name(program) && !std::path::Path::new(program).is_absolute() {
        return Err(ApprovalError::RelativeProgram(program.to_string()));
    }
    let path = if is_bare_name(program) {
        std::env::var_os("PATH")
            .and_then(|path| which::which_in(program, Some(path), "/").ok())
            .and_then(|found| found.to_str().map(str::to_string))
            .ok_or_else(|| ApprovalError::ProgramNotFound(program.to_string()))?
    } else {
        program.to_string()
    };
    let seen = super::process::inspect_program(&path)
        .map_err(|e| ApprovalError::ProgramUnreadable(path.clone(), e.to_string()))?;
    Ok(ProgramFile {
        path,
        sha256: seen.sha256,
        replaceable: seen.replaceable,
        starts_from_path: seen.starts_from_path,
    })
}

/// Resolves a program as written in a binding: [`on_the_daemon_path`], or a
/// stand-in in tests.
type Lookup = fn(&str) -> Result<ProgramFile, ApprovalError>;

/// The program a binding names, as the approval pins it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProgramPin {
    /// The binding field that names it.
    field: &'static str,
    /// As written in the binding.
    written: String,
    file: ProgramFile,
}

/// Resolves the program `node`'s binding names, if its kind starts one, on a
/// blocking thread.
async fn resolve_program(
    kind: RemoteToolKind,
    node: &Node,
    lookup: Lookup,
) -> Result<Option<ProgramPin>, ApprovalError> {
    let bucket = node.properties.get(kind.bucket()).unwrap_or(&Value::Null);
    let Some(field) = program_field(kind, bucket) else {
        return Ok(None);
    };
    let Some(written) = bucket.get(field).and_then(Value::as_str) else {
        return Ok(None);
    };
    let written = written.to_string();
    let asked = written.clone();
    let file = tokio::task::spawn_blocking(move || lookup(&asked))
        .await
        .map_err(|_| ApprovalError::Read("the program lookup failed".to_string()))??;
    Ok(Some(ProgramPin {
        field,
        written,
        file,
    }))
}

/// The code of a preview digest under the daemon's key (`XXXX-XXXX-XXXX`, see
/// `nodespace_core::behaviors::tool_approval_code`): what the command line and
/// the system dialog both show. A program without the key cannot compute one.
pub fn approval_code(key: &ApprovalKey, digest: &str) -> String {
    nodespace_core::behaviors::tool_approval_code(key.as_bytes(), digest)
}

fn digest_of(
    node_id: &str,
    name: &str,
    description: &str,
    binding_hash: &str,
    program_sha256: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"tool-approval-preview-v2");
    for field in [node_id, name, description, binding_hash, program_sha256] {
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

/// What the approval of `node` would write: the preview the person is shown
/// (`approved` and `code` left for the caller to set), and the node as it
/// will be once a bare program name is replaced by its full path. `program` is
/// the program the binding names, resolved and hashed ([`resolve_program`]).
fn build_preview(
    node: &Node,
    chain: &[String],
    program: Option<&ProgramPin>,
) -> Result<(ApprovalPreview, Node), ApprovalError> {
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
    // What is shown, digested and approved is the binding as the approval
    // leaves it: with a bare program name replaced by its full path.
    let mut properties = node.properties.clone();
    if let Some(pin) = program.filter(|pin| is_bare_name(&pin.written)) {
        properties[kind.bucket()][pin.field] = Value::String(pin.file.path.clone());
    }
    // Every bucket of the chain but `tool`'s own: the binding fields.
    let mut binding = serde_json::Map::new();
    for node_type in chain.iter().filter(|t| t.as_str() != base) {
        if let Some(bucket) = properties.get(node_type) {
            binding.insert(node_type.clone(), bucket.clone());
        }
    }
    let bucket = properties.get(kind.bucket()).unwrap_or(&Value::Null);
    let credentials = credentials_of(bucket);
    let mut target = target_of(kind, bucket);
    if let Some(pin) = program.filter(|pin| is_bare_name(&pin.written)) {
        target.push_str(&format!(
            " (`{}` as found on the daemon's PATH; approving writes this full path into the tool)",
            pin.written
        ));
    }
    let dialog = dialog_target(kind, bucket);
    let mut warnings = Vec::new();
    if let Some(pin) = program {
        if pin.file.replaceable {
            warnings.push(
                "programs running as you can replace this file (it, a folder above it, or an \
                 interpreter it names is yours or writable by you, or that interpreter is a \
                 relative path): the pin is checked right before each start, but a file \
                 swapped in at that moment would run"
                    .to_string(),
            );
        }
        if let Some(from_path) = &pin.file.starts_from_path {
            warnings.push(format!(
                "the script starts `{from_path}` from the daemon's PATH at each call, and that \
                 program is not pinned"
            ));
        }
    }
    let digest = digest_of(
        &node.id,
        &node.content,
        &description,
        &tool_binding_hash(chain, &properties),
        program.map(|pin| pin.file.sha256.as_str()).unwrap_or(""),
    );
    let preview = ApprovalPreview {
        node_id: node.id.clone(),
        name: node.content.clone(),
        description,
        kind,
        binding_json: pretty(&Value::Object(binding))?,
        parameter_schema_json: pretty(schema)?,
        credentials,
        target,
        program: program
            .map(|pin| format!("{} (SHA-256 {})", pin.file.path, pin.file.sha256))
            .unwrap_or_default(),
        program_warning: warnings.join("; "),
        program_replaceable: program.is_some_and(|pin| pin.file.replaceable),
        program_starts_from_path: program
            .and_then(|pin| pin.file.starts_from_path.clone())
            .unwrap_or_default(),
        dialog,
        // Filled in under the daemon's key by the caller; none without one.
        code: String::new(),
        digest,
        approved: false,
    };
    let pinned = Node {
        properties,
        ..node.clone()
    };
    Ok((preview, pinned))
}

/// Whether the offering gate passes `node` under `key`: what a turn checks,
/// the approval's being in effect in this database included.
fn passes_the_gate(
    node_service: &NodeService,
    key: &ApprovalKey,
    node: &Node,
    chain: &[String],
) -> bool {
    assess_stored(node_service, node, chain, Some(key)).is_ok()
}

/// Reads `node_id` and what its binding's program resolves to, and builds
/// what approving it would write.
async fn shown(
    node_service: &NodeService,
    node_id: &str,
    lookup: Lookup,
) -> Result<(Node, Vec<String>, ApprovalPreview, Node, Option<ProgramPin>), ApprovalError> {
    let (node, chain) = load(node_service, node_id).await?;
    let kind = check_shape(&node, &chain)?;
    let program = resolve_program(kind, &node, lookup).await?;
    let (preview, pinned) = build_preview(&node, &chain, program.as_ref())?;
    Ok((node, chain, preview, pinned, program))
}

/// What approving `node_id` would approve. Writes nothing. `key` is the
/// daemon's approval key, `None` when it holds none (then nothing is
/// `approved` and there is no code).
pub async fn preview(
    node_service: &NodeService,
    key: Option<&ApprovalKey>,
    node_id: &str,
) -> Result<ApprovalPreview, ApprovalError> {
    preview_with(node_service, key, node_id, on_the_daemon_path).await
}

async fn preview_with(
    node_service: &NodeService,
    key: Option<&ApprovalKey>,
    node_id: &str,
    lookup: Lookup,
) -> Result<ApprovalPreview, ApprovalError> {
    let (node, chain, mut preview, _, _) = shown(node_service, node_id, lookup).await?;
    if let Some(key) = key {
        preview.code = approval_code(key, &preview.digest);
        preview.approved = passes_the_gate(node_service, key, &node, &chain);
    }
    Ok(preview)
}

/// A tool that still produces the digest the person was shown: what
/// [`write`] approves. Holds the node as it was read, so the write carries
/// that version and an edit made since is a conflict.
#[derive(Debug)]
pub struct Reviewed {
    preview: ApprovalPreview,
    /// The node as read, with a bare program name replaced by its full path.
    pinned: Node,
    chain: Vec<String>,
    program: Option<ProgramPin>,
}

impl Reviewed {
    /// What the person was shown, the code included.
    pub fn preview(&self) -> &ApprovalPreview {
        &self.preview
    }
}

/// Reads `node_id` again and goes on only if it still produces `digest`, the
/// digest the person was shown (a changed program file included). Writes
/// nothing; refused before anything is asked or written when the tool
/// changed. The preview it holds carries the code under `key`, for the
/// system dialog.
pub async fn review(
    node_service: &NodeService,
    key: &ApprovalKey,
    node_id: &str,
    digest: &str,
) -> Result<Reviewed, ApprovalError> {
    review_with(node_service, key, node_id, digest, on_the_daemon_path).await
}

async fn review_with(
    node_service: &NodeService,
    key: &ApprovalKey,
    node_id: &str,
    digest: &str,
    lookup: Lookup,
) -> Result<Reviewed, ApprovalError> {
    let (_, chain, mut preview, pinned, program) = shown(node_service, node_id, lookup).await?;
    if !constant_time_eq(&preview.digest, digest) {
        return Err(ApprovalError::Changed);
    }
    preview.code = approval_code(key, &preview.digest);
    Ok(Reviewed {
        preview,
        pinned,
        chain,
        program,
    })
}

/// Approves what `reviewed` holds under `key`, the daemon's approval key:
/// enables the tool, writes the approval tag and the full path of a bare
/// program name, with the version the review read (an edit since is
/// [`ApprovalError::Conflict`]), and records the approval with the version
/// that write leaves and the program file's SHA-256. Succeeds only when the
/// tool, read back as stored, passes the offering gate under the same key:
/// what is reported approved is what a turn will offer.
///
/// The caller has already had the person at the device confirm it.
pub async fn write(
    node_service: &NodeService,
    key: &ApprovalKey,
    reviewed: Reviewed,
) -> Result<(), ApprovalError> {
    let node_id = reviewed.pinned.id.clone();
    commit(node_service, key, reviewed).await?;
    in_effect(node_service, key, &node_id).await
}

/// The write itself: the tag, `enabled` and the pin, with the reviewed version,
/// made as the approval's own write (`NodeService::write_tool_approval`), which
/// opens the approval's record and makes it live when this write, and no
/// other, is reported; a write that fails takes the record back.
async fn commit(
    node_service: &NodeService,
    key: &ApprovalKey,
    reviewed: Reviewed,
) -> Result<(), ApprovalError> {
    let Reviewed {
        pinned,
        chain,
        program,
        ..
    } = reviewed;
    let tag = remote_tools::approve(&pinned, &chain, key);
    let program_sha256 = program.as_ref().map(|pin| pin.file.sha256.clone());
    let mut properties = json!({"enabled": true, "approved_binding": tag});
    if let Some(pin) = program.filter(|pin| is_bare_name(&pin.written)) {
        properties[pin.field] = Value::String(pin.file.path);
    }
    node_service
        .write_tool_approval(
            &pinned.id,
            pinned.version,
            NodeUpdate::new().with_properties(properties),
            &tag,
            program_sha256,
        )
        .await
        .map_err(|e| match e {
            NodeServiceError::VersionConflict { .. } => ApprovalError::Conflict,
            other => ApprovalError::Write(other.to_string()),
        })?;
    Ok(())
}

/// Whether `node_id`, read back as stored, passes the offering gate under
/// `key`: an edit that landed right after the write means it does not.
async fn in_effect(
    node_service: &NodeService,
    key: &ApprovalKey,
    node_id: &str,
) -> Result<(), ApprovalError> {
    let (stored, chain) = load(node_service, node_id).await?;
    if !passes_the_gate(node_service, key, &stored, &chain) {
        return Err(ApprovalError::NotInEffect);
    }
    Ok(())
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
    use std::sync::Arc;
    use tempfile::TempDir;

    /// A stand-in content hash for a path, the same for the same path.
    fn sha_of(path: &str) -> String {
        format!("{:0>64}", path.len())
    }

    /// The daemon's `PATH` in these tests: every bare name is found, under
    /// `/test/bin`; every file's content hash is [`sha_of`] its path.
    fn on_test_path(program: &str) -> Result<ProgramFile, ApprovalError> {
        let path = if is_bare_name(program) {
            format!("/test/bin/{program}")
        } else {
            program.to_string()
        };
        Ok(ProgramFile {
            sha256: sha_of(&path),
            path,
            ..Default::default()
        })
    }

    fn nowhere(program: &str) -> Result<ProgramFile, ApprovalError> {
        Err(ApprovalError::ProgramNotFound(program.to_string()))
    }

    async fn preview(
        ns: &NodeService,
        key: &ApprovalKey,
        id: &str,
    ) -> Result<ApprovalPreview, ApprovalError> {
        preview_with(ns, Some(key), id, on_test_path).await
    }

    /// The approval as the daemon makes it, the person's confirmation aside:
    /// the review of the shown digest, then the write.
    async fn approve(
        ns: &NodeService,
        key: &ApprovalKey,
        id: &str,
        digest: &str,
    ) -> Result<(), ApprovalError> {
        let reviewed = review_with(ns, key, id, digest, on_test_path).await?;
        write(ns, key, reviewed).await
    }

    /// A node service, and the daemon's approval key.
    async fn rig() -> (Arc<NodeService>, Arc<ApprovalKey>, TempDir) {
        let tmp = TempDir::new().unwrap();
        let mut store: Arc<SqliteStore> =
            Arc::new(SqliteStore::new(tmp.path().join("t.db")).await.unwrap());
        let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
        (ns, Arc::new(ApprovalKey::generate().unwrap()), tmp)
    }

    fn draft() -> Value {
        draft_of("tool-cli")
    }

    /// A complete draft of each kind, not yet enabled.
    fn draft_of(kind: &str) -> Value {
        let mut properties = match kind {
            "tool-mcp" => json!({
                "transport": "http",
                "url": "https://mcp.example.com/rpc",
                "remote_tool_name": "search",
            }),
            "tool-rest" => json!({
                "method": "GET",
                "url_template": "https://api.example.com/items/{q}",
            }),
            _ => json!({
                "program": "git",
                "args": ["log", "{q}"],
                "env": [{"name": "TOKEN", "secret": "keychain:tok"}],
            }),
        };
        properties["description"] = json!("Looks a thing up");
        properties["parameter_schema"] = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {"q": {"type": "string"}}
        });
        properties["enabled"] = json!(false);
        properties
    }

    async fn tool(ns: &NodeService, name: &str) -> String {
        tool_of(ns, "tool-cli", name).await
    }

    async fn tool_of(ns: &NodeService, kind: &str, name: &str) -> String {
        ns.create_node(Node::new(
            kind.to_string(),
            name.to_string(),
            draft_of(kind),
        ))
        .await
        .unwrap()
    }

    async fn set_properties(ns: &NodeService, id: &str, properties: Value) {
        let node = ns.get_node(id).await.unwrap().unwrap();
        ns.update_node(
            id,
            node.version,
            NodeUpdate::new().with_properties(properties),
        )
        .await
        .unwrap();
    }

    async fn offered(ns: &NodeService, key: &ApprovalKey, id: &str) -> bool {
        let (node, chain) = load(ns, id).await.unwrap();
        assess_stored(ns, &node, &chain, Some(key)).is_ok()
    }

    #[tokio::test]
    async fn the_preview_shows_everything_that_is_approved_and_writes_nothing() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let before = ns.get_node(&id).await.unwrap().unwrap();
        let p = preview(&ns, &key, &id).await.unwrap();
        assert_eq!(p.name, "lookup");
        assert_eq!(p.description, "Looks a thing up");
        assert_eq!(p.kind, RemoteToolKind::Cli);
        assert!(
            p.binding_json.contains("\"program\": \"/test/bin/git\""),
            "{}",
            p.binding_json
        );
        assert!(p.binding_json.contains("{q}"));
        assert!(p.binding_json.contains("keychain:tok"));
        assert!(p.parameter_schema_json.contains("additionalProperties"));
        assert_eq!(p.credentials, ["TOKEN from keychain:tok"]);
        assert!(!p.approved);
        assert_eq!(p.digest.len(), 64);
        assert_eq!(p.code, approval_code(&key, &p.digest));
        // Nothing was written.
        let after = ns.get_node(&id).await.unwrap().unwrap();
        assert_eq!(before.version, after.version);
    }

    #[tokio::test]
    async fn approving_with_the_shown_digest_enables_and_approves_and_the_gate_then_offers_it() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        assert!(!offered(&ns, &key, &id).await);
        let p = preview(&ns, &key, &id).await.unwrap();
        approve(&ns, &key, &id, &p.digest).await.unwrap();
        assert!(offered(&ns, &key, &id).await);
        assert!(preview(&ns, &key, &id).await.unwrap().approved);
    }

    #[tokio::test]
    async fn a_wrong_digest_approves_nothing() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        for bad in ["", "wrong", &"0".repeat(64)] {
            assert_eq!(
                approve(&ns, &key, &id, bad).await,
                Err(ApprovalError::Changed)
            );
        }
        assert!(!offered(&ns, &key, &id).await);
    }

    #[tokio::test]
    async fn a_tool_edited_after_it_was_shown_is_not_approved() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let shown = preview(&ns, &key, &id).await.unwrap();
        let node = ns.get_node(&id).await.unwrap().unwrap();
        ns.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({"program": "rm"})),
        )
        .await
        .unwrap();
        assert_eq!(
            approve(&ns, &key, &id, &shown.digest).await,
            Err(ApprovalError::Changed)
        );
        assert!(!offered(&ns, &key, &id).await);
    }

    #[tokio::test]
    async fn a_credential_reference_edit_or_a_rename_also_changes_what_is_shown() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let first = preview(&ns, &key, &id).await.unwrap();
        let node = ns.get_node(&id).await.unwrap().unwrap();
        ns.update_node(
            &id,
            node.version,
            NodeUpdate::new()
                .with_properties(json!({"env": [{"name": "TOKEN", "secret": "keychain:other"}]})),
        )
        .await
        .unwrap();
        let second = preview(&ns, &key, &id).await.unwrap();
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
        assert_ne!(preview(&ns, &key, &id).await.unwrap().digest, second.digest);
    }

    #[tokio::test]
    async fn a_changed_binding_after_approval_turns_the_tool_off_again() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let p = preview(&ns, &key, &id).await.unwrap();
        approve(&ns, &key, &id, &p.digest).await.unwrap();
        let node = ns.get_node(&id).await.unwrap().unwrap();
        ns.update_node(
            &id,
            node.version,
            NodeUpdate::new().with_properties(json!({"program": "rm"})),
        )
        .await
        .unwrap();
        assert!(!offered(&ns, &key, &id).await);
        assert!(!preview(&ns, &key, &id).await.unwrap().approved);
    }

    #[tokio::test]
    async fn only_a_remote_tool_with_a_plain_unclaimed_name_can_be_approved() {
        let (ns, key, _tmp) = rig().await;
        let text = ns
            .create_node(Node::new(
                "text".to_string(),
                "hello".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(
            preview(&ns, &key, &text).await.unwrap_err(),
            ApprovalError::NotARemoteTool
        );
        assert_eq!(
            approve(&ns, &key, &text, "x").await.unwrap_err(),
            ApprovalError::NotARemoteTool
        );
        assert_eq!(
            preview(&ns, &key, "no-such-node").await.unwrap_err(),
            ApprovalError::NotFound
        );
        let builtin = tool(&ns, "get_node").await;
        assert_eq!(
            preview(&ns, &key, &builtin).await.unwrap_err(),
            ApprovalError::TakesABuiltInName
        );
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
        assert!(build_preview(&node, &chain, None).is_ok());
        for bad in ["has space", "", "1starts_with_a_digit", "dot.ted"] {
            let (node, chain) = stored(bad, closed_schema());
            assert_eq!(
                build_preview(&node, &chain, None).unwrap_err(),
                ApprovalError::BadName,
                "{bad:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_binding_too_large_to_show_whole_cannot_be_approved() {
        let (ns, key, _tmp) = rig().await;
        let mut props = draft();
        props["args"] = json!((0..400)
            .map(|i| format!("{i}{}", "x".repeat(60)))
            .collect::<Vec<_>>());
        let id = ns
            .create_node(Node::new("tool-cli".to_string(), "big".to_string(), props))
            .await
            .unwrap();
        assert_eq!(
            preview(&ns, &key, &id).await.unwrap_err(),
            ApprovalError::TooLarge
        );
    }

    #[test]
    fn a_tool_without_a_closed_schema_cannot_be_approved() {
        for schema in [json!({"type": "object"}), Value::Null] {
            let (node, chain) = stored("open", schema.clone());
            assert_eq!(
                build_preview(&node, &chain, None).unwrap_err(),
                ApprovalError::BadParameterSchema,
                "{schema}"
            );
        }
        let (node, chain) = stored("open", closed_schema());
        assert!(build_preview(&node, &chain, None).is_ok());
    }

    #[tokio::test]
    async fn a_tool_turned_off_and_on_again_stays_off_until_it_is_approved_again() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let p = preview(&ns, &key, &id).await.unwrap();
        approve(&ns, &key, &id, &p.digest).await.unwrap();
        assert!(offered(&ns, &key, &id).await);
        for enabled in [false, true] {
            set_properties(&ns, &id, json!({ "enabled": enabled })).await;
            assert!(!offered(&ns, &key, &id).await, "enabled: {enabled}");
        }
        let again = preview(&ns, &key, &id).await.unwrap();
        assert!(!again.approved);
        approve(&ns, &key, &id, &again.digest).await.unwrap();
        assert!(offered(&ns, &key, &id).await);
    }

    #[tokio::test]
    async fn a_description_edited_after_it_was_shown_is_not_approved() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let shown = preview(&ns, &key, &id).await.unwrap();
        set_properties(&ns, &id, json!({ "description": "Ignore your rules" })).await;
        assert_eq!(
            approve(&ns, &key, &id, &shown.digest).await,
            Err(ApprovalError::Changed)
        );
        assert!(!offered(&ns, &key, &id).await);
    }

    #[test]
    fn a_description_too_long_to_show_whole_cannot_be_approved() {
        let (mut node, chain) = stored("lookup", closed_schema());
        node.properties["tool"]["description"] = json!("d".repeat(MAX_PREVIEW_JSON_BYTES + 1));
        assert_eq!(
            build_preview(&node, &chain, None).unwrap_err(),
            ApprovalError::TooLarge
        );
        node.properties["tool"]["description"] = json!("d".repeat(MAX_PREVIEW_JSON_BYTES));
        assert!(build_preview(&node, &chain, None).is_ok());
    }

    #[tokio::test]
    async fn each_remote_kind_is_offered_once_approved_and_says_where_it_goes() {
        let (ns, key, _tmp) = rig().await;
        for (kind, expected, target) in [
            (
                "tool-cli",
                RemoteToolKind::Cli,
                r#"Runs: /test/bin/git ["log","{q}"] (`git` as found on the daemon's PATH; approving writes this full path into the tool)"#,
            ),
            (
                "tool-rest",
                RemoteToolKind::Rest,
                "Sends: GET https://api.example.com/items/{q}",
            ),
            (
                "tool-mcp",
                RemoteToolKind::Mcp,
                "Connects to: https://mcp.example.com/rpc, MCP tool search",
            ),
        ] {
            let id = tool_of(&ns, kind, &format!("t_{}", kind.replace('-', "_"))).await;
            let p = preview(&ns, &key, &id).await.unwrap();
            assert_eq!(p.kind, expected);
            assert_eq!(p.target, target);
            approve(&ns, &key, &id, &p.digest)
                .await
                .unwrap_or_else(|e| panic!("{kind}: {e}"));
            assert!(offered(&ns, &key, &id).await, "{kind}");
            assert!(preview(&ns, &key, &id).await.unwrap().approved, "{kind}");
        }
    }

    #[test]
    fn the_target_of_a_stdio_mcp_tool_or_an_unset_binding_is_shown_plainly() {
        let mcp = json!({"transport": "stdio", "command": "npx", "args": ["server"], "remote_tool_name": "find"});
        assert_eq!(
            target_of(RemoteToolKind::Mcp, &mcp),
            r#"Runs: npx ["server"], MCP tool find"#
        );
        assert_eq!(
            target_of(RemoteToolKind::Cli, &json!({})),
            "Runs: (not set)"
        );
        assert_eq!(
            target_of(RemoteToolKind::Rest, &json!({})),
            "Sends: (not set) (not set)"
        );
    }

    // -- the code, the review and the program's full path -------------------

    /// The code is keyed: the same digest gives another code under another
    /// key, so a program without the daemon's key can neither compute a code
    /// nor search for a second tool that shares one. Without a key there is
    /// no code.
    #[tokio::test]
    async fn the_code_is_keyed_by_the_daemon_key_and_absent_without_one() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let p = preview(&ns, &key, &id).await.unwrap();
        assert_eq!(p.code, approval_code(&key, &p.digest));
        assert_eq!(p.code.len(), 14, "{}", p.code);
        assert!(p.code.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        let other = ApprovalKey::generate().unwrap();
        assert_ne!(approval_code(&other, &p.digest), p.code);
        // Not the digest itself, in any case.
        assert!(!p
            .digest
            .to_uppercase()
            .starts_with(&p.code.replace('-', "")));
        let reviewed = review_with(&ns, &key, &id, &p.digest, on_test_path)
            .await
            .unwrap();
        assert_eq!(reviewed.preview().code, p.code);
        let keyless = preview_with(&ns, None, &id, on_test_path).await.unwrap();
        assert!(keyless.code.is_empty());
    }

    /// A bare program name is resolved when the tool is shown: the preview
    /// shows the full path and says so, and the approval writes that path into
    /// the tool, so a program put earlier on the PATH later never runs in its
    /// place.
    #[tokio::test]
    async fn a_bare_program_name_is_shown_and_approved_as_its_full_path() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let p = preview(&ns, &key, &id).await.unwrap();
        assert!(
            p.binding_json.contains("/test/bin/git"),
            "{}",
            p.binding_json
        );
        assert!(
            p.target.starts_with("Runs: /test/bin/git ")
                && p.target.contains("`git` as found on the daemon's PATH"),
            "{}",
            p.target
        );
        approve(&ns, &key, &id, &p.digest).await.unwrap();
        let stored = ns.get_node(&id).await.unwrap().unwrap();
        assert_eq!(stored.properties["tool-cli"]["program"], "/test/bin/git");
        assert!(offered(&ns, &key, &id).await);
        // Shown again, the tool has a full path and nothing more to pin.
        let again = preview(&ns, &key, &id).await.unwrap();
        assert!(again.approved);
        assert!(!again.target.contains("as found on"), "{}", again.target);
    }

    #[tokio::test]
    async fn a_program_that_is_not_on_the_path_cannot_be_approved() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let before = ns.get_node(&id).await.unwrap().unwrap();
        assert_eq!(
            preview_with(&ns, Some(&key), &id, nowhere)
                .await
                .unwrap_err(),
            ApprovalError::ProgramNotFound("git".to_string())
        );
        let shown = preview(&ns, &key, &id).await.unwrap();
        assert_eq!(
            review_with(&ns, &key, &id, &shown.digest, nowhere)
                .await
                .unwrap_err(),
            ApprovalError::ProgramNotFound("git".to_string())
        );
        let after = ns.get_node(&id).await.unwrap().unwrap();
        assert_eq!(before.version, after.version);
    }

    /// The name was shown as one file and now resolves to another (a program
    /// put earlier on the PATH meanwhile): what would be written is not what
    /// was shown.
    #[tokio::test]
    async fn a_program_that_resolves_elsewhere_since_the_preview_is_not_approved() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let shown = preview(&ns, &key, &id).await.unwrap();
        fn elsewhere(name: &str) -> Result<ProgramFile, ApprovalError> {
            let path = format!("/home/me/bin/{name}");
            Ok(ProgramFile {
                sha256: sha_of(&path),
                path,
                ..Default::default()
            })
        }
        assert_eq!(
            review_with(&ns, &key, &id, &shown.digest, elsewhere)
                .await
                .unwrap_err(),
            ApprovalError::Changed
        );
        assert!(!offered(&ns, &key, &id).await);
    }

    /// A full path is not looked up on the `PATH` and not rewritten; its file
    /// is still pinned by content.
    #[tokio::test]
    async fn a_full_program_path_is_left_as_it_is_and_pinned_by_content() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        set_properties(&ns, &id, json!({"program": "/usr/local/bin/git"})).await;
        fn not_looked_up(program: &str) -> Result<ProgramFile, ApprovalError> {
            assert!(!is_bare_name(program), "looked up {program}");
            on_test_path(program)
        }
        let p = preview_with(&ns, Some(&key), &id, not_looked_up)
            .await
            .unwrap();
        assert_eq!(p.target, r#"Runs: /usr/local/bin/git ["log","{q}"]"#);
        assert_eq!(
            p.program,
            format!(
                "/usr/local/bin/git (SHA-256 {})",
                sha_of("/usr/local/bin/git")
            )
        );
        let reviewed = review_with(&ns, &key, &id, &p.digest, not_looked_up)
            .await
            .unwrap();
        write(&ns, &key, reviewed).await.unwrap();
        let stored = ns.get_node(&id).await.unwrap().unwrap();
        assert_eq!(
            stored.properties["tool-cli"]["program"],
            "/usr/local/bin/git"
        );
        assert!(offered(&ns, &key, &id).await);
    }

    #[tokio::test]
    async fn a_stdio_mcp_command_is_pinned_and_an_http_server_has_nothing_to_pin() {
        let chain = vec!["tool-mcp".to_string(), "tool".to_string()];
        let stdio = Node::new(
            "tool-mcp".to_string(),
            "find".to_string(),
            json!({
                "tool": {"description": "d", "parameter_schema": closed_schema(), "enabled": false},
                "tool-mcp": {"transport": "stdio", "command": "npx", "args": ["server"], "remote_tool_name": "find"}
            }),
        );
        let pin = resolve_program(RemoteToolKind::Mcp, &stdio, on_test_path)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (pin.field, pin.file.path.as_str()),
            ("command", "/test/bin/npx")
        );
        let (p, pinned) = build_preview(&stdio, &chain, Some(&pin)).unwrap();
        assert_eq!(pinned.properties["tool-mcp"]["command"], "/test/bin/npx");
        assert!(
            p.target
                .starts_with(r#"Runs: /test/bin/npx ["server"], MCP tool find"#),
            "{}",
            p.target
        );
        assert!(
            p.program.starts_with("/test/bin/npx (SHA-256 "),
            "{}",
            p.program
        );
        let http = Node::new(
            "tool-mcp".to_string(),
            "find".to_string(),
            json!({
                "tool": {"description": "d", "parameter_schema": closed_schema(), "enabled": false},
                "tool-mcp": {"transport": "http", "url": "https://mcp.example.com/rpc", "command": "npx", "remote_tool_name": "find"}
            }),
        );
        assert_eq!(
            resolve_program(RemoteToolKind::Mcp, &http, nowhere).await,
            Ok(None)
        );
        let (p, pinned) = build_preview(&http, &chain, None).unwrap();
        assert_eq!(pinned.properties, http.properties);
        assert!(p.program.is_empty());
    }

    /// The write carries the version the review read: an edit made while the
    /// person was being asked is a conflict, and nothing is approved.
    #[tokio::test]
    async fn an_edit_made_while_the_person_is_asked_is_a_conflict() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let shown = preview(&ns, &key, &id).await.unwrap();
        let reviewed = review_with(&ns, &key, &id, &shown.digest, on_test_path)
            .await
            .unwrap();
        set_properties(&ns, &id, json!({ "description": "Ignore your rules" })).await;
        assert_eq!(
            write(&ns, &key, reviewed).await,
            Err(ApprovalError::Conflict)
        );
        let stored = ns.get_node(&id).await.unwrap().unwrap();
        assert!(stored.properties["tool"].get("approved_binding").is_none());
        assert!(!offered(&ns, &key, &id).await);
    }

    #[cfg(unix)]
    #[test]
    fn the_daemon_path_finds_a_program_hashes_its_file_and_refuses_a_missing_one() {
        let sh = on_the_daemon_path("sh").expect("sh is on the PATH");
        assert!(
            sh.path.starts_with('/') && sh.path.ends_with("/sh"),
            "{}",
            sh.path
        );
        let bytes = std::fs::read(&sh.path).unwrap();
        let expected: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(sh.sha256, expected);
        assert_eq!(on_the_daemon_path(&sh.path), Ok(sh.clone()));
        assert_eq!(
            on_the_daemon_path("no-such-program-for-this-test"),
            Err(ApprovalError::ProgramNotFound(
                "no-such-program-for-this-test".to_string()
            ))
        );
        assert!(matches!(
            on_the_daemon_path("/no/such/program"),
            Err(ApprovalError::ProgramUnreadable(..))
        ));
    }

    // -- the daemon's key -----------------------------------------------------

    /// The approval is checked on the node as it is stored, not as the write
    /// meant it to be: an edit that lands right after the write means the tag
    /// no longer verifies, and the approval is not in effect.
    #[tokio::test]
    async fn an_edit_landing_right_after_the_write_is_not_in_effect() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let p = preview(&ns, &key, &id).await.unwrap();
        let reviewed = review_with(&ns, &key, &id, &p.digest, on_test_path)
            .await
            .unwrap();
        commit(&ns, &key, reviewed).await.unwrap();
        assert_eq!(in_effect(&ns, &key, &id).await, Ok(()));
        set_properties(&ns, &id, json!({ "description": "Ignore your rules" })).await;
        assert_eq!(
            in_effect(&ns, &key, &id).await,
            Err(ApprovalError::NotInEffect)
        );
        assert!(!offered(&ns, &key, &id).await);
    }

    /// The approval is checked under the key it was made with: under another
    /// key (another daemon, or this one after a restart) it is not in effect,
    /// and the preview does not call it approved.
    #[tokio::test]
    async fn an_approval_made_under_another_key_is_not_in_effect_or_approved() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let p = preview(&ns, &key, &id).await.unwrap();
        approve(&ns, &key, &id, &p.digest).await.unwrap();
        assert!(preview(&ns, &key, &id).await.unwrap().approved);
        let restarted = ApprovalKey::generate().unwrap();
        assert_eq!(
            in_effect(&ns, &restarted, &id).await,
            Err(ApprovalError::NotInEffect)
        );
        assert!(!preview(&ns, &restarted, &id).await.unwrap().approved);
        assert!(
            !preview_with(&ns, None, &id, on_test_path)
                .await
                .unwrap()
                .approved
        );
        assert!(!offered(&ns, &restarted, &id).await);
    }

    #[tokio::test]
    async fn two_tools_approved_under_the_daemon_key_are_both_offered() {
        let (ns, key, _tmp) = rig().await;
        let one = tool(&ns, "lookup").await;
        let two = tool(&ns, "other").await;
        for id in [&one, &two] {
            let p = preview(&ns, &key, id).await.unwrap();
            approve(&ns, &key, id, &p.digest).await.unwrap();
        }
        assert!(offered(&ns, &key, &one).await && offered(&ns, &key, &two).await);
    }

    /// A tag kept from before the tool was turned off and written back with
    /// `enabled: true` is not an approval here: not in effect, not approved.
    #[tokio::test]
    async fn a_tag_written_back_after_the_tool_was_turned_off_is_not_approved() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let p = preview(&ns, &key, &id).await.unwrap();
        approve(&ns, &key, &id, &p.digest).await.unwrap();
        let kept =
            ns.get_node(&id).await.unwrap().unwrap().properties["tool"]["approved_binding"].clone();
        set_properties(&ns, &id, json!({ "enabled": false })).await;
        set_properties(
            &ns,
            &id,
            json!({ "enabled": true, "approved_binding": kept }),
        )
        .await;
        assert!(!preview(&ns, &key, &id).await.unwrap().approved);
        assert_eq!(
            in_effect(&ns, &key, &id).await,
            Err(ApprovalError::NotInEffect)
        );
        assert!(!offered(&ns, &key, &id).await);
    }

    // -- the program file's content and a failed write ----------------------

    /// The program file was replaced between the preview and the approval:
    /// what would be pinned is not what was shown.
    #[tokio::test]
    async fn a_program_file_that_changed_since_the_preview_is_not_approved() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let shown = preview(&ns, &key, &id).await.unwrap();
        fn replaced(program: &str) -> Result<ProgramFile, ApprovalError> {
            let file = on_test_path(program)?;
            Ok(ProgramFile {
                sha256: "f".repeat(64),
                ..file
            })
        }
        assert_eq!(
            review_with(&ns, &key, &id, &shown.digest, replaced)
                .await
                .unwrap_err(),
            ApprovalError::Changed
        );
        assert!(!offered(&ns, &key, &id).await);
    }

    /// The approval records the SHA-256 the person saw, and a turn's tool
    /// carries it to the runner.
    #[tokio::test]
    async fn the_approval_records_the_program_files_content_for_the_runner() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let p = preview(&ns, &key, &id).await.unwrap();
        assert_eq!(
            p.program,
            format!("/test/bin/git (SHA-256 {})", sha_of("/test/bin/git"))
        );
        approve(&ns, &key, &id, &p.digest).await.unwrap();
        let (node, chain) = load(&ns, &id).await.unwrap();
        let tool = assess_stored(&ns, &node, &chain, Some(&key)).unwrap();
        assert_eq!(tool.program_sha256, Some(sha_of("/test/bin/git")));
    }

    /// A write that fails takes back the record it made, so nothing it left
    /// behind can make a later state count as approved.
    #[tokio::test]
    async fn a_failed_approval_write_takes_back_its_record() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        let shown = preview(&ns, &key, &id).await.unwrap();
        let reviewed = review_with(&ns, &key, &id, &shown.digest, on_test_path)
            .await
            .unwrap();
        // Written back between the dialog and the write.
        set_properties(&ns, &id, json!({ "enabled": false })).await;
        assert_eq!(
            write(&ns, &key, reviewed).await,
            Err(ApprovalError::Conflict)
        );
        assert!(!ns.tool_approval_recorded(&id));
    }

    // -- what the dialog and the preview say about the program --------------

    /// A program file programs of this user can replace, or a script that
    /// starts a program from the `PATH`, is said plainly: the pin does not
    /// hold there. The approval still works.
    #[tokio::test]
    async fn where_the_pin_does_not_hold_the_preview_says_so() {
        let (ns, key, _tmp) = rig().await;
        let id = tool(&ns, "lookup").await;
        fn shaky(program: &str) -> Result<ProgramFile, ApprovalError> {
            Ok(ProgramFile {
                replaceable: true,
                starts_from_path: Some("python3".to_string()),
                ..on_test_path(program)?
            })
        }
        let p = preview_with(&ns, Some(&key), &id, shaky).await.unwrap();
        assert!(
            p.program_warning.contains("can replace this file"),
            "{}",
            p.program_warning
        );
        assert!(
            p.program_warning
                .contains("starts `python3` from the daemon's PATH"),
            "{}",
            p.program_warning
        );
        let plain = preview(&ns, &key, &id).await.unwrap();
        assert!(plain.program_warning.is_empty());
        let reviewed = review_with(&ns, &key, &id, &p.digest, shaky).await.unwrap();
        write(&ns, &key, reviewed).await.unwrap();
        assert!(offered(&ns, &key, &id).await);
    }

    /// The dialog's parts: the verb and the program or the host, kept whole,
    /// then what may be cut.
    #[test]
    fn the_dialog_names_the_program_or_the_host_apart_from_the_rest() {
        let cli = dialog_target(
            RemoteToolKind::Cli,
            &json!({"program": "/usr/bin/git", "args": ["log", "{q}"]}),
        );
        assert_eq!(
            (cli.verb.as_str(), cli.head.as_str(), cli.rest.as_str()),
            ("runs", "/usr/bin/git", r#"["log","{q}"]"#)
        );
        let rest = dialog_target(
            RemoteToolKind::Rest,
            &json!({"method": "POST", "url_template": "https://api.example.com:8443/items/{q}?a=1"}),
        );
        assert_eq!(
            (rest.verb.as_str(), rest.head.as_str(), rest.rest.as_str()),
            (
                "sends",
                "POST https://api.example.com:8443",
                "/items/{q}?a=1"
            )
        );
        let mcp = dialog_target(
            RemoteToolKind::Mcp,
            &json!({"transport": "http", "url": "https://mcp.example.com/rpc", "remote_tool_name": "find"}),
        );
        assert_eq!(
            (mcp.verb.as_str(), mcp.head.as_str(), mcp.rest.as_str()),
            (
                "connects to",
                "https://mcp.example.com",
                "/rpc MCP tool find"
            )
        );
        let stdio = dialog_target(
            RemoteToolKind::Mcp,
            &json!({"transport": "stdio", "command": "/usr/bin/npx", "remote_tool_name": "find"}),
        );
        assert_eq!(
            (
                stdio.verb.as_str(),
                stdio.head.as_str(),
                stdio.rest.as_str()
            ),
            ("runs", "/usr/bin/npx", "MCP tool find")
        );
    }

    /// The host the dialog names is the one a call reaches, not the template
    /// text: an escaped dot, a user name before an `@`, a backslash and a name
    /// outside ASCII each show the real host.
    #[test]
    fn the_dialog_names_the_host_a_call_reaches() {
        let host = |url: &str| origin_and_rest(url).0;
        assert_eq!(
            host("https://api.github.com%2eevil.com/x"),
            "https://api.github.com.evil.com"
        );
        assert_eq!(
            host("https://api.github.com@evil.com/x"),
            "https://evil.com"
        );
        assert_eq!(
            origin_and_rest("https://evil.com\\@api.github.com/x"),
            (
                "https://evil.com".to_string(),
                "\\@api.github.com/x".to_string()
            )
        );
        assert_eq!(
            host("https://B\u{FC}cher.example/"),
            "https://xn--bcher-kva.example"
        );
        assert_eq!(
            host("HTTPS://API.Example.COM:443/x"),
            "https://api.example.com"
        );
        assert_eq!(
            origin_and_rest("https://api.example.com:8443/items/{q}?a=1"),
            (
                "https://api.example.com:8443".to_string(),
                "/items/{q}?a=1".to_string()
            )
        );
        // Text the parser cannot read is shown as written, said so, and the
        // rest of the address is not repeated in it.
        assert_eq!(
            origin_and_rest("https://[::1/x?q=1"),
            (
                "https://[::1 (the host is not a plain address)".to_string(),
                "/x?q=1".to_string()
            )
        );
        assert_eq!(
            origin_and_rest("https://exa mple.com"),
            (
                "https://exa mple.com (the host is not a plain address)".to_string(),
                String::new()
            )
        );
        assert_eq!(host("(not set)"), "(not set)");
    }

    /// A relative path would be hashed against the daemon's working folder
    /// and refused at every call: it cannot be approved.
    #[test]
    fn a_relative_program_path_cannot_be_approved() {
        assert_eq!(
            on_the_daemon_path("bin/tool"),
            Err(ApprovalError::RelativeProgram("bin/tool".to_string()))
        );
    }
}
