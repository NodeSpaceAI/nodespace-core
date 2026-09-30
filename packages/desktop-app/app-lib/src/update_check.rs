//! App update check — detect when a newer NodeSpace release is available.
//!
//! Detection only, no auto-update: the running version comes from Tauri's
//! `PackageInfo` (i.e. `tauri.conf.json`, the version the bundle actually ships
//! as), the latest published version is read from the release source for this build
//! variant — the public GitHub Releases API for a community build, or the
//! cloud-worker `/pro/latest-version` proxy for a Pro build (the Pro app ships from
//! a private repo it can't read directly) — and the two are compared with semver
//! semantics (so `0.10.0` correctly beats `0.9.0`, which a lexicographic compare
//! would get wrong). Sourcing the running
//! version from `PackageInfo` rather than `CARGO_PKG_VERSION` avoids a build that
//! bumped `tauri.conf.json` but not `Cargo.toml` reporting a stale version and
//! nagging against its own release.
//!
//! The check is best-effort and must never affect startup: any failure — offline,
//! timeout, rate limit, a malformed or missing tag — resolves to "no update
//! known" rather than surfacing an error. The pure comparison/parse helpers carry
//! the logic and are unit-tested without touching the network; [`check_for_update`]
//! is the thin I/O shell around them.
//!
//! The frontend renders the surfacing (a non-blocking banner) by listening for the
//! [`UPDATE_AVAILABLE_EVENT`] emitted at startup, or by invoking the
//! [`check_for_update_command`] Tauri command directly.

use serde::Serialize;
use std::time::Duration;

/// GitHub Releases "latest" endpoint for the public core repository. The latest
/// published (non-draft, non-prerelease) release is what a **community** user can
/// install.
const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/NodeSpaceAI/nodespace-core/releases/latest";

/// Pro update source: the cloud-worker proxy that returns the latest **private**
/// nodespace-sync release version (the Pro app can't read that private repo
/// directly). A Pro build checks this instead of the public core endpoint. Mirrors
/// the auth-worker URL selection in `commands::pro_sync` — release hits the
/// canonical domain, debug hits the local `wrangler dev` worker.
#[cfg(debug_assertions)]
const PRO_LATEST_URL: &str = "http://127.0.0.1:8787/pro/latest-version";
#[cfg(not(debug_assertions))]
const PRO_LATEST_URL: &str = "https://pro.nodespace.ai/pro/latest-version";

/// How long to wait on the network before giving up. Deliberately short — a slow
/// or unreachable network must not delay the "is there an update" answer, which is
/// purely informational.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Event emitted to the frontend at startup when — and only when — a newer version
/// is available. The payload is [`UpdateStatus`]. No event is emitted when the app
/// is current or the check fails, so the banner only ever appears on a real update.
pub const UPDATE_AVAILABLE_EVENT: &str = "update://available";

/// The outcome of an update check. `latest` is `None` when the check could not
/// determine a published version (offline, timeout, no release, bad payload);
/// `update_available` is only ever `true` when a version was fetched AND parses as
/// strictly newer than the running version.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpdateStatus {
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
}

impl UpdateStatus {
    /// The current-version-only status used whenever no newer version is known —
    /// either the app is up to date or the check could not complete.
    fn no_update(current: &str) -> Self {
        Self {
            current: current.to_string(),
            latest: None,
            update_available: false,
        }
    }
}

/// Parse a release tag or version string into a semver `Version`, tolerating a
/// leading `v` (`v0.2.0` and `0.2.0` both parse). Returns `None` for anything that
/// is not valid semver — the caller then treats it as "no update known" rather
/// than nagging on a garbage tag.
fn parse_version(tag: &str) -> Option<semver::Version> {
    let trimmed = tag.trim();
    let normalized = trimmed.strip_prefix('v').unwrap_or(trimmed);
    semver::Version::parse(normalized).ok()
}

/// Whether `latest` is a strictly newer version than `current`, by semver. Any
/// unparseable input yields `false` (fail-safe: never claim an update on garbage,
/// and never nag when we cannot be sure).
pub fn update_available(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(cur), Some(new)) => new > cur,
        _ => false,
    }
}

/// Extract the `tag_name` from a GitHub `releases/latest` JSON body. Pure so the
/// parsing is unit-tested without a live API call. Returns `None` if the body is
/// not the expected shape (e.g. a rate-limit error document, which has no
/// `tag_name`).
fn latest_tag_from_json(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("tag_name")?
        .as_str()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Check whether a newer release exists than `current` (the running app version,
/// passed in from Tauri's `PackageInfo`). Best-effort: every failure path resolves
/// to [`UpdateStatus::no_update`], so the caller can treat the result uniformly and
/// startup is never blocked or surfaced an error. Returns the current version
/// always, the latest and the flag only when a newer version was positively
/// determined.
pub async fn check_for_update(current: &str) -> UpdateStatus {
    // A Pro build tracks the PRIVATE nodespace-sync releases through the cloud-worker
    // proxy (it can't read that repo directly); a community build tracks the public
    // core repo. Either way the comparison and the surfaced banner are identical.
    let latest = if crate::daemon_setup::is_pro_build() {
        fetch_pro_latest_version().await
    } else {
        fetch_latest_tag().await
    };
    match latest {
        Some(tag) if update_available(current, &tag) => UpdateStatus {
            current: current.to_string(),
            latest: Some(tag),
            update_available: true,
        },
        _ => UpdateStatus::no_update(current),
    }
}

/// Fetch the latest release tag from GitHub, swallowing every error to `None`.
/// GitHub requires a `User-Agent`; the `Accept` header pins the stable v3 media
/// type. Kept separate from [`check_for_update`] so the comparison logic above can
/// be tested without the network.
async fn fetch_latest_tag() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .ok()?;
    let resp = client
        .get(LATEST_RELEASE_URL)
        .header("User-Agent", "nodespace-app")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().await.ok()?;
    latest_tag_from_json(&body)
}

/// Extract the version from the cloud-worker `/pro/latest-version` body. Pure so the
/// parsing is unit-tested without a live call. The worker returns
/// `{ "version": "v0.1.0", … }` on success, or `{ "error": "…" }` (no `version`)
/// when it is unconfigured or upstream failed — the latter yields `None`, so a Pro
/// client whose endpoint isn't set up simply sees "no update known".
fn pro_version_from_json(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("version")?
        .as_str()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Fetch the latest Pro release version from the cloud-worker proxy, swallowing
/// every error to `None` (offline, timeout, 503-unconfigured, 5xx, malformed body).
/// The worker holds the private-repo token; the client only ever receives a version
/// string. Kept separate from [`check_for_update`] so [`pro_version_from_json`] is
/// testable without the network.
async fn fetch_pro_latest_version() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .ok()?;
    let resp = client
        .get(PRO_LATEST_URL)
        .header("User-Agent", "nodespace-app")
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().await.ok()?;
    pro_version_from_json(&body)
}

/// Tauri command: run an update check on demand (e.g. from a "check for updates"
/// menu item or on mount). Never errors — returns [`UpdateStatus`]. The running
/// version is taken from `PackageInfo` (the shipped `tauri.conf.json` version).
#[tauri::command]
pub async fn check_for_update_command(app: tauri::AppHandle) -> UpdateStatus {
    let current = app.package_info().version.to_string();
    check_for_update(&current).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_not_lexicographic() {
        // The whole point of using semver: 0.10.0 is newer than 0.9.0, which a
        // string compare would get backwards.
        assert!(update_available("0.9.0", "0.10.0"));
        assert!(!update_available("0.10.0", "0.9.0"));
        assert!(update_available("0.2.0", "0.2.10"));
    }

    #[test]
    fn equal_and_older_are_not_updates() {
        assert!(!update_available("0.2.0", "0.2.0"));
        assert!(!update_available("1.0.0", "0.9.9"));
    }

    #[test]
    fn tolerates_leading_v_on_either_side() {
        assert!(update_available("v0.2.0", "v0.3.0"));
        assert!(update_available("0.2.0", "v0.3.0"));
        assert!(!update_available("v0.3.0", "0.2.0"));
    }

    #[test]
    fn prerelease_is_older_than_release() {
        // semver: 1.0.0-rc.1 < 1.0.0, so a stable release is an update over an rc.
        assert!(update_available("1.0.0-rc.1", "1.0.0"));
        assert!(!update_available("1.0.0", "1.0.0-rc.1"));
    }

    #[test]
    fn garbage_never_claims_an_update() {
        assert!(!update_available("not-a-version", "0.3.0"));
        assert!(!update_available("0.2.0", "latest"));
        assert!(!update_available("", ""));
    }

    #[test]
    fn latest_tag_parsed_from_release_json() {
        let body = r#"{"tag_name":"v0.3.0","name":"0.3.0","draft":false}"#;
        assert_eq!(latest_tag_from_json(body).as_deref(), Some("v0.3.0"));
    }

    #[test]
    fn missing_or_error_json_yields_none() {
        // A rate-limit / error document has no tag_name.
        assert_eq!(
            latest_tag_from_json(r#"{"message":"API rate limit exceeded"}"#),
            None
        );
        assert_eq!(latest_tag_from_json("not json at all"), None);
        assert_eq!(latest_tag_from_json(r#"{"tag_name":""}"#), None);
    }

    #[test]
    fn pro_version_parsed_from_worker_json() {
        // The cloud-worker returns `version` (not GitHub's `tag_name`).
        let body = r#"{"version":"v0.1.0","name":"NodeSpace Pro v0.1.0","published_at":"2026-08-01T12:00:00Z"}"#;
        assert_eq!(pro_version_from_json(body).as_deref(), Some("v0.1.0"));
        // And it feeds the same semver comparison as the community path.
        assert!(update_available(
            "0.1.0",
            &pro_version_from_json(r#"{"version":"0.2.0"}"#).unwrap()
        ));
    }

    #[test]
    fn pro_unconfigured_or_error_body_yields_none() {
        // 503-unconfigured / upstream-error bodies carry `error`, not `version`.
        assert_eq!(
            pro_version_from_json(r#"{"error":"pro_release_detection_unconfigured"}"#),
            None
        );
        assert_eq!(pro_version_from_json(r#"{"version":""}"#), None);
        assert_eq!(pro_version_from_json("not json"), None);
    }

    #[test]
    fn no_update_status_is_current_only() {
        let s = UpdateStatus::no_update("0.2.0");
        assert_eq!(s.current, "0.2.0");
        assert_eq!(s.latest, None);
        assert!(!s.update_available);
    }
}
