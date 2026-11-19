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
//! Limits: redaction replaces exact values, so a transformed form of a
//! credential in a body is not caught; the credential is held in the
//! `HeaderValue` the HTTP stack keeps while the request is in flight; one
//! resolver answer is used for a hop, so a name that points at a different
//! public address later is followed on the next hop only.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nodespace_core::secrets::{resolve_credential_with, SecretStore};
use nodespace_types::{
    CredentialRef, HttpAuth, ToolRestFields, DEFAULT_RESPONSE_BYTES, DEFAULT_TIMEOUT_SECONDS,
};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Method;
use serde_json::{Map, Value};
use url::{Host, Url};
use zeroize::Zeroizing;

use super::redaction::{cut, sanitize, MIN_REDACTABLE_BYTES};
use super::remote_tools::{
    RemoteCallError, RemoteOutput, RemoteTool, RemoteToolKind, RemoteToolRunner,
};

/// Characters left as they are in a placeholder value: the URL's unreserved set.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
/// Most redirects followed.
const MAX_REDIRECTS: usize = 3;
/// Largest request body sent, in bytes.
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
/// Longest credential value accepted as a header value.
const MAX_CREDENTIAL_BYTES: usize = 8 * 1024;
/// How much of a failing response an error message carries.
const ERROR_BODY_BYTES: usize = 1500;
/// Longest connect phase.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest confirmation text.
const MAX_DESTINATION_BYTES: usize = 4096;
/// Longest path-and-query part of the confirmation text.
const MAX_PATH_SHOWN: usize = 1500;
/// Longest credential reference shown.
const MAX_REFERENCE_SHOWN: usize = 300;

type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

fn fail<T>(message: impl Into<String>) -> Result<T, RemoteCallError> {
    Err(RemoteCallError(message.into()))
}

// ---------------------------------------------------------------------------
// Resolving and the address policy
// ---------------------------------------------------------------------------

/// Resolves a host name to addresses. Replaceable, so tests can answer without
/// DNS.
#[async_trait]
pub trait HostResolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>>;
}

/// The operating system's resolver.
pub struct SystemResolver;

#[async_trait]
impl HostResolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
        Ok(tokio::net::lookup_host((host, port))
            .await?
            .map(|address| address.ip())
            .collect())
    }
}

/// Why `ip` may not be connected to, or `None` when it may. `loopback_ok` is
/// true only for a destination the binding names as loopback (`localhost` or a
/// loopback address): a name that merely resolves to a loopback address is not.
pub(crate) fn blocked_reason(ip: IpAddr, loopback_ok: bool) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4, loopback_ok),
        IpAddr::V6(v6) => blocked_v6(v6, loopback_ok),
    }
}

fn blocked_v4(ip: Ipv4Addr, loopback_ok: bool) -> Option<&'static str> {
    let [a, b, c, _] = ip.octets();
    if ip.is_loopback() {
        return (!loopback_ok).then_some("a loopback address");
    }
    if ip.is_unspecified() || a == 0 {
        return Some("an unspecified address");
    }
    if ip.is_private() {
        return Some("a private address");
    }
    if ip.is_link_local() {
        return Some("a link-local address");
    }
    if ip.is_broadcast() || ip.is_multicast() || a >= 240 {
        return Some("a broadcast, multicast or reserved address");
    }
    if a == 100 && (64..=127).contains(&b) {
        return Some("a shared (carrier-grade NAT) address");
    }
    if (a == 192 && b == 0 && c == 2)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
    {
        return Some("a documentation address");
    }
    if a == 198 && (b == 18 || b == 19) {
        return Some("a benchmarking address");
    }
    None
}

fn blocked_v6(ip: Ipv6Addr, loopback_ok: bool) -> Option<&'static str> {
    if ip.is_loopback() {
        return (!loopback_ok).then_some("a loopback address");
    }
    if ip.is_unspecified() {
        return Some("an unspecified address");
    }
    // An IPv4 address carried inside an IPv6 one is judged as the IPv4 address.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return blocked_v4(v4, loopback_ok);
    }
    let segments = ip.segments();
    // NAT64 (64:ff9b::/96) embeds an IPv4 address in the last 32 bits.
    if segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2..6] == [0, 0, 0, 0] {
        let v4 = Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            segments[6] as u8,
            (segments[7] >> 8) as u8,
            segments[7] as u8,
        );
        return blocked_v4(v4, loopback_ok).or(Some("a translated (NAT64) address"));
    }
    // IPv4-compatible (::/96) is deprecated and ambiguous.
    if segments[..6] == [0, 0, 0, 0, 0, 0] {
        return Some("an IPv4-compatible address");
    }
    if ip.is_multicast() {
        return Some("a multicast address");
    }
    let first = segments[0];
    if first & 0xfe00 == 0xfc00 {
        return Some("a unique-local address");
    }
    if first & 0xffc0 == 0xfe80 {
        return Some("a link-local address");
    }
    if first & 0xffc0 == 0xfec0 {
        return Some("a site-local address");
    }
    if first == 0x2001 && segments[1] == 0x0db8 {
        return Some("a documentation address");
    }
    None
}

/// Checks every resolved address and returns the one to connect to (the
/// first). One refused address refuses the call, so a mixed answer cannot be
/// used to slip a private address in.
pub(crate) fn pick_address(
    host: &str,
    addresses: &[IpAddr],
    loopback_ok: bool,
) -> Result<IpAddr, RemoteCallError> {
    if addresses.is_empty() {
        return fail(format!("`{host}` resolved to no address"));
    }
    for ip in addresses {
        if let Some(reason) = blocked_reason(*ip, loopback_ok) {
            return fail(format!(
                "`{host}` resolves to {ip}, {reason}: the request was not sent"
            ));
        }
    }
    Ok(addresses[0])
}

/// A destination the binding names as loopback.
fn is_loopback_host(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.trim_end_matches('.').eq_ignore_ascii_case("localhost"),
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// The scheme, host and port a request may go to.
struct Destination {
    host: String,
    port: u16,
    /// Whether the destination is loopback by what the URL says.
    loopback: bool,
}

/// `https` always; `http` only to a loopback destination; no user information.
fn destination(url: &Url) -> Result<Destination, RemoteCallError> {
    let Some(host) = url.host() else {
        return fail("the URL has no host");
    };
    if !url.username().is_empty() || url.password().is_some() {
        return fail("the URL carries credentials; they are never sent that way");
    }
    let loopback = is_loopback_host(&host);
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => return fail("the request must use https (http only to a loopback host)"),
    }
    let Some(port) = url.port_or_known_default() else {
        return fail("the URL has no usable port");
    };
    Ok(Destination {
        host: url.host_str().unwrap_or_default().to_string(),
        port,
        loopback,
    })
}

/// Answers every lookup with the one address that was checked.
struct PinnedResolver {
    address: SocketAddr,
    lookups: Arc<AtomicUsize>,
}

impl reqwest::dns::Resolve for PinnedResolver {
    fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        let address = self.address;
        Box::pin(async move {
            let addrs: reqwest::dns::Addrs = Box::new(std::iter::once(address));
            Ok(addrs)
        })
    }
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

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The credential header for `auth` and the resolved `value`.
fn auth_header(auth: &HttpAuth, value: &str) -> Result<(HeaderName, HeaderValue), RemoteCallError> {
    if value.len() > MAX_CREDENTIAL_BYTES || value.chars().any(char::is_control) {
        return fail("the credential cannot be sent as a header value");
    }
    let (name, text) = match auth.scheme.as_str() {
        "bearer" => (AUTHORIZATION, format!("Bearer {value}")),
        "basic" => (AUTHORIZATION, format!("Basic {}", base64(value.as_bytes()))),
        "header" => {
            let name = auth.header_name.as_deref().ok_or_else(|| {
                RemoteCallError("the binding names no credential header".to_string())
            })?;
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                RemoteCallError("the credential header name is not valid".to_string())
            })?;
            (name, value.to_string())
        }
        _ => return fail("the binding's auth scheme sends no credential"),
    };
    let mut header = HeaderValue::from_str(&text).map_err(|_| {
        RemoteCallError("the credential cannot be sent as a header value".to_string())
    })?;
    header.set_sensitive(true);
    Ok((name, header))
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

/// The origin a request goes to: scheme, host and port.
fn origin(url: &Url) -> (String, Option<String>, Option<u16>) {
    (
        url.scheme().to_string(),
        url.host_str().map(str::to_ascii_lowercase),
        url.port_or_known_default(),
    )
}

impl RestRunner {
    /// Resolves the credential the binding names: the header and the value to
    /// redact.
    async fn credential(
        &self,
        fields: &ToolRestFields,
    ) -> Result<Option<(HeaderName, HeaderValue, Vec<Zeroizing<String>>)>, RemoteCallError> {
        let Some(auth) = &fields.auth else {
            return Ok(None);
        };
        if auth.scheme == "none" {
            return Ok(None);
        }
        let secret = auth
            .secret
            .as_deref()
            .ok_or_else(|| RemoteCallError("the binding names no credential".to_string()))?;
        let reference = CredentialRef::parse(secret)
            .map_err(|e| RemoteCallError(format!("the tool's binding is not valid: {}", e.0)))?;
        let secrets = Arc::clone(&self.secrets);
        let lookup = Arc::clone(&self.daemon_env);
        let value = tokio::task::spawn_blocking(move || {
            resolve_credential_with(&secrets, &reference, |n| lookup(n))
        })
        .await
        .map_err(|_| RemoteCallError("the credential store task failed".to_string()))?
        .map_err(|e| RemoteCallError(e.to_string()))?;
        // The one place the value is read: it goes into the request header.
        let text = Zeroizing::new(value.expose().to_string());
        if text.len() < MIN_REDACTABLE_BYTES {
            return fail("the credential is too short to be redacted safely");
        }
        let (name, header) = auth_header(auth, &text)?;
        // What the server could echo back: the value, and for `basic` its encoding.
        let mut needles = vec![text.clone()];
        if auth.scheme == "basic" {
            needles.push(Zeroizing::new(base64(text.as_bytes())));
        }
        Ok(Some((name, header, needles)))
    }

    /// Resolves and checks a destination, returning the address to connect to.
    async fn checked_address(&self, destination: &Destination) -> Result<IpAddr, RemoteCallError> {
        // An address in the URL needs no lookup, but the same rules apply to it.
        if let Ok(ip) = destination.host.trim_matches(['[', ']']).parse::<IpAddr>() {
            return pick_address(&destination.host, &[ip], destination.loopback);
        }
        let addresses = self
            .resolver
            .resolve(&destination.host, destination.port)
            .await
            .map_err(|e| {
                RemoteCallError(format!(
                    "`{}` could not be resolved: {}",
                    destination.host,
                    e.kind()
                ))
            })?;
        pick_address(&destination.host, &addresses, destination.loopback)
    }

    /// One request, with the redirects the policy allows.
    async fn exchange(
        &self,
        plan: Plan,
        headers: HeaderMap,
        credential: &Option<(HeaderName, HeaderValue, Vec<Zeroizing<String>>)>,
        needles: &[Zeroizing<String>],
        limit: Duration,
        cap: usize,
    ) -> Result<RemoteOutput, RemoteCallError> {
        let first_origin = origin(&plan.url);
        let mut url = plan.url;
        let method = plan.method;
        let body = plan.body;
        for hop in 0..=MAX_REDIRECTS {
            let destination = destination(&url)?;
            let ip = self.checked_address(&destination).await?;
            let pinned = Arc::new(PinnedResolver {
                address: SocketAddr::new(ip, destination.port),
                lookups: Arc::clone(&self.pinned_lookups),
            });
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .dns_resolver(pinned)
                .connect_timeout(CONNECT_TIMEOUT.min(limit))
                .timeout(limit)
                .user_agent("nodespace-agent")
                .build()
                .map_err(|_| RemoteCallError("the HTTP client could not be built".to_string()))?;
            let mut request = client
                .request(method.clone(), url.clone())
                .headers(headers.clone());
            if let Some((name, value, _)) = credential {
                request = request.header(name.clone(), value.clone());
            }
            if let Some(bytes) = &body {
                request = request
                    .header(CONTENT_TYPE, "application/json")
                    .body(bytes.clone());
            }
            let mut response = request.send().await.map_err(|e| request_error(e, limit))?;
            let status = response.status();

            if status.is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let Some(location) = location else {
                    return fail(format!(
                        "the server answered HTTP {status} with no Location"
                    ));
                };
                if hop == MAX_REDIRECTS {
                    return fail("the server redirected too many times");
                }
                let next = url.join(&location).map_err(|_| {
                    RemoteCallError("the server sent an unusable redirect".to_string())
                })?;
                if origin(&next) != first_origin
                    || !next.username().is_empty()
                    || next.password().is_some()
                {
                    return fail(
                        "the server redirected to a different origin; it was not followed",
                    );
                }
                let keeps_method = matches!(status.as_u16(), 307 | 308);
                if !keeps_method && method != Method::GET {
                    return fail(
                        "the server redirected a request that is not a GET; it was not followed",
                    );
                }
                url = next;
                continue;
            }

            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            if let Some(kind) = &content_type {
                if !is_text_type(kind) {
                    return fail(format!(
                        "the server answered with `{}`, which is not returned (text and JSON only)",
                        kind.chars().take(80).collect::<String>()
                    ));
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
                    Err(e) => return Err(request_error(e, limit)),
                }
            }
            if content_type.is_none() && std::str::from_utf8(&bytes).is_err() && !stopped_at_cap {
                return fail("the server answered without a content type and with non-text data");
            }
            let mut text = sanitize(&bytes, needles, stopped_at_cap);
            let truncated = stopped_at_cap || text.len() > cap;
            cut(&mut text, cap);
            return if status.is_success() {
                Ok(RemoteOutput { text, truncated })
            } else {
                cut(&mut text, ERROR_BODY_BYTES);
                fail(format!("the server answered HTTP {status}: {text}"))
            };
        }
        fail("the server redirected too many times")
    }
}

/// An HTTP client failure as a message that carries no URL.
fn request_error(error: reqwest::Error, limit: Duration) -> RemoteCallError {
    if error.is_timeout() {
        return RemoteCallError(format!(
            "the request did not finish within {} seconds",
            limit.as_secs()
        ));
    }
    let kind = if error.is_connect() {
        "could not connect"
    } else if error.is_body() || error.is_decode() {
        "the response could not be read"
    } else {
        "the request failed"
    };
    RemoteCallError(format!("{kind} ({})", error.without_url()))
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
    match &fields.auth {
        Some(auth) if auth.scheme != "none" => {
            let reference = auth.secret.as_deref().unwrap_or("?");
            let mut line = match auth.scheme.as_str() {
                "header" => format!(
                    "; credential in the {} header from {reference}",
                    auth.header_name.as_deref().unwrap_or("?")
                ),
                other => format!("; credential ({other}) from {reference}"),
            };
            cut(&mut line, MAX_REFERENCE_SHOWN);
            shown.push_str(&line);
        }
        _ => shown.push_str("; no credential"),
    }
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
        let fields = read_binding(tool)?;
        let plan = plan(&fields, args)?;
        let destination = destination(&plan.url)?;
        let ip = self.checked_address(&destination).await?;
        Ok(describe(&fields, &plan, Some(&ip.to_string())))
    }

    async fn run(&self, tool: &RemoteTool, args: &Value) -> Result<RemoteOutput, RemoteCallError> {
        let fields = read_binding(tool)?;
        let plan = plan(&fields, args)?;
        // Scheme and host rules before anything is resolved or read.
        destination(&plan.url)?;
        let credential = self.credential(&fields).await?;
        if credential.is_some() {
            // A credential goes over https, or over http only to a loopback host.
            let d = destination(&plan.url)?;
            if plan.url.scheme() != "https" && !d.loopback {
                return fail("a credential is only sent over https");
            }
        }
        let needles: Vec<Zeroizing<String>> = credential
            .iter()
            .flat_map(|(_, _, values)| values.iter().cloned())
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
        let limit = Duration::from_secs(fields.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS));
        let cap = fields.max_response_bytes.unwrap_or(DEFAULT_RESPONSE_BYTES) as usize;
        match tokio::time::timeout(
            limit,
            self.exchange(plan, headers, &credential, &needles, limit, cap),
        )
        .await
        {
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
    use nodespace_core::secrets::InMemorySecretBackend;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // -- a local HTTP server ----------------------------------------------

    #[derive(Debug, Clone)]
    struct Request {
        head: String,
        body: Vec<u8>,
    }

    enum Reply {
        Whole(Vec<u8>),
        /// The head, then `chunk` every `every`, until the client goes away.
        Stream {
            head: Vec<u8>,
            chunk: Vec<u8>,
            every: Duration,
        },
    }

    fn http(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str("\r\n");
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    fn ok_json(body: &str) -> Reply {
        Reply::Whole(http(
            "200 OK",
            &[("Content-Type", "application/json")],
            body.as_bytes(),
        ))
    }

    struct Server {
        port: u16,
        seen: Arc<Mutex<Vec<Request>>>,
        /// Set when a streaming client went away.
        closed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Server {
        fn hits(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
        fn requests(&self) -> Vec<Request> {
            self.seen.lock().unwrap().clone()
        }
    }

    async fn serve(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Request>>> = Arc::default();
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handler = Arc::new(handler);
        let (seen_in, closed_in) = (Arc::clone(&seen), Arc::clone(&closed));
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let (handler, seen, closed) = (
                    Arc::clone(&handler),
                    Arc::clone(&seen_in),
                    Arc::clone(&closed_in),
                );
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let head_end = loop {
                        let n = stream.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buffer.extend_from_slice(&chunk[..n]);
                        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                            break at + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
                    let length = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    while buffer.len() < head_end + length {
                        let n = stream.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buffer.extend_from_slice(&chunk[..n]);
                    }
                    let request = Request {
                        head,
                        body: buffer[head_end..].to_vec(),
                    };
                    let reply = handler(&request);
                    seen.lock().unwrap().push(request);
                    match reply {
                        Reply::Whole(bytes) => {
                            let _ = stream.write_all(&bytes).await;
                            let _ = stream.shutdown().await;
                        }
                        Reply::Stream { head, chunk, every } => {
                            if stream.write_all(&head).await.is_err() {
                                closed.store(true, Ordering::SeqCst);
                                return;
                            }
                            loop {
                                if stream.write_all(&chunk).await.is_err()
                                    || stream.flush().await.is_err()
                                {
                                    closed.store(true, Ordering::SeqCst);
                                    return;
                                }
                                tokio::time::sleep(every).await;
                            }
                        }
                    }
                });
            }
        });
        Server { port, seen, closed }
    }

    /// A resolver with scripted answers; counts its calls.
    #[derive(Default)]
    struct FakeResolver {
        answers: Mutex<HashMap<String, Vec<Vec<IpAddr>>>>,
        calls: AtomicUsize,
    }

    impl FakeResolver {
        fn with(host: &str, answers: Vec<Vec<IpAddr>>) -> Arc<Self> {
            let r = Self::default();
            r.answers.lock().unwrap().insert(host.to_string(), answers);
            Arc::new(r)
        }
    }

    #[async_trait]
    impl HostResolver for FakeResolver {
        async fn resolve(&self, host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut answers = self.answers.lock().unwrap();
            let list = answers
                .get_mut(host)
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such host"))?;
            Ok(if list.len() > 1 {
                list.remove(0)
            } else {
                list[0].clone()
            })
        }
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

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
}
