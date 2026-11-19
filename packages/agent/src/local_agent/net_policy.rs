//! The network rules the HTTP-facing runners share: which addresses a request
//! may go to (checked after name resolution), the destination rules for the
//! scheme and the host, the client that is pinned to the address that was
//! checked (no proxy, no redirects, no cookies, no content decoding), and the
//! credential header a binding's `auth` describes.
//!
//! The rules and their reasons are described on the runners that use them
//! ([`super::rest_runner`], [`super::mcp_runner`]); the tests of the address
//! policy sit with the REST runner's.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nodespace_core::secrets::{resolve_credential_with, SecretStore};
use nodespace_types::{CredentialRef, HttpAuth};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use reqwest::Method;
use url::{Host, Url};
use zeroize::Zeroizing;

use super::process::EnvLookup;
use super::redaction::{cut, sanitize, MIN_REDACTABLE_BYTES};
use super::remote_tools::RemoteCallError;

/// Longest connect phase.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest credential value accepted as a header value.
const MAX_CREDENTIAL_BYTES: usize = 8 * 1024;

pub(crate) fn fail<T>(message: impl Into<String>) -> Result<T, RemoteCallError> {
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
    // 192.0.0.0/24 (IETF protocol assignments, 192.0.0.8 among them) and the
    // deprecated 6to4 relay anycast range.
    if (a == 192 && b == 0 && c == 0) || (a == 192 && b == 88 && c == 99) {
        return Some("a reserved address");
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
    // Transition and special-purpose ranges that carry or hide an IPv4 address,
    // or are not for the public internet: 6to4 (2002::/16), Teredo (2001::/32),
    // discard-only (100::/64), 3fff::/20, 64:ff9b:1::/48, 5f00::/16.
    if first == 0x2002 {
        return Some("a 6to4 address");
    }
    if first == 0x2001 && segments[1] == 0 {
        return Some("a Teredo address");
    }
    if first == 0x0100 && segments[1..4] == [0, 0, 0] {
        return Some("a discard-only address");
    }
    if first == 0x3fff && segments[1] & 0xf000 == 0 {
        return Some("a documentation address");
    }
    if first == 0x0064 && segments[1] == 0xff9b && segments[2] == 1 {
        return Some("a local-use NAT64 address");
    }
    if first == 0x5f00 {
        return Some("a reserved address");
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
        // A destination the binding names as loopback must stay on this machine:
        // plain http to it is allowed because it cannot leave, so a name that
        // resolves anywhere else is refused.
        if loopback_ok && !is_loopback_address(*ip) {
            return fail(format!(
                "`{host}` resolves to {ip}, which is not a loopback address: the request was not sent"
            ));
        }
    }
    Ok(addresses[0])
}

/// Whether `ip` is a loopback address (`::ffff:127.x` included).
fn is_loopback_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
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
pub(crate) struct Destination {
    pub(crate) host: String,
    pub(crate) port: u16,
    /// Whether the destination is loopback by what the URL says.
    pub(crate) loopback: bool,
}

/// `https` always; `http` only to a loopback destination; no user information.
pub(crate) fn destination(url: &Url) -> Result<Destination, RemoteCallError> {
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

pub(crate) fn base64(input: &[u8]) -> String {
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
pub(crate) fn auth_header(
    auth: &HttpAuth,
    value: &str,
) -> Result<(HeaderName, HeaderValue), RemoteCallError> {
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

/// The origin a request goes to: scheme, host and port.
pub(crate) fn origin(url: &Url) -> (String, Option<String>, Option<u16>) {
    (
        url.scheme().to_string(),
        url.host_str().map(str::to_ascii_lowercase),
        url.port_or_known_default(),
    )
}

/// An HTTP client failure as a message that carries no URL.
pub(crate) fn request_error(
    error: reqwest::Error,
    limit: Duration,
    needles: &[Zeroizing<String>],
) -> RemoteCallError {
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
    // The text may carry something the server sent: redact it and keep it short
    // and plain.
    let mut detail = sanitize(error.without_url().to_string().as_bytes(), needles, false);
    detail.retain(|c| !c.is_control());
    cut(&mut detail, 200);
    RemoteCallError(format!("{kind} ({detail})"))
}

/// The credential a binding's `auth` describes, ready to send: the header, and
/// every value the server could echo back (to be redacted).
pub(crate) struct HttpCredential {
    pub name: HeaderName,
    pub value: HeaderValue,
    pub needles: Vec<Zeroizing<String>>,
}

/// Resolves the credential `auth` names, or `None` when it names none. Runs the
/// platform store off the async threads; the value is read only here.
pub(crate) async fn http_credential(
    secrets: &Arc<SecretStore>,
    lookup: &EnvLookup,
    auth: Option<&HttpAuth>,
) -> Result<Option<HttpCredential>, RemoteCallError> {
    let Some(auth) = auth else {
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
    let secrets = Arc::clone(secrets);
    let lookup = Arc::clone(lookup);
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
    Ok(Some(HttpCredential {
        name,
        value: header,
        needles,
    }))
}

/// Resolves and checks a destination, returning the address to connect to.
pub(crate) async fn checked_address(
    resolver: &dyn HostResolver,
    destination: &Destination,
) -> Result<IpAddr, RemoteCallError> {
    // An address in the URL needs no lookup, but the same rules apply to it.
    if let Ok(ip) = destination.host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return pick_address(&destination.host, &[ip], destination.loopback);
    }
    let addresses = resolver
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

/// A client for one hop: no proxy, no redirects (they are followed by hand, one
/// origin only), no cookie store, no content decoding, and a resolver that
/// answers only with the address that was checked, so the connection goes there
/// with the original host kept for `Host` and TLS.
pub(crate) fn pinned_client(
    ip: IpAddr,
    port: u16,
    lookups: &Arc<AtomicUsize>,
    limit: Duration,
) -> Result<reqwest::Client, RemoteCallError> {
    let pinned = Arc::new(PinnedResolver {
        address: SocketAddr::new(ip, port),
        lookups: Arc::clone(lookups),
    });
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(pinned)
        .connect_timeout(CONNECT_TIMEOUT.min(limit))
        .timeout(limit)
        .user_agent("nodespace-agent")
        .build()
        .map_err(|_| RemoteCallError("the HTTP client could not be built".to_string()))
}

/// Most redirects followed.
const MAX_REDIRECTS: usize = 3;

/// What `send_following` needs besides the request.
pub(crate) struct Outgoing<'a> {
    pub resolver: &'a dyn HostResolver,
    pub lookups: &'a Arc<AtomicUsize>,
    pub limit: Duration,
    /// Credential values to redact from error text.
    pub needles: &'a [Zeroizing<String>],
}

/// Sends one request and returns the response that is not a redirect, applying
/// the destination rules on every hop: the scheme and host rules, the name
/// resolved and every address checked, the connection pinned to the checked
/// address. Redirects are followed only within the origin the request started
/// at (so no credential or header reaches another origin), at most three; a
/// redirect that would change a non-`GET` method is refused.
pub(crate) async fn send_following(
    ctx: &Outgoing<'_>,
    method: Method,
    url: Url,
    headers: &HeaderMap,
    body: Option<&[u8]>,
) -> Result<reqwest::Response, RemoteCallError> {
    let first_origin = origin(&url);
    let mut url = url;
    for hop in 0..=MAX_REDIRECTS {
        let target = destination(&url)?;
        let ip = checked_address(ctx.resolver, &target).await?;
        let client = pinned_client(ip, target.port, ctx.lookups, ctx.limit)?;
        let mut request = client
            .request(method.clone(), url.clone())
            .headers(headers.clone());
        if let Some(bytes) = body {
            request = request.body(bytes.to_vec());
        }
        let response = request
            .send()
            .await
            .map_err(|e| request_error(e, ctx.limit, ctx.needles))?;
        let status = response.status();
        if !status.is_redirection() {
            return Ok(response);
        }
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
        let next = url
            .join(&location)
            .map_err(|_| RemoteCallError("the server sent an unusable redirect".to_string()))?;
        if origin(&next) != first_origin || !next.username().is_empty() || next.password().is_some()
        {
            return fail("the server redirected to a different origin; it was not followed");
        }
        let keeps_method = matches!(status.as_u16(), 307 | 308);
        if !keeps_method && method != Method::GET {
            return fail("the server redirected a request that is not a GET; it was not followed");
        }
        url = next;
    }
    fail("the server redirected too many times")
}

/// Longest credential reference shown.
const MAX_REFERENCE_SHOWN: usize = 300;

/// How a confirmation says what credential a request carries (bounded, and
/// meant to come before anything long).
pub(crate) fn credential_note(auth: Option<&HttpAuth>) -> String {
    match auth {
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
            line
        }
        _ => "; no credential".to_string(),
    }
}
