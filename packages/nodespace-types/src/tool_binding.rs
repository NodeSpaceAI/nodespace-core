//! The binding of a remote tool: how to reach a tool that lives outside
//! NodeSpace (`tool-mcp`, `tool-rest`, `tool-cli`), read from the node's
//! stored properties.
//!
//! One reader per kind decodes the kind's own bucket into a typed binding and
//! checks the rules that hold for any stored value (a field present must be
//! well formed). [`ToolMcpFields::check_complete`] and its siblings add what
//! an enabled tool also needs: every required field, and every `{name}`
//! placeholder naming a parameter. The behaviours in `nodespace-core` call
//! these, and so does whatever later runs the call, so the rules are written
//! once.
//!
//! A credential is not stored here: a `secret` field holds a
//! [`CredentialRef`], a name resolved at call time on this device. These
//! types are readers decoded from a node's stored properties; they are not a
//! typed wire shape (the tool family's wire shape is generic), and are generated
//! to TypeScript only because the generator covers every serde type.

use serde::Deserialize;
use serde_json::Value;
use url::Url;

/// Largest timeout a remote call may carry, in seconds.
pub const MAX_TIMEOUT_SECONDS: u64 = 300;
/// Timeout applied when the node sets none, in seconds.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
/// Largest cap on a response or output a node may set, in bytes (10 MiB).
pub const MAX_RESPONSE_BYTES: u64 = 10 * 1024 * 1024;
/// Cap applied when the node sets none, in bytes (1 MiB).
pub const DEFAULT_RESPONSE_BYTES: u64 = 1024 * 1024;

/// Longest name a remote tool may have, in characters.
const MAX_TOOL_NAME_LEN: usize = 64;

/// A binding rule a remote tool breaks. The message names the rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ToolBindingError(pub String);

fn err<T>(message: impl Into<String>) -> Result<T, ToolBindingError> {
    Err(ToolBindingError(message.into()))
}

// ---------------------------------------------------------------------------
// Credential references
// ---------------------------------------------------------------------------

/// Where a credential's value is kept. The node holds this reference and
/// checked as a name only: a string shaped like a token passes as a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialRef {
    /// `keychain:<name>`: an entry in the device's credential store.
    Keychain(String),
    /// `env:<NAME>`: an environment variable of the process that runs the call.
    Env(String),
}

impl CredentialRef {
    /// Parses a well-formed reference. Anything else, including a string that
    /// looks like a token, is refused: the reference is never resolved here.
    ///
    /// ```
    /// use nodespace_types::CredentialRef;
    /// assert!(CredentialRef::parse("keychain:github-token").is_ok());
    /// assert!(CredentialRef::parse("env:GITHUB_TOKEN").is_ok());
    /// assert!(CredentialRef::parse("ghp_abcdef123456").is_err());
    /// ```
    pub fn parse(text: &str) -> Result<Self, ToolBindingError> {
        if let Some(name) = text.strip_prefix("keychain:") {
            let ok = !name.is_empty()
                && name.len() <= 64
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
            if ok {
                return Ok(Self::Keychain(name.to_string()));
            }
        } else if let Some(name) = text.strip_prefix("env:") {
            if is_env_name(name) {
                return Ok(Self::Env(name.to_string()));
            }
        }
        err("a secret must be a credential reference, `keychain:<name>` or `env:<NAME>`; it is checked as a name only")
    }
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 128
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Shared rules
// ---------------------------------------------------------------------------

/// Whether `name` is a tool name the model API accepts: ASCII letters, digits,
/// `_` and `-`, starting with a letter, at most 64 characters.
pub fn is_tool_name(name: &str) -> bool {
    name.len() <= MAX_TOOL_NAME_LEN
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A remote tool's `parameter_schema` must be an object whose root is closed:
/// `type: "object"` and `additionalProperties: false`, so no argument the
/// schema does not declare is forwarded to an outside system.
pub fn check_closed_root(schema: &Value) -> Result<(), ToolBindingError> {
    let Some(root) = schema.as_object() else {
        return err("parameter_schema must be a JSON object");
    };
    if root.get("type").and_then(Value::as_str) != Some("object") {
        return err("a remote tool's parameter_schema must have type \"object\" at its root");
    }
    if root.get("additionalProperties") != Some(&Value::Bool(false)) {
        return err(
            "a remote tool's parameter_schema must set additionalProperties: false at its root",
        );
    }
    Ok(())
}

/// The names `parameter_schema` declares under `properties`.
fn parameter_names(schema: Option<&Value>) -> Vec<&str> {
    schema
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object)
        .map(|props| props.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

fn check_names_declared(
    what: &str,
    names: &[&str],
    schema: Option<&Value>,
) -> Result<(), ToolBindingError> {
    let declared = parameter_names(schema);
    for name in names {
        if !declared.contains(name) {
            return err(format!(
                "{what} `{name}` does not name a property of parameter_schema"
            ));
        }
    }
    Ok(())
}

/// A program or command: a bare name, or an absolute path with no `..`
/// segment; never a shell line, a relative path or an option.
///
/// A bare name is looked up on the executor's path. An interpreter such as
/// `sh` or `env` is still accepted: what guards it is the fingerprint and the
/// confirmation the executor asks for, and the executor must put `--` before
/// any model-supplied value.
fn check_program(field: &str, value: &str) -> Result<(), ToolBindingError> {
    let bad = |c: char| {
        c.is_whitespace()
            || c.is_control()
            || matches!(
                c,
                ';' | '&'
                    | '|'
                    | '<'
                    | '>'
                    | '$'
                    | '`'
                    | '"'
                    | '\''
                    | '*'
                    | '?'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '!'
                    | '#'
            )
    };
    let refuse = || {
        err(format!(
            "{field} must be a bare executable name or an absolute path with no `..` segment, \
             with no spaces, arguments or shell syntax"
        ))
    };
    if value.is_empty()
        || value.len() > 4096
        || value.chars().any(bad)
        || value.starts_with(['-', '~'])
    {
        return refuse();
    }
    let separators = |c: char| c == '/' || c == '\\';
    if !value.contains(separators) {
        return Ok(());
    }
    let bytes = value.as_bytes();
    let absolute = value.starts_with('/')
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'));
    if absolute && !value.split(separators).any(|segment| segment == "..") {
        Ok(())
    } else {
        refuse()
    }
}

fn check_literal_arg(field: &str, value: &str) -> Result<(), ToolBindingError> {
    if value.contains('\0') || value.len() > 4096 {
        return err(format!(
            "{field} items must be text without NUL and at most 4096 bytes"
        ));
    }
    Ok(())
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// An absolute `https` URL, or `http` for a loopback host; no embedded
/// credentials.
fn check_url(field: &str, url: &Url) -> Result<(), ToolBindingError> {
    if url.host().is_none() {
        return err(format!("{field} must be an absolute URL with a host"));
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(url) => {}
        _ => {
            return err(format!(
                "{field} must use https (http is allowed only for a loopback host)"
            ))
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return err(format!(
            "{field} must not carry credentials; use a secret reference"
        ));
    }
    Ok(())
}

fn check_timeout(value: Option<u64>) -> Result<(), ToolBindingError> {
    match value {
        Some(0) => err("timeout_seconds must be at least 1"),
        Some(n) if n > MAX_TIMEOUT_SECONDS => err(format!(
            "timeout_seconds must be at most {MAX_TIMEOUT_SECONDS}"
        )),
        _ => Ok(()),
    }
}

fn check_cap(field: &str, value: Option<u64>) -> Result<(), ToolBindingError> {
    match value {
        Some(0) => err(format!("{field} must be at least 1")),
        Some(n) if n > MAX_RESPONSE_BYTES => {
            err(format!("{field} must be at most {MAX_RESPONSE_BYTES}"))
        }
        _ => Ok(()),
    }
}

/// Header names a binding may never set, as a static header or as the header
/// that carries a credential: they frame, route or authenticate the request
/// itself.
fn is_reserved_header(lower: &str) -> bool {
    matches!(
        lower,
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "upgrade"
            | "te"
            | "trailer"
            | "cookie"
            | "set-cookie"
            | "proxy-authorization"
    ) || lower.starts_with("proxy-")
}

/// Whether a header name says it carries a credential. Static headers are
/// non-secret by name, so these go in `auth`.
fn is_credential_header(lower: &str) -> bool {
    lower == "authorization"
        || [
            "token", "secret", "password", "passwd", "api-key", "apikey", "auth",
        ]
        .iter()
        .any(|word| lower.contains(word))
}

fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// One `{name, secret}` entry of an `env` list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct EnvSecret {
    /// The environment variable the child process gets.
    pub name: String,
    /// The credential reference its value comes from.
    pub secret: String,
}

/// Variables that change which program runs or what it loads.
fn is_reserved_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("LD_")
        || upper.starts_with("DYLD_")
        || matches!(
            upper.as_str(),
            "PATH"
                | "PATHEXT"
                | "IFS"
                | "ENV"
                | "BASH_ENV"
                | "SHELL"
                | "COMSPEC"
                | "PYTHONPATH"
                | "PYTHONSTARTUP"
                | "PYTHONHOME"
                | "NODE_OPTIONS"
                | "NODE_PATH"
                | "PERL5OPT"
                | "PERL5LIB"
                | "RUBYOPT"
                | "RUBYLIB"
                | "CLASSPATH"
                | "JAVA_TOOL_OPTIONS"
                | "_JAVA_OPTIONS"
                | "GIT_EXEC_PATH"
                | "GIT_SSH_COMMAND"
        )
}

fn check_env(env: &[EnvSecret]) -> Result<(), ToolBindingError> {
    let mut seen = Vec::new();
    for entry in env {
        if !is_env_name(&entry.name) {
            return err(format!(
                "env name `{}` is not a valid variable name",
                entry.name
            ));
        }
        if seen.contains(&entry.name.as_str()) {
            return err(format!("env name `{}` is listed twice", entry.name));
        }
        if is_reserved_env(&entry.name) {
            return err(format!(
                "env name `{}` changes how a program is found or loaded and is not allowed",
                entry.name
            ));
        }
        seen.push(entry.name.as_str());
        CredentialRef::parse(&entry.secret)?;
    }
    Ok(())
}

/// The `auth` object of an http binding.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct HttpAuth {
    /// `none`, `bearer`, `basic` (REST only) or `header`.
    pub scheme: String,
    /// The header that carries the credential, for `header`.
    #[serde(default)]
    pub header_name: Option<String>,
    /// The credential reference, for every scheme but `none`.
    #[serde(default)]
    pub secret: Option<String>,
}

fn check_auth(auth: &HttpAuth, allowed: &[&str]) -> Result<(), ToolBindingError> {
    if !allowed.contains(&auth.scheme.as_str()) {
        return err(format!("auth scheme must be one of {}", allowed.join(", ")));
    }
    let scheme = auth.scheme.as_str();
    match (&auth.secret, scheme) {
        (Some(_), "none") => return err("auth scheme none takes no secret"),
        (None, "none") => {}
        (None, _) => return err(format!("auth scheme {scheme} needs a secret reference")),
        (Some(secret), _) => {
            CredentialRef::parse(secret)?;
        }
    }
    match (&auth.header_name, scheme) {
        (Some(name), "header")
            if is_header_name(name) && !is_reserved_header(&name.to_ascii_lowercase()) => {}
        (_, "header") => {
            return err(
                "auth scheme header needs a valid header_name that is not a reserved header",
            )
        }
        (Some(_), _) => return err("header_name is only for auth scheme header"),
        (None, _) => {}
    }
    Ok(())
}

fn decode<T: for<'de> Deserialize<'de>>(kind: &str, bucket: &Value) -> Result<T, ToolBindingError> {
    // A node that has set nothing yet has no bucket.
    let bucket = if bucket.is_null() {
        &Value::Object(Default::default())
    } else {
        bucket
    };
    serde_json::from_value(bucket.clone())
        .map_err(|e| ToolBindingError(format!("{kind} binding is malformed: {e}")))
}

fn require<'a, T>(field: &str, value: &'a Option<T>) -> Result<&'a T, ToolBindingError> {
    match value {
        Some(v) => Ok(v),
        None => err(format!("{field} is required before the tool is enabled")),
    }
}

// ---------------------------------------------------------------------------
// tool-mcp
// ---------------------------------------------------------------------------

/// The `tool-mcp` bucket: a tool on an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct ToolMcpFields {
    /// `stdio` or `http`.
    #[serde(default)]
    pub transport: Option<String>,
    /// stdio: the executable.
    #[serde(default)]
    pub command: Option<String>,
    /// stdio: one argv element per item.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// http: the server URL.
    #[serde(default)]
    pub url: Option<String>,
    /// The tool's name on the server.
    #[serde(default)]
    pub remote_tool_name: Option<String>,
    /// stdio: environment entries whose values are credential references.
    #[serde(default)]
    pub env: Option<Vec<EnvSecret>>,
    /// http: how the call authenticates.
    #[serde(default)]
    pub auth: Option<HttpAuth>,
    /// Seconds before the call is abandoned.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

impl ToolMcpFields {
    /// Decodes the `tool-mcp` bucket and checks every field that is set.
    pub fn from_bucket(bucket: &Value) -> Result<Self, ToolBindingError> {
        let fields: Self = decode("tool-mcp", bucket)?;
        fields.check()?;
        Ok(fields)
    }

    fn check(&self) -> Result<(), ToolBindingError> {
        let transport = match self.transport.as_deref() {
            None => None,
            Some(t @ ("stdio" | "http")) => Some(t),
            Some(_) => return err("transport must be stdio or http"),
        };
        if let Some(command) = &self.command {
            check_program("command", command)?;
        }
        for arg in self.args.iter().flatten() {
            check_literal_arg("args", arg)?;
        }
        if let Some(url) = &self.url {
            match Url::parse(url) {
                Ok(parsed) => check_url("url", &parsed)?,
                Err(_) => return err("url must be an absolute URL"),
            }
        }
        if let Some(name) = &self.remote_tool_name {
            if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
                return err("remote_tool_name must be a non-empty name");
            }
        }
        check_env(self.env.as_deref().unwrap_or_default())?;
        if let Some(auth) = &self.auth {
            check_auth(auth, &["none", "bearer", "header"])?;
        }
        check_timeout(self.timeout_seconds)?;
        // Each transport owns its fields.
        let stdio_only = [
            ("command", self.command.is_some()),
            ("args", self.args.is_some()),
            ("env", self.env.as_ref().is_some_and(|e| !e.is_empty())),
        ];
        let http_only = [("url", self.url.is_some()), ("auth", self.auth.is_some())];
        let (other, owner) = match transport {
            Some("stdio") => (&http_only[..], "http"),
            Some("http") => (&stdio_only[..], "stdio"),
            _ => return Ok(()),
        };
        if let Some((field, _)) = other.iter().find(|(_, set)| *set) {
            return err(format!("{field} is only for the {owner} transport"));
        }
        Ok(())
    }

    /// What an enabled tool needs on top of [`Self::from_bucket`].
    pub fn check_complete(
        &self,
        _parameter_schema: Option<&Value>,
    ) -> Result<(), ToolBindingError> {
        match require("transport", &self.transport)?.as_str() {
            "stdio" => {
                require("command", &self.command)?;
            }
            _ => {
                require("url", &self.url)?;
            }
        }
        require("remote_tool_name", &self.remote_tool_name)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// tool-rest
// ---------------------------------------------------------------------------

/// A static header of a `tool-rest` binding.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct StaticHeader {
    /// Header name.
    pub name: String,
    /// A fixed, non-secret value.
    pub value: String,
}

/// The `tool-rest` bucket: an HTTP endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct ToolRestFields {
    /// `GET`, `POST`, `PUT`, `PATCH` or `DELETE`.
    #[serde(default)]
    pub method: Option<String>,
    /// Absolute URL; `{name}` placeholders only after the host and port.
    #[serde(default)]
    pub url_template: Option<String>,
    /// Arguments sent as query parameters.
    #[serde(default)]
    pub query_params: Option<Vec<String>>,
    /// `none` or `json`.
    #[serde(default)]
    pub body: Option<String>,
    /// Static, non-secret headers.
    #[serde(default)]
    pub headers: Option<Vec<StaticHeader>>,
    /// How the call authenticates.
    #[serde(default)]
    pub auth: Option<HttpAuth>,
    /// Seconds before the call is abandoned.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Cap on the response text, in bytes.
    #[serde(default)]
    pub max_response_bytes: Option<u64>,
}

/// A URL template: a literal `http://` or `https://` prefix, then the same
/// scheme, host and port however each `{name}` placeholder is filled.
///
/// The template is parsed twice, with every placeholder replaced by `x` and
/// then by `y`. A placeholder that reaches the scheme, the host or the port
/// (`https:{h}/p`, `https:///{h}`, `https://h:{p}/`) changes one of them
/// between the two parses and is refused. A backslash is refused outright:
/// the URL parser treats it as a slash. The executor percent-encodes each
/// value it puts in a placeholder.
fn check_url_template(template: &str) -> Result<(), ToolBindingError> {
    placeholders(template)?;
    let lower = template.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return err("url_template must start with a literal http:// or https://");
    }
    if template.contains('\\') {
        return err("url_template must not contain a backslash");
    }
    let fill = |with: char| {
        let mut out = String::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            out.push(with);
            rest = &rest[rest.find('}').map_or(rest.len(), |c| c + 1)..];
        }
        out.push_str(rest);
        out
    };
    let (Ok(x), Ok(y)) = (Url::parse(&fill('x')), Url::parse(&fill('y'))) else {
        return err("url_template must be an absolute URL");
    };
    let origin = |u: &Url| {
        (
            u.scheme().to_string(),
            u.host_str().map(str::to_string),
            u.port(),
        )
    };
    if origin(&x) != origin(&y) {
        return err("url_template must not template the scheme, host or port");
    }
    check_url("url_template", &x)
}

/// The `{name}` placeholders of `template`, or why it is malformed.
fn placeholders(template: &str) -> Result<Vec<&str>, ToolBindingError> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find(['{', '}']) {
        if rest.as_bytes()[open] == b'}' {
            return err("url_template has an unmatched `}`");
        }
        let Some(len) = rest[open + 1..].find('}') else {
            return err("url_template has an unmatched `{`");
        };
        let name = &rest[open + 1..open + 1 + len];
        if !is_env_name(name) {
            return err(format!(
                "url_template placeholder `{{{name}}}` is not a name"
            ));
        }
        names.push(name);
        rest = &rest[open + 2 + len..];
    }
    Ok(names)
}

impl ToolRestFields {
    /// Decodes the `tool-rest` bucket and checks every field that is set.
    pub fn from_bucket(bucket: &Value) -> Result<Self, ToolBindingError> {
        let fields: Self = decode("tool-rest", bucket)?;
        fields.check()?;
        Ok(fields)
    }

    /// The placeholder names of the URL template.
    pub fn placeholder_names(&self) -> Vec<&str> {
        self.url_template
            .as_deref()
            .and_then(|t| placeholders(t).ok())
            .unwrap_or_default()
    }

    fn check(&self) -> Result<(), ToolBindingError> {
        if let Some(method) = &self.method {
            if !["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&method.as_str()) {
                return err("method must be GET, POST, PUT, PATCH or DELETE");
            }
        }
        if let Some(template) = &self.url_template {
            check_url_template(template)?;
        }
        for name in self.query_params.iter().flatten() {
            if !is_env_name(name) {
                return err(format!(
                    "query_params entry `{name}` is not a parameter name"
                ));
            }
        }
        match self.body.as_deref() {
            None | Some("none") => {}
            Some("json") => {
                if self.method.as_deref() == Some("GET") {
                    return err("a GET request cannot have a json body");
                }
            }
            Some(_) => return err("body must be none or json"),
        }
        for header in self.headers.iter().flatten() {
            let lower = header.name.to_ascii_lowercase();
            if !is_header_name(&header.name) {
                return err(format!("header name `{}` is not valid", header.name));
            }
            if is_reserved_header(&lower) || is_credential_header(&lower) {
                return err(format!(
                    "header `{}` is not a static header; credentials go in auth",
                    header.name
                ));
            }
            if header.value.chars().any(char::is_control) {
                return err("header values must not contain control characters");
            }
        }
        if let Some(auth) = &self.auth {
            check_auth(auth, &["none", "bearer", "basic", "header"])?;
        }
        check_timeout(self.timeout_seconds)?;
        check_cap("max_response_bytes", self.max_response_bytes)
    }

    /// What an enabled tool needs on top of [`Self::from_bucket`]: the
    /// required fields, and every placeholder and query name declared by
    /// `parameter_schema`.
    pub fn check_complete(&self, parameter_schema: Option<&Value>) -> Result<(), ToolBindingError> {
        require("method", &self.method)?;
        let template = require("url_template", &self.url_template)?;
        let in_path = placeholders(template)?;
        check_names_declared("url_template placeholder", &in_path, parameter_schema)?;
        let queries: Vec<&str> = self
            .query_params
            .iter()
            .flatten()
            .map(String::as_str)
            .collect();
        check_names_declared("query_params entry", &queries, parameter_schema)?;
        if let Some(both) = queries.iter().find(|q| in_path.contains(q)) {
            return err(format!(
                "`{both}` is both a url_template placeholder and a query parameter"
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// tool-cli
// ---------------------------------------------------------------------------

/// The `tool-cli` bucket: a command-line program.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct ToolCliFields {
    /// An executable name or absolute path.
    #[serde(default)]
    pub program: Option<String>,
    /// One argv element per item: a literal, or exactly one `{name}`.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// The only environment the child gets, besides a fixed base list.
    #[serde(default)]
    pub env: Option<Vec<EnvSecret>>,
    /// Seconds before the call is killed.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Cap on the output, in bytes.
    #[serde(default)]
    pub max_output_bytes: Option<u64>,
}

/// The parameter a whole-element placeholder `{name}` names.
fn whole_placeholder(arg: &str) -> Option<&str> {
    arg.strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .filter(|name| is_env_name(name))
}

impl ToolCliFields {
    /// Decodes the `tool-cli` bucket and checks every field that is set.
    pub fn from_bucket(bucket: &Value) -> Result<Self, ToolBindingError> {
        let fields: Self = decode("tool-cli", bucket)?;
        fields.check()?;
        Ok(fields)
    }

    /// The parameter names the args reference, in order.
    pub fn placeholder_names(&self) -> Vec<&str> {
        self.args
            .iter()
            .flatten()
            .filter_map(|a| whole_placeholder(a))
            .collect()
    }

    fn check(&self) -> Result<(), ToolBindingError> {
        if let Some(program) = &self.program {
            check_program("program", program)?;
        }
        for arg in self.args.iter().flatten() {
            check_literal_arg("args", arg)?;
            // An argument can never split into two or be built from parts.
            if whole_placeholder(arg).is_none() && arg.contains(['{', '}']) {
                return err(format!(
                    "args item `{arg}` must be a literal or exactly one {{name}} placeholder"
                ));
            }
        }
        check_env(self.env.as_deref().unwrap_or_default())?;
        check_timeout(self.timeout_seconds)?;
        check_cap("max_output_bytes", self.max_output_bytes)
    }

    /// What an enabled tool needs on top of [`Self::from_bucket`].
    pub fn check_complete(&self, parameter_schema: Option<&Value>) -> Result<(), ToolBindingError> {
        require("program", &self.program)?;
        check_names_declared(
            "args placeholder",
            &self.placeholder_names(),
            parameter_schema,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg<T>(r: Result<T, ToolBindingError>) -> String {
        r.err().expect("must be refused").0
    }

    #[test]
    fn a_credential_reference_is_a_name_never_a_value() {
        assert_eq!(
            CredentialRef::parse("keychain:gh.token-1"),
            Ok(CredentialRef::Keychain("gh.token-1".into()))
        );
        assert_eq!(
            CredentialRef::parse("env:GH_TOKEN"),
            Ok(CredentialRef::Env("GH_TOKEN".into()))
        );
        for value in [
            "ghp_0123456789abcdef",
            "sk-ant-api03-xyz",
            "Bearer abc",
            "keychain:",
            "keychain:has space",
            "env:1BAD",
            "env:lower-case",
            "file:/etc/passwd",
            "",
        ] {
            assert!(CredentialRef::parse(value).is_err(), "{value}");
        }
    }

    #[test]
    fn tool_names_follow_the_identifier_rule() {
        assert!(is_tool_name("list_files-2"));
        assert!(!is_tool_name("2fast"));
        assert!(!is_tool_name("has space"));
        assert!(!is_tool_name(""));
        assert!(!is_tool_name(&"a".repeat(65)));
        assert!(is_tool_name(&"a".repeat(64)));
    }

    #[test]
    fn the_root_of_a_remote_schema_is_closed() {
        assert!(check_closed_root(&json!({"type":"object","additionalProperties":false})).is_ok());
        assert!(check_closed_root(&json!({"type":"object"})).is_err());
        assert!(check_closed_root(&json!({"additionalProperties":false})).is_err());
        assert!(check_closed_root(
            &json!({"type":"object","additionalProperties":{"type":"string"}})
        )
        .is_err());
    }

    #[test]
    fn mcp_transport_owns_its_fields() {
        let ok = ToolMcpFields::from_bucket(&json!({
            "transport": "stdio", "command": "/usr/bin/server", "args": ["--flag", "x y"],
            "remote_tool_name": "search", "env": [{"name":"TOKEN","secret":"env:TOKEN"}],
        }))
        .unwrap();
        assert!(ok.check_complete(None).is_ok());
        assert!(msg(ToolMcpFields::from_bucket(
            &json!({"transport":"stdio","url":"https://x.test"})
        ))
        .contains("http transport"));
        assert!(msg(ToolMcpFields::from_bucket(
            &json!({"transport":"http","command":"server"})
        ))
        .contains("stdio transport"));
        assert!(msg(ToolMcpFields::from_bucket(&json!({"transport":"ws"}))).contains("transport"));
        assert!(
            msg(ToolMcpFields::from_bucket(&json!({"command":"rm -rf /"}))).contains("command")
        );
        // Incomplete drafts parse; they are not enabled.
        let draft = ToolMcpFields::from_bucket(&json!({"transport":"http"})).unwrap();
        assert!(msg(draft.check_complete(None)).contains("url"));
    }

    #[test]
    fn urls_are_https_or_loopback_and_carry_no_credentials() {
        for url in [
            "https://api.example.com/mcp",
            "http://127.0.0.1:8080/mcp",
            "http://localhost/x",
        ] {
            assert!(
                ToolMcpFields::from_bucket(&json!({"transport":"http","url":url})).is_ok(),
                "{url}"
            );
        }
        for url in [
            "http://example.com/mcp",
            "ftp://x.test",
            "/relative",
            "https://u:p@x.test/",
        ] {
            assert!(
                ToolMcpFields::from_bucket(&json!({"transport":"http","url":url})).is_err(),
                "{url}"
            );
        }
    }

    #[test]
    fn auth_needs_a_reference_unless_none() {
        let with =
            |auth: Value| ToolMcpFields::from_bucket(&json!({"transport":"http","auth":auth}));
        assert!(with(json!({"scheme":"bearer","secret":"keychain:t"})).is_ok());
        assert!(with(json!({"scheme":"none"})).is_ok());
        assert!(with(json!({"scheme":"header","header_name":"X-Key","secret":"env:K"})).is_ok());
        assert!(with(json!({"scheme":"bearer","secret":"ghp_literal"})).is_err());
        assert!(with(json!({"scheme":"bearer"})).is_err());
        assert!(with(json!({"scheme":"header","secret":"env:K"})).is_err());
        assert!(with(json!({"scheme":"none","secret":"env:K"})).is_err());
        assert!(with(json!({"scheme":"basic","secret":"env:K"})).is_err());
    }

    #[test]
    fn limits_are_bounded() {
        assert!(ToolMcpFields::from_bucket(&json!({"timeout_seconds":300})).is_ok());
        assert!(msg(ToolMcpFields::from_bucket(&json!({"timeout_seconds":301}))).contains("300"));
        assert!(ToolMcpFields::from_bucket(&json!({"timeout_seconds":0})).is_err());
        assert!(
            ToolRestFields::from_bucket(&json!({"max_response_bytes":MAX_RESPONSE_BYTES})).is_ok()
        );
        assert!(
            ToolRestFields::from_bucket(&json!({"max_response_bytes":MAX_RESPONSE_BYTES + 1}))
                .is_err()
        );
        assert!(
            ToolCliFields::from_bucket(&json!({"max_output_bytes":MAX_RESPONSE_BYTES + 1}))
                .is_err()
        );
    }

    fn schema(props: &[&str]) -> Value {
        let properties: serde_json::Map<String, Value> = props
            .iter()
            .map(|p| (p.to_string(), json!({"type":"string"})))
            .collect();
        json!({"type":"object","additionalProperties":false,"properties":properties})
    }

    #[test]
    fn rest_url_templates_never_template_the_origin() {
        let rest = |t: &str| ToolRestFields::from_bucket(&json!({"method":"GET","url_template":t}));
        assert!(rest("https://api.test/items/{id}").is_ok());
        assert!(rest("https://api.test:8443/items/{id}?x={y}").is_ok());
        for bad in [
            "https://{host}/items",
            "{scheme}://api.test/",
            "https://api.test:{port}/",
            "https://api.test/{id",
            "https://api.test/id}",
            "https://api.test/{}",
            "http://api.test/items",
        ] {
            assert!(rest(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_placeholder_can_never_reach_the_scheme_host_or_port() {
        let rest = |t: &str| ToolRestFields::from_bucket(&json!({"method":"GET","url_template":t}));
        for bad in [
            "https:{h}/p",
            "https:/{h}/p",
            "https:\\{h}/p",
            "https:///{a}",
            "https://{host}/x",
            "https://h:{p}/",
            "https://user@{host}/x",
            "ftp://a.test/{x}",
            "//a.test/{x}",
            "/{x}",
            "https://a.test\\{x}",
            "{x}",
        ] {
            assert!(rest(bad).is_err(), "{bad}");
        }
        for ok in [
            "HTTPS://a.test/{id}",
            "https://a.test/{id}/sub?q={q}",
            "https://a.test:8443/{id}#{frag}",
            "http://localhost:3000/{id}",
        ] {
            assert!(rest(ok).is_ok(), "{ok}: {:?}", rest(ok).err());
        }
    }

    #[test]
    fn a_program_is_a_bare_name_or_an_absolute_path_without_dotdot() {
        let cli = |p: &str| ToolCliFields::from_bucket(&json!({"program":p}));
        for ok in [
            "git",
            "sh",
            "env",
            "/usr/bin/git",
            "C:\\tools\\x.exe",
            "C:/tools/x.exe",
        ] {
            assert!(cli(ok).is_ok(), "{ok}");
        }
        for bad in [
            "../../bin/sh",
            "bin/x",
            "./x",
            "~/x",
            "~",
            "-rf",
            "--help",
            "a/../../b",
            "/usr/../bin/sh",
            "C:\\a\\..\\b",
            "\\server\\share\\x",
        ] {
            assert!(cli(bad).is_err(), "{bad}");
        }
        assert!(ToolMcpFields::from_bucket(&json!({"command":"../sh"})).is_err());
    }

    #[test]
    fn header_names_share_one_reserved_list() {
        let header =
            |n: &str| ToolRestFields::from_bucket(&json!({"headers":[{"name":n,"value":"v"}]}));
        let auth = |n: &str| {
            ToolRestFields::from_bucket(&json!({
                "auth":{"scheme":"header","header_name":n,"secret":"env:K"}
            }))
        };
        for reserved in [
            "Host",
            "Cookie",
            "Set-Cookie",
            "Content-Length",
            "Transfer-Encoding",
            "Proxy-Foo",
            "Connection",
        ] {
            assert!(header(reserved).is_err(), "static {reserved}");
            assert!(auth(reserved).is_err(), "auth {reserved}");
        }
        // Credential-looking names are not static headers, but may carry the credential.
        for secretish in [
            "Authorization",
            "X-Api-Key",
            "X-Auth-Token",
            "x-access-token",
            "Api-Key",
        ] {
            assert!(header(secretish).is_err(), "static {secretish}");
        }
        for name in ["X-Api-Key", "Authorization", "X-Auth-Token"] {
            assert!(auth(name).is_ok(), "auth {name}");
        }
        assert!(header("Accept").is_ok());
    }

    #[test]
    fn env_names_that_change_what_runs_are_refused() {
        let env =
            |n: &str| ToolCliFields::from_bucket(&json!({"env":[{"name":n,"secret":"env:K"}]}));
        for bad in [
            "PATH",
            "path",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "NODE_OPTIONS",
            "BASH_ENV",
            "IFS",
            "PYTHONPATH",
        ] {
            assert!(env(bad).is_err(), "{bad}");
        }
        for ok in ["API_TOKEN", "GITHUB_TOKEN", "HOME_URL"] {
            assert!(env(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn rest_placeholders_and_queries_must_name_parameters() {
        let fields = ToolRestFields::from_bucket(&json!({
            "method":"GET","url_template":"https://api.test/items/{id}","query_params":["q"]
        }))
        .unwrap();
        assert!(fields.check_complete(Some(&schema(&["id", "q"]))).is_ok());
        assert!(msg(fields.check_complete(Some(&schema(&["q"])))).contains("`id`"));
        assert!(msg(fields.check_complete(Some(&schema(&["id"])))).contains("`q`"));
        assert!(msg(fields.check_complete(None)).contains("`id`"));
        let overlap = ToolRestFields::from_bucket(&json!({
            "method":"GET","url_template":"https://api.test/{id}","query_params":["id"]
        }))
        .unwrap();
        assert!(msg(overlap.check_complete(Some(&schema(&["id"])))).contains("both"));
    }

    #[test]
    fn rest_headers_are_static_and_non_secret() {
        let with = |h: Value| ToolRestFields::from_bucket(&json!({"headers": h}));
        assert!(with(json!([{"name":"Accept","value":"application/json"}])).is_ok());
        for name in ["Authorization", "cookie", "Host", "Content-Length"] {
            assert!(with(json!([{"name":name,"value":"x"}])).is_err(), "{name}");
        }
        assert!(
            ToolRestFields::from_bucket(&json!({"method":"GET","body":"json"})).is_err(),
            "a GET has no body"
        );
        assert!(ToolRestFields::from_bucket(&json!({"method":"TRACE"})).is_err());
    }

    #[test]
    fn cli_args_are_whole_element_placeholders_never_a_shell_line() {
        let cli = |b: Value| ToolCliFields::from_bucket(&b);
        let ok = cli(json!({"program":"git","args":["log","-n","{count}"]})).unwrap();
        assert_eq!(ok.placeholder_names(), vec!["count"]);
        assert!(ok.check_complete(Some(&schema(&["count"]))).is_ok());
        assert!(msg(ok.check_complete(Some(&schema(&[])))).contains("`count`"));
        for program in [
            "git status",
            "git; rm -rf /",
            "$(whoami)",
            "a|b",
            "",
            "x\ny",
        ] {
            assert!(cli(json!({"program":program})).is_err(), "{program}");
        }
        for arg in ["--n={count}", "{a}{b}", "x{", "{not valid}"] {
            assert!(cli(json!({"program":"git","args":[arg]})).is_err(), "{arg}");
        }
        assert!(msg(cli(json!({})).unwrap().check_complete(None)).contains("program"));
    }

    #[test]
    fn an_undeclared_binding_key_is_refused() {
        assert!(ToolCliFields::from_bucket(&json!({"program":"git","shell":true})).is_err());
        assert!(ToolMcpFields::from_bucket(&json!({"headers":[]})).is_err());
    }

    #[test]
    fn a_cleared_field_is_stored_as_null_and_reads_as_unset() {
        let fields = ToolCliFields::from_bucket(&json!({"program":null,"args":null})).unwrap();
        assert_eq!(fields.program, None);
        assert!(ToolCliFields::from_bucket(&Value::Null).is_ok());
    }
}
