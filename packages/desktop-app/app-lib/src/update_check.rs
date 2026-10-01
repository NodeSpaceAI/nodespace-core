//! App update check — detect when a newer NodeSpace release is available.
//!
//! Detection only, no auto-update: the running version comes from Tauri's
//! `PackageInfo` (i.e. `tauri.conf.json`, the version the bundle actually ships
//! as), the latest published version is read from the app's [`UpdateSource`],
//! and the two are compared with semver semantics (so `0.10.0` correctly beats
//! `0.9.0`, which a lexicographic compare would get wrong). Sourcing the running
//! version from `PackageInfo` rather than `CARGO_PKG_VERSION` avoids a build that
//! bumped `tauri.conf.json` but not `Cargo.toml` reporting a stale version and
//! nagging against its own release.
//!
//! The update source is managed state: [`assemble`](crate::assemble) stores the
//! one an app crate supplies through
//! [`AppExtensions::update_source`](crate::AppExtensions::update_source), or the
//! built-in source when it supplies none. The built-in source reads the public
//! core repository's GitHub releases and downloads from its releases page.
//!
//! The check is best-effort and must never affect startup: any failure — offline,
//! timeout, rate limit, a malformed or missing tag — resolves to "no update
//! known" rather than surfacing an error. The pure comparison/parse helpers carry
//! the logic and are unit-tested without touching the network;
//! [`check_for_update_for_app`] is the thin I/O shell around them.
//!
//! The frontend renders the surfacing (a non-blocking banner) by listening for the
//! [`UPDATE_AVAILABLE_EVENT`] emitted at startup, or by invoking the
//! [`check_for_update_command`] Tauri command directly. The payload also names
//! where to download the update ([`UpdateStatus::download_url`]), so the banner
//! opens whatever location the source that found the update gave it; a source that
//! names none leaves it `None` and the banner offers no download.

use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

/// The public core repository, whose GitHub releases the built-in source reads.
const CORE_REPO: &str = "NodeSpaceAI/nodespace-core";

/// The page a user of the public source downloads a new release from.
const RELEASES_PAGE_URL: &str = "https://github.com/NodeSpaceAI/nodespace-core/releases/latest";

/// How long to wait on the network before giving up. Deliberately short — a slow
/// or unreachable network must not delay the "is there an update" answer, which is
/// purely informational.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Event emitted to the frontend at startup when — and only when — a newer version
/// is available. The payload is [`UpdateStatus`]. No event is emitted when the app
/// is current or the check fails, so the banner only ever appears on a real update.
pub const UPDATE_AVAILABLE_EVENT: &str = "update://available";

/// Where the app's update check looks for the latest version, and where the
/// update banner's Download button sends the user.
///
/// An app crate supplies one through
/// [`AppExtensions::update_source`](crate::AppExtensions::update_source); an app
/// that supplies none uses the built-in source, [`UpdateSource::community`].
/// The source is fixed when the app is built and stays the same for the life
/// of the process. The running version it is compared with is always the one
/// in the app's bundle config (`tauri.conf.json`), never one the source names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateSource {
    /// Where the latest published version is read from.
    pub latest: LatestVersionSource,
    /// The page the banner's Download button opens when an update is found.
    /// `None` hides Download, leaving the banner with only its dismiss action.
    ///
    /// Use an `http` or `https` URL. The banner opens it through the opener
    /// plugin, whose default scope refuses other schemes, so Download would
    /// then do nothing.
    pub download_url: Option<&'static str>,
}

/// Where an [`UpdateSource`] reads the latest published version from.
///
/// Every failure to read it (offline, a timeout, a non-success status, a body
/// of the wrong shape, or a version that is not semver) means "no update
/// known": the check never reports an error and never shows the banner on one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LatestVersionSource {
    /// The latest release of a GitHub repository, read from the public
    /// `releases/latest` API, which skips drafts and prereleases. Its
    /// `tag_name` is the version, with or without a leading `v`.
    GitHubLatestRelease {
        /// The repository as `owner/name`.
        repo: &'static str,
    },
    /// An endpoint that answers a GET with a JSON object whose `version`
    /// string is the latest version, as semver with or without a leading `v`,
    /// for example `{"version": "v1.4.0"}`. Other fields are ignored.
    VersionEndpoint {
        /// The endpoint's full URL.
        url: &'static str,
    },
}

impl UpdateSource {
    /// The public core repository's latest GitHub release, downloaded from its
    /// releases page.
    #[must_use]
    pub const fn community() -> Self {
        Self {
            latest: LatestVersionSource::GitHubLatestRelease { repo: CORE_REPO },
            download_url: Some(RELEASES_PAGE_URL),
        }
    }
}

/// The update source [`assemble`](crate::assemble) manages when an app crate
/// supplies none: [`UpdateSource::community`].
pub(crate) fn builtin_update_source() -> UpdateSource {
    UpdateSource::community()
}

/// The app's update source, in managed state for every update check to read.
pub(crate) struct UpdateSourceState(pub UpdateSource);

/// The update source managed on `app`, or the built-in one when the app was not
/// built with [`assemble`](crate::assemble).
pub(crate) fn update_source_for_app<R: Runtime>(app: &AppHandle<R>) -> UpdateSource {
    app.try_state::<UpdateSourceState>()
        .map_or_else(builtin_update_source, |state| state.0.clone())
}

/// The outcome of an update check. `latest` is `None` when the check could not
/// determine a published version (offline, timeout, no release, bad payload);
/// `update_available` is only ever `true` when a version was fetched AND parses as
/// strictly newer than the running version. `download_url` is where the source that
/// found the update sends the user to get it; it is `None` when there is no update
/// or the source names no location, and serializes as `null`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpdateStatus {
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub download_url: Option<String>,
}

impl UpdateStatus {
    /// The current-version-only status used whenever no newer version is known —
    /// either the app is up to date or the check could not complete.
    fn no_update(current: &str) -> Self {
        Self {
            current: current.to_string(),
            latest: None,
            update_available: false,
            download_url: None,
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

/// Compare the running version with what a source reported. An update carries the
/// source's `download_url`; anything else (no version, garbage, equal or older)
/// is [`UpdateStatus::no_update`] and carries none, even when the source has one.
fn status_from(current: &str, latest: Option<String>, download_url: Option<&str>) -> UpdateStatus {
    match latest {
        Some(tag) if update_available(current, &tag) => UpdateStatus {
            current: current.to_string(),
            latest: Some(tag),
            update_available: true,
            download_url: download_url.map(str::to_string),
        },
        _ => UpdateStatus::no_update(current),
    }
}

/// Check whether `app`'s update source has a newer release than the running app.
///
/// The running version is `app`'s `PackageInfo` version (the shipped
/// `tauri.conf.json` version), and the source is the one managed on `app`, or
/// the built-in one when `app` manages none. Best-effort: every failure path
/// resolves to [`UpdateStatus::no_update`], so the caller can treat the result
/// uniformly and startup is never blocked or surfaced an error. Returns the
/// current version always; the latest, the flag and the source's download
/// location only when a newer version was positively determined.
pub async fn check_for_update_for_app<R: Runtime>(app: &AppHandle<R>) -> UpdateStatus {
    let current = app.package_info().version.to_string();
    let source = update_source_for_app(app);
    let latest = fetch_latest_version(&source.latest).await;
    if latest.is_none() {
        // Expected when offline, but also what a mistyped source looks like.
        tracing::debug!(source = ?source.latest, "update check found no latest version");
    }
    status_from(&current, latest, source.download_url)
}

/// Read the latest version from `source`, swallowing every error to `None`.
async fn fetch_latest_version(source: &LatestVersionSource) -> Option<String> {
    match source {
        LatestVersionSource::GitHubLatestRelease { repo } => {
            fetch_github_latest_release(repo).await
        }
        LatestVersionSource::VersionEndpoint { url } => fetch_version_endpoint(url).await,
    }
}

/// The GitHub Releases "latest" API endpoint for `repo` (`owner/name`). It
/// answers with the latest published (non-draft, non-prerelease) release.
fn github_latest_release_url(repo: &str) -> String {
    format!("https://api.github.com/repos/{repo}/releases/latest")
}

/// Fetch the latest release tag of `repo` from GitHub, swallowing every error to
/// `None`. GitHub requires a `User-Agent`; the `Accept` header pins the stable v3
/// media type. Kept separate from [`check_for_update_for_app`] so the comparison
/// logic above can be tested without the network.
async fn fetch_github_latest_release(repo: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .ok()?;
    let resp = client
        .get(github_latest_release_url(repo))
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

/// Extract the `version` from a version endpoint's body. Pure so the parsing is
/// unit-tested without a live call. An endpoint answers `{ "version": "v0.1.0", … }`
/// when it knows the latest version; any other body (an `{ "error": "…" }`
/// document, an empty version, not JSON) yields `None`, so an endpoint that is
/// unconfigured or failing simply means "no update known".
fn version_from_endpoint_json(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("version")?
        .as_str()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Fetch the latest version from the version endpoint at `url`, swallowing every
/// error to `None` (offline, timeout, a non-success status, a malformed body).
/// Kept separate from [`check_for_update_for_app`] so
/// [`version_from_endpoint_json`] is testable without the network.
async fn fetch_version_endpoint(url: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .ok()?;
    let resp = client
        .get(url)
        .header("User-Agent", "nodespace-app")
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().await.ok()?;
    version_from_endpoint_json(&body)
}

/// Tauri command: run an update check on demand (e.g. from a "check for updates"
/// menu item or on mount), against the same update source as the startup check.
/// Never errors — returns [`UpdateStatus`].
#[tauri::command]
pub async fn check_for_update_command(app: tauri::AppHandle) -> UpdateStatus {
    check_for_update_for_app(&app).await
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
    fn version_endpoint_body_parsed() {
        // A version endpoint answers `version` (not GitHub's `tag_name`), and
        // other fields are ignored.
        let body =
            r#"{"version":"v0.1.0","name":"Release v0.1.0","published_at":"2026-08-01T12:00:00Z"}"#;
        assert_eq!(version_from_endpoint_json(body).as_deref(), Some("v0.1.0"));
        // And it feeds the same semver comparison as a GitHub release tag.
        assert!(update_available(
            "0.1.0",
            &version_from_endpoint_json(r#"{"version":"0.2.0"}"#).unwrap()
        ));
    }

    #[test]
    fn version_endpoint_error_body_yields_none() {
        // An unconfigured or failing endpoint answers `error`, not `version`.
        assert_eq!(
            version_from_endpoint_json(r#"{"error":"release_detection_unconfigured"}"#),
            None
        );
        assert_eq!(version_from_endpoint_json(r#"{"version":""}"#), None);
        assert_eq!(version_from_endpoint_json("not json"), None);
    }

    #[test]
    fn community_source_targets_the_public_release_endpoint() {
        let LatestVersionSource::GitHubLatestRelease { repo } = UpdateSource::community().latest
        else {
            panic!("the community source reads a GitHub release");
        };
        assert_eq!(
            github_latest_release_url(repo),
            "https://api.github.com/repos/NodeSpaceAI/nodespace-core/releases/latest"
        );
    }

    #[test]
    fn community_source_downloads_from_the_releases_page() {
        assert_eq!(
            UpdateSource::community().download_url,
            Some("https://github.com/NodeSpaceAI/nodespace-core/releases/latest")
        );
    }

    #[test]
    fn builtin_source_is_the_core_repositorys_latest_github_release() {
        let source = builtin_update_source();
        assert_eq!(
            source.latest,
            LatestVersionSource::GitHubLatestRelease {
                repo: "NodeSpaceAI/nodespace-core"
            }
        );
        assert_eq!(source, UpdateSource::community());
    }

    #[test]
    fn no_update_status_is_current_only() {
        let s = UpdateStatus::no_update("0.2.0");
        assert_eq!(s.current, "0.2.0");
        assert_eq!(s.latest, None);
        assert!(!s.update_available);
        assert_eq!(s.download_url, None);
    }

    #[test]
    fn an_update_carries_its_sources_download_url() {
        let s = status_from(
            "0.2.0",
            Some("v0.3.0".to_string()),
            Some("https://example.test/get"),
        );
        assert!(s.update_available);
        assert_eq!(s.latest.as_deref(), Some("v0.3.0"));
        assert_eq!(s.download_url.as_deref(), Some("https://example.test/get"));
    }

    #[test]
    fn no_update_carries_no_download_url_even_when_the_source_has_one() {
        let url = Some("https://example.test/get");
        // Equal, older, garbage and absent versions are all "no update".
        for latest in [Some("0.2.0"), Some("0.1.0"), Some("latest"), None] {
            let s = status_from("0.2.0", latest.map(str::to_string), url);
            assert!(!s.update_available, "latest = {latest:?}");
            assert_eq!(s.download_url, None, "latest = {latest:?}");
        }
    }

    #[test]
    fn an_update_from_a_source_without_a_download_url_has_none() {
        let s = status_from("0.2.0", Some("0.3.0".to_string()), None);
        assert!(s.update_available);
        assert_eq!(s.download_url, None);
    }

    #[test]
    fn download_url_serializes_as_a_key_that_is_null_when_absent() {
        // The frontend reads `download_url` off the wire; a missing key and a
        // null value must both mean "no download", but the shape stays fixed.
        let with = serde_json::to_value(status_from(
            "0.2.0",
            Some("0.3.0".to_string()),
            Some("https://example.test/get"),
        ))
        .unwrap();
        assert_eq!(with["download_url"], "https://example.test/get");

        let without = serde_json::to_value(UpdateStatus::no_update("0.2.0")).unwrap();
        assert!(without.get("download_url").is_some_and(|v| v.is_null()));
    }
}
