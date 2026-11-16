//! Click-to-load for one remote image in an AI chat reply.
//!
//! The webview never loads a remote image on its own: the content security
//! policy keeps `img-src` to the app itself, `asset:` and `data:`, and the chat
//! sanitiser drops every network source. A reply that carries an image shows a
//! placeholder with the address instead. When the person presses "Load image",
//! the frontend sends that one address here, this module fetches it from the
//! app process, and the frontend shows the bytes as a `data:` image, which the
//! policy already allows. No policy or capability changes, and nothing is
//! requested before the click.
//!
//! The address comes from model output, so it is treated as hostile:
//!
//! - `https` only, no credentials in the address, the default port only.
//! - The host must resolve, and every address it resolves to must be a public
//!   one (no loopback, private, link-local, carrier-grade NAT, documentation,
//!   multicast or reserved range, and no IPv6 form that embeds one of those).
//!   The connection is pinned to the addresses that were checked, so the name
//!   cannot resolve differently between the check and the connect.
//! - Redirects are followed by hand, at most three, and every hop is checked
//!   the same way. No proxy is used, because a proxy would resolve the name
//!   itself and sidestep the check.
//! - No cookies, no authentication, no referrer, a fixed user agent.
//! - The status must be 200, the declared type one of PNG, JPEG, GIF or WebP
//!   (never SVG: it can carry script), the body at most 5 MiB, and the first
//!   bytes must match the declared type.
//!
//! A refusal is reported as a short code, never as the underlying error, so
//! the reply cannot use the message to probe the network.

use base64::Engine as _;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use reqwest::redirect::Policy as RedirectPolicy;
use reqwest::{Client, Url};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// Longest address accepted.
const MAX_URL_LEN: usize = 2048;
/// Largest body accepted.
const MAX_BYTES: usize = 5 * 1024 * 1024;
/// Redirect hops followed before the load is refused.
const MAX_REDIRECTS: usize = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
const USER_AGENT: &str = "nodespace-app";
const ACCEPT_IMAGES: &str = "image/png,image/jpeg,image/gif,image/webp";

/// The loaded image, ready for the frontend to put in a `data:` URL.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RemoteImage {
    pub mime: String,
    pub data_base64: String,
}

/// Why a load was refused. The code is the only thing the frontend sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    InvalidUrl,
    NotHttps,
    BlockedAddress,
    TooManyRedirects,
    BadStatus,
    NotAnImage,
    TooLarge,
    Network,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::InvalidUrl => "invalid_url",
            Refusal::NotHttps => "not_https",
            Refusal::BlockedAddress => "blocked_address",
            Refusal::TooManyRedirects => "too_many_redirects",
            Refusal::BadStatus => "bad_status",
            Refusal::NotAnImage => "not_an_image",
            Refusal::TooLarge => "too_large",
            Refusal::Network => "network",
        }
    }
}

/// What a fetch may reach. Production uses [`Policy::STRICT`]; the loosened
/// policy exists only in tests, so a local test server can stand in for the
/// network.
#[derive(Debug, Clone, Copy)]
struct Policy {
    allow_private_addresses: bool,
    allow_http: bool,
    allow_any_port: bool,
}

impl Policy {
    const STRICT: Policy = Policy {
        allow_private_addresses: false,
        allow_http: false,
        allow_any_port: false,
    };
}

/// True when the address is one a public host can have. Everything that names
/// this machine, the local network, or a reserved or special-use range is
/// refused, including the IPv6 forms that carry an IPv4 address.
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || o[0] == 0
        // 100.64.0.0/10, carrier-grade NAT
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
        // 192.0.0.0/24, IETF protocol assignments
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        // 198.18.0.0/15, benchmarking
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        // 240.0.0.0/4, reserved
        || o[0] >= 240)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    // ::ffff:a.b.c.d and ::a.b.c.d answer as the IPv4 address they carry.
    if let Some(v4) = ip.to_ipv4() {
        return is_public_v4(v4);
    }
    let s = ip.segments();
    // 64:ff9b::/96 (NAT64) and 2002::/16 (6to4) carry an IPv4 address too.
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return is_public_v4(v4);
    }
    if s[0] == 0x2002 {
        let v4 = Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8);
        return is_public_v4(v4);
    }
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        // fc00::/7, unique local
        || (s[0] & 0xfe00) == 0xfc00
        // fe80::/10, link-local, and fec0::/10, site-local
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] & 0xffc0) == 0xfec0
        // 2001:db8::/32, documentation
        || (s[0] == 0x2001 && s[1] == 0x0db8))
}

/// Parses and vets an address before any network access. Only the shape of
/// the address is checked here; where the host resolves is checked by
/// [`resolve`].
fn validate_url(raw: &str, policy: &Policy) -> Result<Url, Refusal> {
    if raw.is_empty() || raw.len() > MAX_URL_LEN {
        return Err(Refusal::InvalidUrl);
    }
    let url = Url::parse(raw).map_err(|_| Refusal::InvalidUrl)?;
    match url.scheme() {
        "https" => {}
        "http" if policy.allow_http => {}
        _ => return Err(Refusal::NotHttps),
    }
    if !url.username().is_empty() || url.password().is_some() || url.host().is_none() {
        return Err(Refusal::InvalidUrl);
    }
    if !policy.allow_any_port && url.port().is_some_and(|p| p != 443) {
        return Err(Refusal::InvalidUrl);
    }
    Ok(url)
}

/// Resolves the host and returns the addresses to connect to. Every address
/// must be public: one private answer among public ones refuses the load.
async fn resolve(url: &Url, policy: &Policy) -> Result<Vec<SocketAddr>, Refusal> {
    let port = url.port_or_known_default().ok_or(Refusal::InvalidUrl)?;
    let addrs: Vec<SocketAddr> = match url.host() {
        Some(url::Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        Some(url::Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        Some(url::Host::Domain(name)) => tokio::net::lookup_host((name, port))
            .await
            .map_err(|_| Refusal::Network)?
            .collect(),
        None => return Err(Refusal::InvalidUrl),
    };
    if addrs.is_empty() {
        return Err(Refusal::Network);
    }
    if !policy.allow_private_addresses && addrs.iter().any(|a| !is_public_ip(a.ip())) {
        return Err(Refusal::BlockedAddress);
    }
    Ok(addrs)
}

/// Resolves a redirect target against the page it came from and vets it like
/// a first address.
fn next_hop(current: &Url, location: &str, policy: &Policy) -> Result<Url, Refusal> {
    let target = current.join(location).map_err(|_| Refusal::InvalidUrl)?;
    validate_url(target.as_str(), policy)
}

/// The image types that are loaded, with the bytes each one starts with.
fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The declared media type without parameters, lower-cased, when it is one of
/// the four that are loaded.
fn allowed_declared_type(header: &str) -> Option<&'static str> {
    let essence = header.split(';').next()?.trim().to_ascii_lowercase();
    match essence.as_str() {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

fn build_client(url: &Url, addrs: &[SocketAddr], policy: &Policy) -> Result<Client, Refusal> {
    let mut builder = Client::builder()
        .redirect(RedirectPolicy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .user_agent(USER_AGENT)
        // A proxy resolves the name itself and would sidestep the address check.
        .no_proxy()
        .https_only(!policy.allow_http);
    if let Some(url::Host::Domain(name)) = url.host() {
        // Connect only to the addresses that were checked.
        builder = builder.resolve_to_addrs(name, addrs);
    }
    builder.build().map_err(|_| Refusal::Network)
}

async fn fetch_image(raw: &str, policy: &Policy) -> Result<RemoteImage, Refusal> {
    let mut url = validate_url(raw, policy)?;
    for hop in 0..=MAX_REDIRECTS {
        let addrs = resolve(&url, policy).await?;
        let client = build_client(&url, &addrs, policy)?;
        let mut resp = client
            .get(url.clone())
            .header(ACCEPT, ACCEPT_IMAGES)
            .send()
            .await
            .map_err(|_| Refusal::Network)?;

        let status = resp.status();
        if status.is_redirection() {
            if hop == MAX_REDIRECTS {
                return Err(Refusal::TooManyRedirects);
            }
            let location = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(Refusal::BadStatus)?;
            url = next_hop(&url, location, policy)?;
            continue;
        }
        if status.as_u16() != 200 {
            return Err(Refusal::BadStatus);
        }

        let declared = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(allowed_declared_type)
            .ok_or(Refusal::NotAnImage)?;
        let announced = resp
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        if announced.is_some_and(|n| n > MAX_BYTES) {
            return Err(Refusal::TooLarge);
        }

        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|_| Refusal::Network)? {
            if body.len() + chunk.len() > MAX_BYTES {
                return Err(Refusal::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }

        // The first bytes must agree with the declared type: a server that
        // calls an HTML page or an SVG "image/png" is refused.
        if sniff_image(&body) != Some(declared) {
            return Err(Refusal::NotAnImage);
        }
        return Ok(RemoteImage {
            mime: declared.to_string(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&body),
        });
    }
    Err(Refusal::TooManyRedirects)
}

/// Loads one remote image the person asked for. Called only from the
/// "Load image" button of a chat reply's placeholder.
#[tauri::command]
pub async fn fetch_remote_image(url: String) -> Result<RemoteImage, String> {
    fetch_image(&url, &Policy::STRICT)
        .await
        .map_err(|refusal| refusal.code().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const LOOSE: Policy = Policy {
        allow_private_addresses: true,
        allow_http: true,
        allow_any_port: true,
    };

    fn png() -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        b.extend_from_slice(&[0; 32]);
        b
    }

    /// Serves each canned response to one connection, in order, on a local port.
    /// Returns the port and the paths requested.
    async fn serve(
        responses: Vec<Vec<u8>>,
    ) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_in = seen.clone();
        tokio::spawn(async move {
            for response in responses {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = request
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split(' ')
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
                seen_in.lock().unwrap().push(path);
                let _ = sock.write_all(&response).await;
                let _ = sock.shutdown().await;
            }
        });
        (port, seen)
    }

    fn http(status: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\n").into_bytes();
        for (k, v) in headers {
            out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(body);
        out
    }

    // ---- address classification

    #[test]
    fn public_addresses_pass_and_every_special_range_is_refused() {
        let public = [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
        ];
        for ip in public {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip} is public");
        }
        let refused = [
            "0.0.0.0",
            "0.1.2.3",
            "127.0.0.1",
            "127.8.8.8",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "192.0.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:db8::1",
            // IPv6 forms that carry a private IPv4 address
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::1",
            "2002:a9fe:a9fe::1",
        ];
        for ip in refused {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip} is not public");
        }
    }

    #[test]
    fn the_edges_of_the_private_ranges_are_exact() {
        assert!(is_public_ip("172.15.255.255".parse().unwrap()));
        assert!(!is_public_ip("172.16.0.0".parse().unwrap()));
        assert!(!is_public_ip("172.31.255.255".parse().unwrap()));
        assert!(is_public_ip("172.32.0.0".parse().unwrap()));
        assert!(is_public_ip("100.63.255.255".parse().unwrap()));
        assert!(is_public_ip("100.128.0.0".parse().unwrap()));
        assert!(is_public_ip("198.17.255.255".parse().unwrap()));
        assert!(is_public_ip("198.20.0.0".parse().unwrap()));
    }

    // ---- address shape

    #[test]
    fn only_a_plain_https_address_is_accepted() {
        let strict = Policy::STRICT;
        assert!(validate_url("https://example.com/a.png", &strict).is_ok());
        assert!(validate_url("https://example.com:443/a.png", &strict).is_ok());
        assert!(validate_url("https://example.com/a.png?x=1#frag", &strict).is_ok());

        assert_eq!(
            validate_url("http://example.com/a.png", &strict).unwrap_err(),
            Refusal::NotHttps
        );
        assert_eq!(
            validate_url("ftp://example.com/a.png", &strict).unwrap_err(),
            Refusal::NotHttps
        );
        assert_eq!(
            validate_url("file:///etc/passwd", &strict).unwrap_err(),
            Refusal::NotHttps
        );
        assert_eq!(
            validate_url("data:image/png;base64,AAAA", &strict).unwrap_err(),
            Refusal::NotHttps
        );
        assert_eq!(
            validate_url("javascript:alert(1)", &strict).unwrap_err(),
            Refusal::NotHttps
        );
        assert_eq!(
            validate_url("//example.com/a.png", &strict).unwrap_err(),
            Refusal::InvalidUrl
        );
        assert_eq!(
            validate_url("https://user:pw@example.com/a.png", &strict).unwrap_err(),
            Refusal::InvalidUrl
        );
        assert_eq!(
            validate_url("https://user@example.com/a.png", &strict).unwrap_err(),
            Refusal::InvalidUrl
        );
        assert_eq!(
            validate_url("https://example.com:8443/a.png", &strict).unwrap_err(),
            Refusal::InvalidUrl
        );
        assert_eq!(validate_url("", &strict).unwrap_err(), Refusal::InvalidUrl);
        assert_eq!(
            validate_url("not a url", &strict).unwrap_err(),
            Refusal::InvalidUrl
        );
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(
            validate_url(&long, &strict).unwrap_err(),
            Refusal::InvalidUrl
        );
    }

    #[tokio::test]
    async fn an_address_that_names_this_machine_or_the_local_network_is_refused_before_any_connection(
    ) {
        for raw in [
            "https://127.0.0.1/a.png",
            "https://[::1]/a.png",
            "https://10.1.2.3/a.png",
            "https://192.168.0.1/a.png",
            "https://169.254.169.254/latest/meta-data/",
            "https://[::ffff:127.0.0.1]/a.png",
            "https://0x7f.1/a.png",
            "https://2130706433/a.png",
            "https://localhost/a.png",
        ] {
            assert_eq!(
                fetch_image(raw, &Policy::STRICT).await.unwrap_err(),
                Refusal::BlockedAddress,
                "{raw}"
            );
        }
    }

    // ---- redirects

    #[test]
    fn a_redirect_target_is_vetted_like_a_first_address() {
        let strict = Policy::STRICT;
        let here = Url::parse("https://cdn.example.com/a/b.png").unwrap();
        assert_eq!(
            next_hop(&here, "/c/d.png", &strict).unwrap().as_str(),
            "https://cdn.example.com/c/d.png"
        );
        assert_eq!(
            next_hop(&here, "https://other.example.org/x.png", &strict)
                .unwrap()
                .as_str(),
            "https://other.example.org/x.png"
        );
        assert_eq!(
            next_hop(&here, "http://other.example.org/x.png", &strict).unwrap_err(),
            Refusal::NotHttps
        );
        assert_eq!(
            next_hop(&here, "file:///etc/passwd", &strict).unwrap_err(),
            Refusal::NotHttps
        );
        assert_eq!(
            next_hop(&here, "https://user:pw@other.example.org/x.png", &strict).unwrap_err(),
            Refusal::InvalidUrl
        );
        assert_eq!(
            next_hop(&here, "https://other.example.org:9000/x.png", &strict).unwrap_err(),
            Refusal::InvalidUrl
        );
    }

    #[tokio::test]
    async fn a_redirect_into_the_local_network_is_refused() {
        // Every hop goes through `next_hop` (the shape of the address) and then
        // `resolve` (where it points), under the same strict policy as the first.
        let strict = Policy::STRICT;
        let here = Url::parse("https://cdn.example.com/a.png").unwrap();
        for target in [
            "https://169.254.169.254/latest/meta-data/",
            "https://127.0.0.1/x.png",
            "https://[::1]/x.png",
            "https://10.0.0.5/x.png",
            "https://[fd00::1]/x.png",
        ] {
            let hop = next_hop(&here, target, &strict).unwrap();
            assert_eq!(
                resolve(&hop, &strict).await.unwrap_err(),
                Refusal::BlockedAddress,
                "{target}"
            );
        }
    }

    #[tokio::test]
    async fn a_redirect_to_a_non_web_address_or_one_with_credentials_is_refused_by_the_fetch_itself(
    ) {
        // Even under the loose policy the scheme and the userinfo rules apply to
        // every hop, so a redirect cannot leave `http(s)` or smuggle credentials.
        for (location, expected) in [
            ("file:///etc/passwd".to_string(), Refusal::NotHttps),
            ("ftp://example.com/x.png".to_string(), Refusal::NotHttps),
            (
                "http://user:pw@127.0.0.1:9/x.png".to_string(),
                Refusal::InvalidUrl,
            ),
        ] {
            let (port, seen) = serve(vec![http(
                "302 Found",
                &[("Location", location.clone())],
                b"",
            )])
            .await;
            assert_eq!(
                fetch_image(&format!("http://127.0.0.1:{port}/a.png"), &LOOSE)
                    .await
                    .unwrap_err(),
                expected,
                "{location}"
            );
            assert_eq!(
                seen.lock().unwrap().len(),
                1,
                "{location}: no second request"
            );
        }
    }

    #[tokio::test]
    async fn a_redirect_is_followed_and_a_chain_longer_than_three_is_refused() {
        let image = png();
        let (port, seen) = serve(vec![
            http("302 Found", &[("Location", "/b.png".to_string())], b""),
            http(
                "200 OK",
                &[("Content-Type", "image/png".to_string())],
                &image,
            ),
        ])
        .await;
        let got = fetch_image(&format!("http://127.0.0.1:{port}/a.png"), &LOOSE)
            .await
            .unwrap();
        assert_eq!(got.mime, "image/png");
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["/a.png".to_string(), "/b.png".to_string()]
        );

        let hop = |to: &str| http("302 Found", &[("Location", to.to_string())], b"");
        let (port, _) = serve(vec![hop("/1"), hop("/2"), hop("/3"), hop("/4")]).await;
        assert_eq!(
            fetch_image(&format!("http://127.0.0.1:{port}/0"), &LOOSE)
                .await
                .unwrap_err(),
            Refusal::TooManyRedirects
        );
    }

    // ---- the response

    #[tokio::test]
    async fn a_png_is_returned_with_its_type_and_exact_bytes() {
        let image = png();
        let (port, _) = serve(vec![http(
            "200 OK",
            &[("Content-Type", "image/png".to_string())],
            &image,
        )])
        .await;
        let got = fetch_image(&format!("http://127.0.0.1:{port}/a.png"), &LOOSE)
            .await
            .unwrap();
        assert_eq!(got.mime, "image/png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(got.data_base64)
                .unwrap(),
            image
        );
    }

    #[tokio::test]
    async fn a_type_with_parameters_and_each_allowed_format_are_accepted() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            (
                "image/jpeg; charset=binary",
                vec![0xff, 0xd8, 0xff, 0xe0, 0, 0],
            ),
            ("IMAGE/GIF", b"GIF89a....".to_vec()),
            ("image/webp", b"RIFF\x10\x00\x00\x00WEBPVP8 ".to_vec()),
        ];
        for (declared, body) in cases {
            let (port, _) = serve(vec![http(
                "200 OK",
                &[("Content-Type", declared.to_string())],
                &body,
            )])
            .await;
            let got = fetch_image(&format!("http://127.0.0.1:{port}/x"), &LOOSE).await;
            assert!(got.is_ok(), "{declared}: {got:?}");
        }
    }

    #[tokio::test]
    async fn svg_html_and_a_mislabelled_body_are_refused() {
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>";
        for (declared, body) in [
            ("image/svg+xml", svg.to_vec()),
            ("text/html", b"<html></html>".to_vec()),
            ("application/octet-stream", png()),
            // declared as a PNG, but the bytes are an SVG / HTML page
            ("image/png", svg.to_vec()),
            ("image/png", b"<html>not an image</html>".to_vec()),
            // declared as a PNG, but the bytes are a GIF
            ("image/png", b"GIF89a....".to_vec()),
        ] {
            let (port, _) = serve(vec![http(
                "200 OK",
                &[("Content-Type", declared.to_string())],
                &body,
            )])
            .await;
            assert_eq!(
                fetch_image(&format!("http://127.0.0.1:{port}/x"), &LOOSE)
                    .await
                    .unwrap_err(),
                Refusal::NotAnImage,
                "{declared}"
            );
        }
        // no Content-Type at all
        let (port, _) = serve(vec![http("200 OK", &[], &png())]).await;
        assert_eq!(
            fetch_image(&format!("http://127.0.0.1:{port}/x"), &LOOSE)
                .await
                .unwrap_err(),
            Refusal::NotAnImage
        );
    }

    #[tokio::test]
    async fn an_error_status_is_refused() {
        for status in [
            "404 Not Found",
            "500 Internal Server Error",
            "204 No Content",
            "403 Forbidden",
        ] {
            let (port, _) = serve(vec![http(
                status,
                &[("Content-Type", "image/png".to_string())],
                &png(),
            )])
            .await;
            assert_eq!(
                fetch_image(&format!("http://127.0.0.1:{port}/x"), &LOOSE)
                    .await
                    .unwrap_err(),
                Refusal::BadStatus,
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn a_body_over_the_limit_is_refused_whether_or_not_the_size_is_announced() {
        // Announced: the header alone refuses it, before the body is read.
        let mut big = png();
        big.resize(MAX_BYTES + 1, 0);
        let (port, _) = serve(vec![http(
            "200 OK",
            &[("Content-Type", "image/png".to_string())],
            &big,
        )])
        .await;
        assert_eq!(
            fetch_image(&format!("http://127.0.0.1:{port}/x"), &LOOSE)
                .await
                .unwrap_err(),
            Refusal::TooLarge
        );

        // Not announced: a chunked body that grows past the limit is cut off.
        let mut chunked = b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        let first = png();
        chunked.extend_from_slice(format!("{:x}\r\n", first.len()).as_bytes());
        chunked.extend_from_slice(&first);
        chunked.extend_from_slice(b"\r\n");
        let filler = vec![0u8; 1024 * 1024];
        for _ in 0..6 {
            chunked.extend_from_slice(format!("{:x}\r\n", filler.len()).as_bytes());
            chunked.extend_from_slice(&filler);
            chunked.extend_from_slice(b"\r\n");
        }
        chunked.extend_from_slice(b"0\r\n\r\n");
        let (port, _) = serve(vec![chunked]).await;
        assert_eq!(
            fetch_image(&format!("http://127.0.0.1:{port}/x"), &LOOSE)
                .await
                .unwrap_err(),
            Refusal::TooLarge
        );
    }

    #[tokio::test]
    async fn a_request_carries_no_credentials_cookies_or_referrer() {
        let image = png();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured_in = captured.clone();
        let response = http(
            "200 OK",
            &[("Content-Type", "image/png".to_string())],
            &image,
        );
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            *captured_in.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            let _ = sock.write_all(&response).await;
            let _ = sock.shutdown().await;
        });
        fetch_image(&format!("http://127.0.0.1:{port}/a.png?q=1"), &LOOSE)
            .await
            .unwrap();
        let request = captured.lock().unwrap().clone();
        assert!(request.starts_with("get /a.png?q=1 http/1.1"), "{request}");
        assert!(request.contains("user-agent: nodespace-app"), "{request}");
        assert!(
            request.contains("accept: image/png,image/jpeg,image/gif,image/webp"),
            "{request}"
        );
        for header in ["cookie:", "authorization:", "referer:", "origin:"] {
            assert!(
                !request.contains(header),
                "{header} must not be sent: {request}"
            );
        }
    }

    #[test]
    fn a_refusal_is_reported_as_a_short_stable_code() {
        let all = [
            (Refusal::InvalidUrl, "invalid_url"),
            (Refusal::NotHttps, "not_https"),
            (Refusal::BlockedAddress, "blocked_address"),
            (Refusal::TooManyRedirects, "too_many_redirects"),
            (Refusal::BadStatus, "bad_status"),
            (Refusal::NotAnImage, "not_an_image"),
            (Refusal::TooLarge, "too_large"),
            (Refusal::Network, "network"),
        ];
        for (refusal, code) in all {
            assert_eq!(refusal.code(), code);
        }
    }
}
