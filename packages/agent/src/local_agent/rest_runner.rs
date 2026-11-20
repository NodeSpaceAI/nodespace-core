//! The runner of `tool-rest` tools: sends the one HTTP request a binding
//! describes, with the arguments the model chose, and returns the response as
//! data.
//!
//! The rules it keeps (each is tested against local servers and a fake
//! resolver; nothing touches the real network):
//!
//! - **The binding is read through the typed reader** ([`ToolRestFields`]) and
//!   must be complete; anything malformed fails closed before a request is made.
//!   The method, the headers and the destination come from the binding, never
//!   from the model.
//! - **The URL is built from the parsed template.** Each `{name}` placeholder
//!   is filled with the model's value percent-encoded (everything but
//!   `A-Za-z0-9-._~`), a value of `.`, `..` or the empty string is refused (a
//!   dot segment would climb out of the path), and the result is parsed again:
//!   its scheme, host and port must equal the template's, and everything the
//!   request uses comes from that parsed `Url`, never the stored text. Query
//!   parameters are added through the URL's own query serializer.
//! - **Model values go only into declared slots:** path placeholders, query
//!   parameters, and, for a `json` body, the remaining arguments as the JSON
//!   object. They never reach a header. With `body: none`, an argument that
//!   fits none of those slots is refused.
//! - **https, with one exception.** `https` always; `http` only for a loopback
//!   host (`localhost` or a loopback address), checked on every hop.
//! - **Addresses are checked after DNS resolution, and the connection is
//!   pinned to what was checked.** Every address a host resolves to must be
//!   allowed (a single private, link-local, unspecified, multicast, shared
//!   (100.64/10), documentation or otherwise reserved address refuses the
//!   call, as does any loopback address for a name other than `localhost`);
//!   the first is used, and the client's resolver answers only with it, so the
//!   connection goes to the address that was checked with the original host
//!   kept for `Host` and TLS. The hostname is resolved once per hop.
//! - **Redirects are followed only within the same origin** (scheme, host and
//!   port), at most three, never to another origin and never carrying
//!   credentials elsewhere: a redirect to any other origin (another host, port
//!   or scheme, `http` included) is refused, and each hop is checked again from
//!   the start. A redirect that would change a non-`GET` method is refused.
//! - **Credentials.** The binding's `auth` reference is resolved only here,
//!   goes into one header of the requests to the binding's own origin, is held
//!   in zeroized memory, and is redacted from everything returned. It is sent
//!   over `https`, or over `http` only to a loopback host (it cannot leave the
//!   machine). A value with a control character, or too short to redact, is
//!   refused.
//! - **No proxy, no cookies, no compression.** Direct connections only: the
//!   `HTTP_PROXY` family of variables is ignored, so a credential is never
//!   silently routed through a proxy. No cookie store; no content decoding.
//! - **Limits.** A total timeout covers connecting, every hop and reading the
//!   body (a server that sends a byte at a time is stopped); the response is
//!   read as a stream and capped; a request body is capped.
//! - **The response is data.** Only text and JSON-like content types are
//!   returned (no HTML is rendered, nothing is decoded as an image or
//!   archive); no response header is returned. A non-success status is an
//!   error with a bounded, redacted piece of the body.
//!
//! Limits: the system resolver runs on the blocking thread pool, so a lookup that
//! never returns can hold one pool thread for as long as the OS lets it (the
//! call itself is stopped by the timeout); redaction replaces exact values, so a
//! URL-encoded or JSON-escaped echo of a value that contains `\"`, `\\` or `+`
//! is not caught; a model-controlled query argument can carry data to the bound
//! origin, by design; the credential is held in the
//! `HeaderValue` the HTTP stack keeps while the request is in flight; one
//! resolver answer is used for a hop, so a name that points at a different
//! public address later is followed on the next hop only.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nodespace_core::secrets::SecretStore;
use nodespace_types::{ToolRestFields, DEFAULT_RESPONSE_BYTES, DEFAULT_TIMEOUT_SECONDS};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use reqwest::Method;
use serde_json::{Map, Value};
use url::Url;
use zeroize::Zeroizing;

use super::net_policy::{
    checked_address, credential_note, destination, http_credential, request_error, send_following,
    FixedResolver, HostResolver, HttpCredential, Outgoing, ResolvedPin, SystemResolver,
    CONNECT_TIMEOUT,
};
use super::process::EnvLookup;
use super::redaction::{cut, sanitize};
use super::remote_tools::{
    Confirmation, RemoteCallError, RemoteOutput, RemoteTool, RemoteToolKind, RemoteToolRunner,
};

/// Characters left as they are in a placeholder value: the URL's unreserved set.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
/// Largest request body sent, in bytes.
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
/// How much of a failing response an error message carries.
const ERROR_BODY_BYTES: usize = 1500;
/// Longest confirmation text.
const MAX_DESTINATION_BYTES: usize = 4096;
/// Longest path-and-query part of the confirmation text.
const MAX_PATH_SHOWN: usize = 1500;

fn fail<T>(message: impl Into<String>) -> Result<T, RemoteCallError> {
    Err(RemoteCallError(message.into()))
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

/// Runs `tool-rest` tools.
pub struct RestRunner {
    secrets: Arc<SecretStore>,
    daemon_env: EnvLookup,
    resolver: Arc<dyn HostResolver>,
    /// How many times a client asked its resolver, which must be the pinned one:
    /// a lookup that went anywhere else is not counted.
    pinned_lookups: Arc<AtomicUsize>,
}

impl RestRunner {
    /// A runner whose credentials come from `secrets`, using the system
    /// resolver and this process's environment for `env:` references.
    pub fn new(secrets: Arc<SecretStore>) -> Self {
        Self {
            secrets,
            daemon_env: Arc::new(|name| std::env::var(name).ok()),
            resolver: Arc::new(SystemResolver),
            pinned_lookups: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A runner with its own resolver and view of the environment, for tests.
    pub fn with_parts(
        secrets: Arc<SecretStore>,
        lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
        resolver: Arc<dyn HostResolver>,
    ) -> Self {
        Self {
            secrets,
            daemon_env: Arc::new(lookup),
            resolver,
            pinned_lookups: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// What one call sends.
struct Plan {
    method: Method,
    url: Url,
    body: Option<Vec<u8>>,
    /// The body as JSON text, for the confirmation question.
    body_text: Option<String>,
}

fn read_binding(tool: &RemoteTool) -> Result<ToolRestFields, RemoteCallError> {
    let fields = ToolRestFields::from_bucket(tool.binding())
        .map_err(|e| RemoteCallError(format!("the tool's binding is not valid: {}", e.0)))?;
    fields
        .check_complete(Some(&tool.parameter_schema))
        .map_err(|e| RemoteCallError(format!("the tool's binding is not complete: {}", e.0)))?;
    Ok(fields)
}

/// A model value as the text that goes in a placeholder or a query parameter.
fn scalar(name: &str, value: &Value) -> Result<String, RemoteCallError> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return fail(format!("`{name}` must be a string, number or boolean")),
    };
    if text.contains('\0') {
        return fail(format!("`{name}` cannot contain a NUL character"));
    }
    Ok(text)
}

/// A value for a path placeholder: percent-encoded, and never a dot segment.
fn path_value(name: &str, value: &Value) -> Result<String, RemoteCallError> {
    let text = scalar(name, value)?;
    if text.is_empty() || text == "." || text == ".." {
        return fail(format!(
            "`{name}` cannot be empty, `.` or `..` (it would change the path)"
        ));
    }
    Ok(utf8_percent_encode(&text, UNRESERVED).to_string())
}

/// The request for a call: the URL from the parsed template, the method from the
/// binding, a body from the arguments no other slot took.
fn plan(fields: &ToolRestFields, args: &Value) -> Result<Plan, RemoteCallError> {
    let template = fields
        .url_template
        .as_deref()
        .ok_or_else(|| RemoteCallError("the tool's binding has no url_template".to_string()))?;
    let method = fields
        .method
        .as_deref()
        .ok_or_else(|| RemoteCallError("the tool's binding has no method".to_string()))?;
    let method = Method::from_bytes(method.as_bytes())
        .map_err(|_| RemoteCallError("the tool's binding has an invalid method".to_string()))?;
    let empty = Map::new();
    let args = args.as_object().unwrap_or(&empty);

    // The template, with each placeholder filled by its encoded value.
    let mut text = String::new();
    let mut rest = template;
    let mut used: Vec<&str> = Vec::new();
    while let Some(open) = rest.find('{') {
        text.push_str(&rest[..open]);
        let close = rest[open..]
            .find('}')
            .map(|c| open + c)
            .ok_or_else(|| RemoteCallError("the url_template is malformed".to_string()))?;
        let name = &rest[open + 1..close];
        let value = args
            .get(name)
            .ok_or_else(|| RemoteCallError(format!("`{name}` is required")))?;
        text.push_str(&path_value(name, value)?);
        used.push(name);
        rest = &rest[close + 1..];
    }
    text.push_str(rest);
    let mut url = Url::parse(&text)
        .map_err(|_| RemoteCallError("the request URL could not be built".to_string()))?;

    // The destination is the template's, however the placeholders were filled.
    let blank = template_with(template, "x");
    let expected = Url::parse(&blank)
        .map_err(|_| RemoteCallError("the url_template is malformed".to_string()))?;
    if (url.scheme(), url.host_str(), url.port())
        != (expected.scheme(), expected.host_str(), expected.port())
    {
        return fail("the request URL does not go where the binding says");
    }

    // Query parameters, through the URL's own serializer.
    let mut query: Vec<(&str, String)> = Vec::new();
    for name in fields.query_params.iter().flatten() {
        if let Some(value) = args.get(name.as_str()) {
            query.push((name.as_str(), scalar(name, value)?));
            used.push(name.as_str());
        }
    }
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in &query {
            pairs.append_pair(name, value);
        }
    }

    // The body: whatever no other slot took.
    let leftover: Map<String, Value> = args
        .iter()
        .filter(|(key, _)| !used.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let (body, body_text) = match fields.body.as_deref() {
        Some("json") => {
            let text = serde_json::to_string(&Value::Object(leftover))
                .map_err(|_| RemoteCallError("the body could not be built".to_string()))?;
            if text.len() > MAX_REQUEST_BODY_BYTES {
                return fail("the request body is too large");
            }
            (Some(text.clone().into_bytes()), Some(text))
        }
        _ => {
            if let Some(name) = leftover.keys().next() {
                return fail(format!(
                    "`{name}` has nowhere to go: the tool takes it neither in the URL, the query nor a body"
                ));
            }
            (None, None)
        }
    };
    Ok(Plan {
        method,
        url,
        body,
        body_text,
    })
}

/// `template` with every placeholder replaced by `fill`.
fn template_with(template: &str, fill: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        out.push_str(fill);
        rest = &rest[rest.find('}').map_or(rest.len(), |c| c + 1)..];
    }
    out.push_str(rest);
    out
}

/// Whether a response of `content_type` is returned as data: text and JSON-like
/// types only.
fn is_text_type(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    essence.starts_with("text/")
        || essence.contains("json")
        || essence.contains("xml")
        || essence == "application/x-www-form-urlencoded"
        || essence == "application/javascript"
}

impl RestRunner {
    /// One request, with the redirects the policy allows.
    async fn exchange(
        &self,
        plan: Plan,
        headers: HeaderMap,
        credential: &Option<HttpCredential>,
        needles: &[Zeroizing<String>],
        limit: Duration,
        cap: usize,
    ) -> Result<RemoteOutput, RemoteCallError> {
        let mut headers = headers;
        if let Some(credential) = credential {
            headers.insert(credential.name.clone(), credential.value.clone());
        }
        if plan.body.is_some() {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        let mut response = send_following(
            &Outgoing {
                resolver: &*self.resolver,
                lookups: &self.pinned_lookups,
                limit,
                needles,
            },
            plan.method,
            plan.url,
            &headers,
            plan.body.as_deref(),
        )
        .await?;
        let status = response.status();
        // (the response is read here)
        {
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            if let Some(kind) = &content_type {
                if !is_text_type(kind) {
                    // Nothing from the response headers is repeated: it is the
                    // server's text, and the credential may be in it.
                    return fail("the server answered with a content type that is not text or JSON, which is not returned");
                }
            }
            // Read as a stream; stop at the cap.
            let mut bytes: Vec<u8> = Vec::new();
            let mut stopped_at_cap = false;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        bytes.extend_from_slice(&chunk);
                        if bytes.len() >= cap {
                            stopped_at_cap = true;
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => return Err(request_error(e, limit, needles)),
                }
            }
            if content_type.is_none() && std::str::from_utf8(&bytes).is_err() && !stopped_at_cap {
                return fail("the server answered without a content type and with non-text data");
            }
            let mut text = sanitize(&bytes, needles, stopped_at_cap);
            let truncated = stopped_at_cap || text.len() > cap;
            cut(&mut text, cap);
            if status.is_success() {
                Ok(RemoteOutput { text, truncated })
            } else {
                cut(&mut text, ERROR_BODY_BYTES);
                fail(format!("the server answered HTTP {status}: {text}"))
            }
        }
    }
}

/// The confirmation text, built from the call and, when known, where the host
/// resolved to.
fn describe(fields: &ToolRestFields, plan: &Plan, resolved: Option<&str>) -> String {
    let url = &plan.url;
    let host = url.host_str().unwrap_or_default();
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let mut shown = format!("{} {}://{host}{port}", plan.method, url.scheme());
    match resolved {
        Some(addresses) => shown.push_str(&format!(" (resolved to {addresses})")),
        None => shown.push_str(" (the address is checked when it is sent)"),
    }
    // The credential comes before the parts that can be long.
    shown.push_str(&credential_note(fields.auth.as_ref()));
    let mut path = url[url::Position::BeforePath..].to_string();
    if path.len() > MAX_PATH_SHOWN {
        cut(&mut path, MAX_PATH_SHOWN);
        path.push('\u{2026}');
    }
    shown.push_str(&format!("; path and query: {path}"));
    if let Some(body) = &plan.body_text {
        let budget = MAX_DESTINATION_BYTES.saturating_sub(shown.len() + "; body: ".len() + 3);
        let mut body = body.clone();
        if body.len() > budget {
            cut(&mut body, budget);
            body.push('\u{2026}');
        }
        shown.push_str(&format!("; body: {body}"));
    }
    cut(&mut shown, MAX_DESTINATION_BYTES);
    shown
}

#[async_trait]
impl RemoteToolRunner for RestRunner {
    fn kind(&self) -> RemoteToolKind {
        RemoteToolKind::Rest
    }

    fn destination(&self, tool: &RemoteTool, args: &Value) -> String {
        let fields = match read_binding(tool) {
            Ok(fields) => fields,
            Err(e) => return format!("(unusable binding: {})", e.0),
        };
        match plan(&fields, args) {
            Ok(plan) => describe(&fields, &plan, None),
            Err(e) => format!("(unusable call: {})", e.0),
        }
    }

    async fn confirmation_text(
        &self,
        tool: &RemoteTool,
        args: &Value,
    ) -> Result<String, RemoteCallError> {
        Ok(self.confirmation(tool, args).await?.text)
    }

    async fn confirmation(
        &self,
        tool: &RemoteTool,
        args: &Value,
    ) -> Result<Confirmation, RemoteCallError> {
        let fields = read_binding(tool)?;
        let plan = plan(&fields, args)?;
        let destination = destination(&plan.url)?;
        // A resolver that never answers must not hold the question: a timeout is a
        // refused destination.
        let ip = tokio::time::timeout(
            CONNECT_TIMEOUT,
            checked_address(&*self.resolver, &destination),
        )
        .await
        .map_err(|_| {
            RemoteCallError(format!(
                "`{}` could not be resolved in time: the request was not sent",
                destination.host
            ))
        })??;
        Ok(Confirmation {
            text: describe(&fields, &plan, Some(&ip.to_string())),
            // The run uses this address, not a second lookup.
            pin: Some(ResolvedPin {
                host: destination.host.clone(),
                address: ip,
            }),
        })
    }

    async fn run_pinned(
        &self,
        tool: &RemoteTool,
        args: &Value,
        pin: &ResolvedPin,
    ) -> Result<RemoteOutput, RemoteCallError> {
        let pinned = RestRunner {
            secrets: Arc::clone(&self.secrets),
            daemon_env: Arc::clone(&self.daemon_env),
            resolver: Arc::new(FixedResolver { pin: pin.clone() }),
            pinned_lookups: Arc::clone(&self.pinned_lookups),
        };
        pinned.run(tool, args).await
    }

    async fn run(&self, tool: &RemoteTool, args: &Value) -> Result<RemoteOutput, RemoteCallError> {
        let fields = read_binding(tool)?;
        let plan = plan(&fields, args)?;
        // Scheme and host rules before anything is resolved or read.
        destination(&plan.url)?;
        let limit = Duration::from_secs(fields.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS));
        let cap = fields.max_response_bytes.unwrap_or(DEFAULT_RESPONSE_BYTES) as usize;
        let work = async {
            // The credential store read, the name lookup, every hop and the body
            // all count against the one timeout.
            let credential =
                http_credential(&self.secrets, &self.daemon_env, fields.auth.as_ref()).await?;
            if credential.is_some() {
                // Redundant with `destination` (http only reaches a loopback host),
                // kept as a second guard: a credential goes over https, or over
                // http only to a loopback host.
                let d = destination(&plan.url)?;
                if plan.url.scheme() != "https" && !d.loopback {
                    return fail("a credential is only sent over https");
                }
            }
            let needles: Vec<Zeroizing<String>> = credential
                .iter()
                .flat_map(|c| c.needles.iter().cloned())
                .collect();
            let mut headers = HeaderMap::new();
            for header in fields.headers.iter().flatten() {
                let name = HeaderName::from_bytes(header.name.as_bytes()).map_err(|_| {
                    RemoteCallError("the tool's binding is not valid: a header name".to_string())
                })?;
                let value = HeaderValue::from_str(&header.value).map_err(|_| {
                    RemoteCallError("the tool's binding is not valid: a header value".to_string())
                })?;
                headers.append(name, value);
            }
            self.exchange(plan, headers, &credential, &needles, limit, cap)
                .await
        };
        match tokio::time::timeout(limit, work).await {
            Ok(result) => result,
            Err(_) => fail(format!(
                "the request did not finish within {} seconds",
                limit.as_secs()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_agent::net_policy::{base64, blocked_reason, pick_address};
    use crate::local_agent::test_http::{http, ip, ok_json, serve, FakeResolver, Reply, Request};
    use nodespace_core::secrets::InMemorySecretBackend;
    use serde_json::json;
    use std::net::IpAddr;
    use std::sync::atomic::Ordering;
    use std::time::Instant;
    use url::Url;

    // -- helpers -----------------------------------------------------------

    fn schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "id": {"type": "string"},
                "q": {"type": "string"},
                "n": {"type": "integer"},
                "o": {"type": "object"},
                "title": {"type": "string"},
                "extra": {"type": "string"},
            },
        })
    }

    fn tool(binding: Value) -> RemoteTool {
        RemoteTool {
            node_id: "node-1".into(),
            name: "r_tool".into(),
            kind: RemoteToolKind::Rest,
            description: "d".into(),
            parameter_schema: schema(),
            properties: json!({ "tool-rest": binding }),
            approval_tag: String::new(),
        }
    }

    fn store() -> Arc<SecretStore> {
        Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default())))
    }

    fn runner(secrets: Arc<SecretStore>) -> RestRunner {
        RestRunner::with_parts(secrets, |_| None, Arc::new(FakeResolver::default()))
    }

    fn get(port: u16, path: &str) -> Value {
        json!({"method": "GET", "url_template": format!("http://127.0.0.1:{port}{path}")})
    }

    async fn run(
        r: &RestRunner,
        binding: Value,
        args: Value,
    ) -> Result<RemoteOutput, RemoteCallError> {
        r.run(&tool(binding), &args).await
    }

    fn request_line(request: &Request) -> &str {
        request.head.lines().next().unwrap()
    }

    // -- the URL ------------------------------------------------------------

    #[tokio::test]
    async fn placeholders_are_percent_encoded_in_the_parsed_url() {
        let server = serve(|_| ok_json("{}")).await;
        let r = runner(store());
        run(
            &r,
            get(server.port, "/items/{id}"),
            json!({"id": "a b/c?d#e&f=g"}),
        )
        .await
        .unwrap();
        assert_eq!(
            request_line(&server.requests()[0]),
            "GET /items/a%20b%2Fc%3Fd%23e%26f%3Dg HTTP/1.1"
        );
        // Unreserved characters stay, and a number is allowed.
        run(
            &r,
            get(server.port, "/items/{id}"),
            json!({"id": "A-z_0.9~"}),
        )
        .await
        .unwrap();
        assert_eq!(
            request_line(&server.requests()[1]),
            "GET /items/A-z_0.9~ HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn a_dot_segment_or_empty_value_is_refused_before_anything_is_sent() {
        let server = serve(|_| ok_json("{}")).await;
        let r = runner(store());
        for bad in ["", ".", ".."] {
            let err = run(&r, get(server.port, "/items/{id}/x"), json!({"id": bad}))
                .await
                .unwrap_err();
            assert!(err.0.contains("`id`"), "{bad:?}: {}", err.0);
        }
        // A value that only contains dots is encoded, not a dot segment.
        run(&r, get(server.port, "/items/{id}/x"), json!({"id": "..."}))
            .await
            .unwrap();
        assert_eq!(server.hits(), 1);
        // Neither a structured value nor a missing one.
        assert!(
            run(&r, get(server.port, "/items/{id}"), json!({"id": {"a": 1}}))
                .await
                .is_err()
        );
        assert!(run(&r, get(server.port, "/items/{id}"), json!({}))
            .await
            .is_err());
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn query_parameters_are_added_through_the_serializer_and_absent_ones_skipped() {
        let server = serve(|_| ok_json("{}")).await;
        let r = runner(store());
        let mut binding = get(server.port, "/search");
        binding["query_params"] = json!(["q", "n"]);
        run(&r, binding.clone(), json!({"q": "x&y=z #1"}))
            .await
            .unwrap();
        assert_eq!(
            request_line(&server.requests()[0]),
            "GET /search?q=x%26y%3Dz+%231 HTTP/1.1"
        );
        run(&r, binding.clone(), json!({"q": "a", "n": 3}))
            .await
            .unwrap();
        assert_eq!(
            request_line(&server.requests()[1]),
            "GET /search?q=a&n=3 HTTP/1.1"
        );
        assert!(run(&r, binding, json!({"q": {"a": 1}})).await.is_err());
    }

    #[tokio::test]
    async fn the_method_and_headers_come_from_the_binding_and_the_body_from_the_leftover_arguments()
    {
        let server = serve(|_| ok_json("{}")).await;
        let r = runner(store());
        let mut binding = json!({
            "method": "POST",
            "url_template": format!("http://127.0.0.1:{}/items/{{id}}", server.port),
            "body": "json",
            "headers": [{"name": "X-Env", "value": "prod"}],
        });
        run(
            &r,
            binding.clone(),
            json!({"id": "7", "title": "T-VALUE", "n": 3}),
        )
        .await
        .unwrap();
        let request = &server.requests()[0];
        assert_eq!(request_line(request), "POST /items/7 HTTP/1.1");
        assert!(request.head.to_ascii_lowercase().contains("x-env: prod"));
        assert!(request
            .head
            .to_ascii_lowercase()
            .contains("content-type: application/json"));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body, json!({"title": "T-VALUE", "n": 3}));
        // Model values never reach a header.
        assert!(!request.head.contains("T-VALUE"), "{}", request.head);
        // With no body slot, an argument with nowhere to go is refused.
        binding["body"] = json!("none");
        let err = run(&r, binding, json!({"id": "7", "title": "t"}))
            .await
            .unwrap_err();
        assert!(err.0.contains("`title`"), "{}", err.0);
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn an_oversize_request_body_is_refused() {
        let server = serve(|_| ok_json("{}")).await;
        let r = runner(store());
        let binding = json!({
            "method": "POST",
            "url_template": format!("http://127.0.0.1:{}/x", server.port),
            "body": "json",
        });
        let big = "x".repeat(60 * 1024);
        let err = run(&r, binding, json!({"o": {"a": big.clone(), "b": big}})).await;
        assert!(err.is_err());
        assert_eq!(server.hits(), 0);
    }

    // -- credentials ---------------------------------------------------------

    const TOKEN: &str = "SECRETVALUE-123";

    fn with_token(scheme: &str, extra: Value) -> (Arc<SecretStore>, Value) {
        let secrets = store();
        secrets.set("tok", TOKEN).unwrap();
        let mut auth = json!({"scheme": scheme, "secret": "keychain:tok"});
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            auth[k] = v;
        }
        (secrets, auth)
    }

    /// The JSON content type is set, not appended: a declared `Content-Type` does
    /// not make a second line (the credential header is `insert`ed the same way;
    /// a declared header of its name is refused by the binding).
    #[tokio::test]
    async fn a_declared_content_type_does_not_duplicate_the_json_one() {
        let server = serve(|_| ok_json("{}")).await;
        let (secrets, _) = with_token("bearer", json!({}));
        let binding = json!({
            "method": "POST",
            "url_template": format!("http://127.0.0.1:{}/x", server.port),
            "body": "json",
            "headers": [
                {"name": "Content-Type", "value": "text/plain"}
            ],
            "auth": {"scheme": "bearer", "secret": "keychain:tok"},
        });
        run(&runner(secrets), binding, json!({"title": "t"}))
            .await
            .unwrap();
        let head = server.requests()[0].head.to_ascii_lowercase();
        assert_eq!(head.matches("content-type:").count(), 1, "{head}");
        assert!(head.contains("content-type: application/json"), "{head}");
        assert_eq!(head.matches("authorization:").count(), 1, "{head}");
    }

    #[tokio::test]
    async fn a_credential_is_sent_in_the_bound_header_and_redacted_from_what_comes_back() {
        for (scheme, extra, expect) in [
            (
                "bearer",
                json!({}),
                format!("authorization: Bearer {TOKEN}"),
            ),
            (
                "basic",
                json!({}),
                format!("authorization: Basic {}", base64(TOKEN.as_bytes())),
            ),
            (
                "header",
                json!({"header_name": "X-Api-Key"}),
                format!("x-api-key: {TOKEN}"),
            ),
        ] {
            let (secrets, auth) = with_token(scheme, extra);
            // The server echoes what it was sent, in the body and in a header.
            let server = serve(|request| {
                Reply::Whole(http(
                    "200 OK",
                    &[
                        ("Content-Type", "text/plain"),
                        ("X-Echo", &request.head.replace("\r\n", "|")),
                    ],
                    request.head.as_bytes(),
                ))
            })
            .await;
            let mut binding = get(server.port, "/x");
            binding["auth"] = auth;
            let out = run(&runner(secrets), binding, json!({})).await.unwrap();
            let head = server.requests()[0].head.to_ascii_lowercase();
            assert!(
                head.contains(&expect.to_ascii_lowercase()) || head.contains(&expect),
                "{scheme}: {head}"
            );
            assert!(out.text.contains("[redacted]"), "{scheme}: {}", out.text);
            assert!(
                !out.text.contains(&base64(TOKEN.as_bytes())),
                "{scheme}: {}",
                out.text
            );
            assert!(!out.text.contains(TOKEN), "{scheme}: {}", out.text);
            // No response header comes back.
            assert!(!out.text.contains("X-Echo") && !out.text.contains("x-echo"));
        }
    }

    #[tokio::test]
    async fn a_missing_short_or_unsendable_credential_stops_the_call_before_any_request() {
        let server = serve(|_| ok_json("{}")).await;
        let secrets = store();
        secrets.set("short", "abc").unwrap();
        secrets.set("crlf", "abcd\r\nX-Evil: 1").unwrap();
        secrets.set("ctl", "abcd\u{7}efgh").unwrap();
        let r = runner(secrets);
        for (reference, expect) in [
            ("keychain:absent", "not set on this device"),
            ("keychain:short", "too short"),
            ("keychain:crlf", "cannot be sent as a header value"),
            ("keychain:ctl", "cannot be sent as a header value"),
            ("env:NOT_SET_ANYWHERE", "not set in the environment"),
        ] {
            let mut binding = get(server.port, "/x");
            binding["auth"] = json!({"scheme": "bearer", "secret": reference});
            let err = run(&r, binding, json!({})).await.unwrap_err();
            assert!(err.0.contains(expect), "{reference}: {}", err.0);
        }
        assert_eq!(server.hits(), 0);
    }

    // -- the destination -----------------------------------------------------

    #[test]
    fn the_address_policy_blocks_private_link_local_and_reserved_addresses() {
        for blocked in [
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "0.1.2.3",
            "100.64.0.1",
            "100.127.255.254",
            "224.0.0.1",
            "255.255.255.255",
            "240.0.0.1",
            "192.0.2.1",
            "198.51.100.7",
            "203.0.113.9",
            "198.18.0.1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "::",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a00:1",
            "2001:db8::1",
            "::1.2.3.4",
            "fec0::1",
            // Transition and special-purpose ranges.
            "2002:a00:1::1",
            "2002:7f00:1::1",
            "2002:a9fe:a9fe::1",
            "2002::1",
            "2001::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "100::1",
            "3fff::1",
            "3fff:fff::1",
            "64:ff9b:1::1",
            "5f00::1",
            "192.0.0.8",
            "192.0.0.1",
            "192.88.99.1",
        ] {
            assert!(
                blocked_reason(ip(blocked), false).is_some(),
                "{blocked} must be blocked"
            );
            // Being named loopback does not unblock anything but loopback.
            assert!(
                blocked_reason(ip(blocked), true).is_some(),
                "{blocked} must be blocked even for localhost"
            );
        }
        for allowed in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "172.32.0.1",
            "100.63.0.1",
            "2606:4700:4700::1111",
        ] {
            assert!(
                blocked_reason(ip(allowed), false).is_none(),
                "{allowed} must be allowed"
            );
        }
        // Loopback: only for a destination the binding names as loopback.
        for loopback in ["127.0.0.1", "127.1.2.3", "::1", "::ffff:127.0.0.1"] {
            assert!(
                blocked_reason(ip(loopback), false).is_some(),
                "{loopback} by name"
            );
            assert!(
                blocked_reason(ip(loopback), true).is_none(),
                "{loopback} for localhost"
            );
        }
    }

    #[test]
    fn one_blocked_address_in_an_answer_refuses_the_call() {
        let mixed = [ip("8.8.8.8"), ip("10.0.0.5")];
        let err = pick_address("api.example", &mixed, false).unwrap_err();
        assert!(
            err.0.contains("10.0.0.5") && err.0.contains("private"),
            "{}",
            err.0
        );
        assert_eq!(
            pick_address("api.example", &[ip("8.8.8.8"), ip("1.1.1.1")], false).unwrap(),
            ip("8.8.8.8")
        );
        assert!(pick_address("api.example", &[], false).is_err());
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_a_forbidden_address_is_refused_after_resolution() {
        for (answers, expect) in [
            (vec![vec![ip("10.0.0.5")]], "private"),
            (vec![vec![ip("169.254.169.254")]], "link-local"),
            (vec![vec![ip("127.0.0.1")]], "loopback"),
            (vec![vec![ip("8.8.8.8"), ip("192.168.0.1")]], "private"),
            (vec![vec![ip("fd00::1")]], "unique-local"),
            (vec![vec![ip("::ffff:10.0.0.1")]], "private"),
        ] {
            let resolver = FakeResolver::with("api.example", answers);
            let r = RestRunner::with_parts(store(), |_| None, resolver.clone());
            let binding = json!({"method": "GET", "url_template": "https://api.example/x"});
            let err = run(&r, binding, json!({})).await.unwrap_err();
            assert!(
                err.0.contains(expect) && err.0.contains("not sent"),
                "{expect}: {}",
                err.0
            );
        }
        // Addresses written in the URL follow the same rules.
        for host in [
            "10.0.0.1",
            "169.254.169.254",
            "[fd00::1]",
            "[::ffff:192.168.0.1]",
        ] {
            let r = runner(store());
            let binding = json!({"method": "GET", "url_template": format!("https://{host}/x")});
            let err = run(&r, binding, json!({})).await.unwrap_err();
            assert!(err.0.contains("not sent"), "{host}: {}", err.0);
        }
    }

    #[tokio::test]
    async fn the_connection_is_pinned_to_the_address_that_was_checked() {
        let server = serve(|_| ok_json("{\"ok\":true}")).await;
        // The first answer is the loopback address the server is on; a rebinding
        // second answer is a private address the call must never see.
        let resolver = FakeResolver::with(
            "localhost",
            vec![vec![ip("127.0.0.1")], vec![ip("10.0.0.5")]],
        );
        let r = RestRunner::with_parts(store(), |_| None, resolver.clone());
        let binding =
            json!({"method": "GET", "url_template": format!("http://localhost:{}/x", server.port)});
        let out = run(&r, binding, json!({})).await.unwrap();
        assert_eq!(out.text, "{\"ok\":true}");
        // Resolved once, and the client asked only the pinned resolver.
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(r.pinned_lookups.load(Ordering::SeqCst), 1);
        assert_eq!(server.hits(), 1);
    }

    #[test]
    fn the_scheme_and_user_information_rules_hold_on_every_url() {
        let check = |text: &str| {
            destination(&Url::parse(text).unwrap()).map(|d| (d.host, d.port, d.loopback))
        };
        assert_eq!(
            check("https://example.com/x").unwrap(),
            ("example.com".to_string(), 443, false)
        );
        assert_eq!(
            check("http://localhost:8080/x").unwrap(),
            ("localhost".to_string(), 8080, true)
        );
        assert!(check("http://127.0.0.1/x").unwrap().2);
        assert!(check("http://[::1]/x").unwrap().2);
        assert!(check("http://example.com/x").is_err());
        assert!(check("http://10.0.0.1/x").is_err());
        assert!(check("ftp://example.com/x").is_err());
        assert!(check("https://user:pw@example.com/x").is_err());
        assert!(check("https://user@example.com/x").is_err());
        // A name that merely sounds like localhost is not loopback.
        assert!(!check("https://localhost.example.com/x").unwrap().2);
        assert!(check("http://localhost.example.com/x").is_err());
    }

    #[tokio::test]
    async fn a_binding_that_is_not_well_formed_fails_closed() {
        let r = runner(store());
        for (why, binding) in [
            (
                "a plain http host",
                json!({"method": "GET", "url_template": "http://api.example.com/x"}),
            ),
            (
                "no method",
                json!({"url_template": "https://api.example.com/x"}),
            ),
            (
                "an undeclared placeholder",
                json!({"method": "GET", "url_template": "https://api.example.com/{nope}"}),
            ),
            (
                "a templated host",
                json!({"method": "GET", "url_template": "https://{id}.example.com/x"}),
            ),
            (
                "a credential header",
                json!({"method": "GET", "url_template": "https://a.example.com/", "headers": [{"name": "Authorization", "value": "x"}]}),
            ),
            (
                "a literal secret",
                json!({"method": "GET", "url_template": "https://a.example.com/", "auth": {"scheme": "bearer", "secret": "ghp_literal"}}),
            ),
            (
                "an unknown key",
                json!({"method": "GET", "url_template": "https://a.example.com/", "proxy": "x"}),
            ),
        ] {
            let err = run(&r, binding, json!({"id": "a"})).await.unwrap_err();
            assert!(err.0.contains("binding"), "{why}: {}", err.0);
        }
    }

    // -- redirects ---------------------------------------------------------------

    #[tokio::test]
    async fn same_origin_redirects_are_followed_with_method_and_body_kept_for_307() {
        let server = serve(|request| {
            if request_line(request).contains("/old") {
                Reply::Whole(http("307 Temporary Redirect", &[("Location", "/new")], b""))
            } else {
                Reply::Whole(http(
                    "200 OK",
                    &[("Content-Type", "application/json")],
                    request.body.as_slice(),
                ))
            }
        })
        .await;
        let r = runner(store());
        let binding = json!({
            "method": "POST",
            "url_template": format!("http://127.0.0.1:{}/old", server.port),
            "body": "json",
        });
        let out = run(&r, binding, json!({"title": "kept"})).await.unwrap();
        assert_eq!(out.text, "{\"title\":\"kept\"}");
        let seen = server.requests();
        assert_eq!(seen.len(), 2);
        assert!(request_line(&seen[1]).starts_with("POST /new"));
    }

    #[tokio::test]
    async fn a_redirect_to_another_origin_is_never_followed_and_no_credential_is_sent_there() {
        let other = serve(|_| ok_json("{\"leak\":true}")).await;
        let other_port = other.port;
        for location in [
            format!("http://127.0.0.1:{other_port}/steal"), // another port
            format!("http://localhost:{other_port}/steal"), // another host name
            "http://10.0.0.1/internal".to_string(),         // a private address
            "https://127.0.0.1/secure".to_string(),         // another scheme
            "http://169.254.169.254/latest/meta-data".to_string(),
            "//evil.example/x".to_string(),
            "http://user:pw@127.0.0.1/x".to_string(),
        ] {
            let loc = location.clone();
            let first =
                serve(move |_| Reply::Whole(http("302 Found", &[("Location", &loc)], b""))).await;
            let (secrets, auth) = with_token("bearer", json!({}));
            let mut binding = get(first.port, "/start");
            binding["auth"] = auth;
            let err = run(&runner(secrets), binding, json!({})).await.unwrap_err();
            assert!(
                err.0.contains("different origin") || err.0.contains("unusable redirect"),
                "{location}: {}",
                err.0
            );
            assert_eq!(first.hits(), 1, "{location}");
        }
        // The other server saw nothing, so nothing carried a credential to it.
        assert_eq!(other.hits(), 0);
    }

    #[tokio::test]
    async fn redirects_are_limited_and_a_redirect_that_would_change_the_method_is_not_followed() {
        let looping =
            serve(|_| Reply::Whole(http("302 Found", &[("Location", "/again")], b""))).await;
        let err = run(&runner(store()), get(looping.port, "/start"), json!({}))
            .await
            .unwrap_err();
        assert!(err.0.contains("too many"), "{}", err.0);
        assert_eq!(
            looping.hits(),
            4,
            "the original request and three redirects"
        );

        let moved =
            serve(|_| Reply::Whole(http("302 Found", &[("Location", "/elsewhere")], b""))).await;
        let binding = json!({
            "method": "POST",
            "url_template": format!("http://127.0.0.1:{}/x", moved.port),
            "body": "json",
        });
        let err = run(&runner(store()), binding, json!({"title": "t"}))
            .await
            .unwrap_err();
        assert!(err.0.contains("not a GET"), "{}", err.0);
        assert_eq!(moved.hits(), 1);
    }

    // -- the response ---------------------------------------------------------------

    #[tokio::test]
    async fn the_response_is_read_as_a_stream_and_stopped_at_the_cap() {
        let endless = serve(|_| Reply::Stream {
            head: b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n"
                .to_vec(),
            chunk: vec![b'x'; 512],
            every: Duration::from_millis(1),
        })
        .await;
        let mut binding = get(endless.port, "/big");
        binding["max_response_bytes"] = json!(2000);
        let started = Instant::now();
        let out = run(&runner(store()), binding, json!({})).await.unwrap();
        assert!(out.truncated);
        assert!(
            out.text.len() <= 2000 && out.text.len() > 1500,
            "{}",
            out.text.len()
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        // The server was cut off, not waited for.
        for _ in 0..40 {
            if endless.closed.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            endless.closed.load(Ordering::SeqCst),
            "the connection was not closed at the cap"
        );
    }

    #[tokio::test]
    async fn a_server_that_drips_is_stopped_by_the_total_timeout() {
        let drip = serve(|_| Reply::Stream {
            head: b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n"
                .to_vec(),
            chunk: b"x".to_vec(),
            every: Duration::from_millis(200),
        })
        .await;
        let mut binding = get(drip.port, "/slow");
        binding["timeout_seconds"] = json!(1);
        let started = Instant::now();
        let err = run(&runner(store()), binding, json!({})).await.unwrap_err();
        assert!(
            err.0.contains("did not finish within 1 seconds"),
            "{}",
            err.0
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_cancelled_call_closes_its_connection() {
        let drip = serve(|_| Reply::Stream {
            head: b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n"
                .to_vec(),
            chunk: b"x".to_vec(),
            every: Duration::from_millis(50),
        })
        .await;
        let r = runner(store());
        let t = tool(get(drip.port, "/slow"));
        let no_args = json!({});
        let _ = tokio::time::timeout(Duration::from_millis(600), r.run(&t, &no_args)).await;
        for _ in 0..40 {
            if drip.closed.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            drip.closed.load(Ordering::SeqCst),
            "the server was still being read from"
        );
    }

    #[tokio::test]
    async fn only_text_and_json_are_returned_as_data() {
        for (kind, body, ok) in [
            ("application/json", "{\"a\":1}", true),
            ("application/problem+json; charset=utf-8", "{}", true),
            ("text/html", "<script>alert(1)</script>", true),
            ("text/plain", "hello", true),
            ("application/xml", "<a/>", true),
            ("image/png", "PNG", false),
            ("application/octet-stream", "bytes", false),
            ("application/zip", "PK", false),
            ("audio/mpeg", "ID3", false),
        ] {
            let server = serve(move |_| {
                Reply::Whole(http("200 OK", &[("Content-Type", kind)], body.as_bytes()))
            })
            .await;
            let result = run(&runner(store()), get(server.port, "/x"), json!({})).await;
            assert_eq!(result.is_ok(), ok, "{kind}: {result:?}");
            if let Ok(out) = result {
                assert_eq!(out.text, body);
            }
        }
        // No content type: text is accepted, anything that is not UTF-8 is not.
        let plain = serve(|_| Reply::Whole(http("200 OK", &[], b"hi"))).await;
        assert_eq!(
            run(&runner(store()), get(plain.port, "/x"), json!({}))
                .await
                .unwrap()
                .text,
            "hi"
        );
        let binary = serve(|_| Reply::Whole(http("200 OK", &[], &[0xff, 0xfe, 0x00]))).await;
        assert!(run(&runner(store()), get(binary.port, "/x"), json!({}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn an_error_status_is_an_error_with_a_bounded_redacted_body() {
        let (secrets, auth) = with_token("bearer", json!({}));
        let server = serve(|request| {
            let mut body = request.head.clone().into_bytes();
            body.extend_from_slice(&vec![b'z'; 5000]);
            Reply::Whole(http(
                "500 Internal Server Error",
                &[("Content-Type", "text/plain")],
                &body,
            ))
        })
        .await;
        let mut binding = get(server.port, "/x");
        binding["auth"] = auth;
        let err = run(&runner(secrets), binding, json!({})).await.unwrap_err();
        assert!(
            err.0.starts_with("the server answered HTTP 500"),
            "{}",
            err.0
        );
        assert!(err.0.len() < ERROR_BODY_BYTES + 100);
        assert!(!err.0.contains(TOKEN));
    }

    // -- no proxy ------------------------------------------------------------------

    #[tokio::test]
    async fn the_proxy_environment_is_ignored() {
        let proxy = serve(|_| ok_json("{\"via\":\"proxy\"}")).await;
        let server = serve(|_| ok_json("{\"via\":\"direct\"}")).await;
        let proxy_url = format!("http://127.0.0.1:{}", proxy.port);
        for name in [
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
            "HTTPS_PROXY",
            "https_proxy",
        ] {
            std::env::set_var(name, &proxy_url);
        }
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
        // A name that is not loopback by address, so a proxy would be consulted.
        let resolver = FakeResolver::with("localhost", vec![vec![ip("127.0.0.1")]]);
        let r = RestRunner::with_parts(store(), |_| None, resolver);
        let binding =
            json!({"method": "GET", "url_template": format!("http://localhost:{}/x", server.port)});
        let out = run(&r, binding, json!({})).await.unwrap();
        assert_eq!(out.text, "{\"via\":\"direct\"}");
        assert_eq!(proxy.hits(), 0);
    }

    // -- the confirmation ----------------------------------------------------------

    #[tokio::test]
    async fn the_confirmation_shows_the_resolved_address_and_the_credential_before_anything_long() {
        let (secrets, auth) = with_token("bearer", json!({}));
        let resolver = FakeResolver::with("api.example", vec![vec![ip("8.8.8.8")]]);
        let r = RestRunner::with_parts(secrets, |_| None, resolver);
        let binding = json!({
            "method": "POST",
            "url_template": "https://api.example/items/{id}",
            "body": "json",
            "auth": auth,
        });
        let t = tool(binding);
        let shown = r
            .confirmation_text(&t, &json!({"id": "7", "title": "x".repeat(10_000)}))
            .await
            .unwrap();
        assert!(shown.starts_with("POST https://api.example"), "{shown}");
        assert!(shown.contains("(resolved to 8.8.8.8)"), "{shown}");
        assert!(
            shown.contains("credential (bearer) from keychain:tok"),
            "{shown}"
        );
        assert!(shown.contains("path and query: /items/7"), "{shown}");
        assert!(
            shown.ends_with('\u{2026}'),
            "the body was not cut: {}",
            &shown[shown.len() - 30..]
        );
        assert!(shown.len() <= MAX_DESTINATION_BYTES);
        assert!(!shown.contains(TOKEN));
        // A long path cannot push the credential out either.
        let long = tool(json!({
            "method": "GET",
            "url_template": "https://api.example/{id}",
            "auth": {"scheme": "header", "header_name": "X-Key", "secret": "env:KEY_VAR"},
        }));
        let shown = r
            .confirmation_text(&long, &json!({"id": "p".repeat(5000)}))
            .await
            .unwrap();
        assert!(
            shown.contains("credential in the X-Key header from env:KEY_VAR"),
            "{shown}"
        );
        assert!(shown.contains('\u{2026}') && shown.len() <= MAX_DESTINATION_BYTES);
        // A forbidden destination refuses before the user is asked.
        let blocked = FakeResolver::with("api.example", vec![vec![ip("10.0.0.5")]]);
        let r = RestRunner::with_parts(store(), |_| None, blocked);
        assert!(r.confirmation_text(&t, &json!({"id": "7"})).await.is_err());
    }

    // -- review fixes ---------------------------------------------------------

    /// A destination named loopback may be reached over plain http because it
    /// cannot leave the machine: a name that resolves anywhere else is refused,
    /// and nothing is sent (so no credential goes out in the clear).
    #[tokio::test]
    async fn a_loopback_named_destination_must_resolve_to_loopback() {
        let server = serve(|_| ok_json("{}")).await;
        for answers in [
            vec![ip("8.8.8.8")],
            vec![ip("127.0.0.1"), ip("8.8.8.8")],
            vec![ip("2606:4700:4700::1111")],
        ] {
            let resolver = FakeResolver::with("localhost", vec![answers]);
            let (secrets, auth) = with_token("bearer", json!({}));
            let r = RestRunner::with_parts(secrets, |_| None, resolver);
            let mut binding = json!({
                "method": "GET",
                "url_template": format!("http://localhost:{}/x", server.port),
            });
            binding["auth"] = auth;
            let err = run(&r, binding, json!({})).await.unwrap_err();
            assert!(
                err.0.contains("not a loopback address") || err.0.contains("not sent"),
                "{}",
                err.0
            );
        }
        assert_eq!(server.hits(), 0);
        assert!(pick_address("localhost", &[ip("8.8.8.8")], true).is_err());
        assert!(pick_address("localhost", &[ip("127.0.0.1"), ip("::1")], true).is_ok());
        assert!(pick_address("localhost", &[ip("::ffff:127.0.0.1")], true).is_ok());
    }

    /// Nothing from a response header is repeated, the credential in it included.
    #[tokio::test]
    async fn a_content_type_that_carries_the_credential_is_not_echoed() {
        let (secrets, auth) = with_token("bearer", json!({}));
        let server = serve(|_| {
            Reply::Whole(http(
                "200 OK",
                &[("Content-Type", &format!("x/{TOKEN}"))],
                b"data",
            ))
        })
        .await;
        let mut binding = get(server.port, "/x");
        binding["auth"] = auth;
        let err = run(&runner(secrets), binding, json!({})).await.unwrap_err();
        assert!(err.0.contains("not text or JSON"), "{}", err.0);
        assert!(!err.0.contains(TOKEN) && !err.0.contains("x/"), "{}", err.0);
    }

    struct Hanging;

    #[async_trait]
    impl HostResolver for Hanging {
        async fn resolve(&self, _host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
            std::future::pending().await
        }
    }

    /// A resolver that never answers does not hold the call past its timeout.
    #[tokio::test]
    async fn the_total_timeout_covers_the_name_lookup() {
        let r = RestRunner::with_parts(store(), |_| None, Arc::new(Hanging));
        let mut binding = json!({"method": "GET", "url_template": "https://api.example/x"});
        binding["timeout_seconds"] = json!(1);
        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(15), run(&r, binding, json!({})))
            .await
            .expect("the call outlived its timeout");
        let err = result.unwrap_err();
        assert!(
            err.0.contains("did not finish within 1 seconds"),
            "{}",
            err.0
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// ... and neither does it hold the question put to the user: a lookup that
    /// does not answer is a refused destination.
    #[tokio::test(start_paused = true)]
    async fn the_confirmation_does_not_wait_forever_for_a_name_lookup() {
        let r = RestRunner::with_parts(store(), |_| None, Arc::new(Hanging));
        let t = tool(
            json!({"method": "POST", "url_template": "https://api.example/x", "body": "json"}),
        );
        let result = tokio::time::timeout(
            Duration::from_secs(120),
            r.confirmation_text(&t, &json!({"title": "t"})),
        )
        .await
        .expect("the confirmation waited for a lookup that never answered");
        let err = result.unwrap_err();
        assert!(err.0.contains("could not be resolved in time"), "{}", err.0);
    }

    #[tokio::test]
    async fn the_run_uses_the_address_the_confirmation_resolved_not_a_second_lookup() {
        let server = serve(|_| ok_json("{}")).await;
        // The name answers loopback for the question and a private address after
        // (a rebinding): a second lookup would be refused, the pin is not.
        let resolver = FakeResolver::with(
            "localhost",
            vec![vec![ip("127.0.0.1")], vec![ip("10.0.0.5")]],
        );
        let r = RestRunner::with_parts(store(), |_| None, resolver.clone());
        let t = tool(json!({
            "method": "GET",
            "url_template": format!("http://localhost:{}/x", server.port),
        }));
        let confirmation = r.confirmation(&t, &json!({})).await.unwrap();
        let pin = confirmation.pin.expect("a resolving runner pins");
        assert_eq!(pin.address, ip("127.0.0.1"));
        let out = r.run_pinned(&t, &json!({}), &pin).await.unwrap();
        assert_eq!(out.text, "{}");
        assert_eq!(server.hits(), 1);
        // Only the question resolved the name.
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        // Without the pin, the rebound answer is what the run would have used.
        let e = r.run(&t, &json!({})).await.unwrap_err().0;
        assert!(!e.is_empty());
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn a_pin_for_another_host_is_not_a_resolution_for_this_one() {
        let server = serve(|_| ok_json("{}")).await;
        let resolver = FakeResolver::with("localhost", vec![vec![ip("127.0.0.1")]]);
        let r = RestRunner::with_parts(store(), |_| None, resolver);
        let t = tool(json!({
            "method": "GET",
            "url_template": format!("http://localhost:{}/x", server.port),
        }));
        let wrong = ResolvedPin {
            host: "other.example".into(),
            address: ip("127.0.0.1"),
        };
        assert!(r.run_pinned(&t, &json!({}), &wrong).await.is_err());
        assert_eq!(server.hits(), 0);
    }
}
