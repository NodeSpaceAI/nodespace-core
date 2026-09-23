#!/bin/bash
# build-pkg.sh — build a signed + notarized macOS .pkg installer
#
# Produces a flat package that installs:
#   /usr/local/bin/nodespace
#   /usr/local/bin/nodespaced
#   /Applications/NodeSpace.app
#   /Library/LaunchAgents/app.nodespace.daemon.plist
#
# Usage (from repo root):
#   TRIPLE=aarch64-apple-darwin ./scripts/build-pkg.sh
#   TRIPLE=x86_64-apple-darwin  ./scripts/build-pkg.sh
#
# Required env vars (set by CI from GitHub secrets; set manually for local):
#   APPLE_SIGNING_IDENTITY     — e.g. "Developer ID Application: Acme Inc (TEAMID)"
#   APPLE_INSTALLER_IDENTITY   — e.g. "Developer ID Installer: Acme Inc (TEAMID)"
#   APPLE_ID                   — Apple ID email for notarization
#   APPLE_PASSWORD             — App-specific password for notarization
#   APPLE_TEAM_ID              — 10-char team ID
#
# Optional:
#   PKG_VERSION                — defaults to the canonical app version (tauri.conf.json)
#   SKIP_NOTARIZATION          — set to "1" to skip notarytool (local testing)

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRIPLE="${TRIPLE:-aarch64-apple-darwin}"
SCRIPTS_DIR="${REPO_ROOT}/scripts"
PKG_RESOURCES="${SCRIPTS_DIR}/pkg-resources"
TAURI_APP_PATH="${REPO_ROOT}/target/${TRIPLE}/release/bundle/macos/NodeSpace.app"
BUILD_DIR="${REPO_ROOT}/target/pkg-build"
PAYLOAD_ROOT="${BUILD_DIR}/payload"
OUTPUT_DIR="${REPO_ROOT}/target/pkg-output"

# ---------------------------------------------------------------------------
# Prereqs check
# ---------------------------------------------------------------------------
for tool in jq pkgbuild productbuild codesign xcrun; do
    if ! command -v "${tool}" &>/dev/null; then
        echo "error: '${tool}' not found — install Xcode Command Line Tools" >&2
        exit 1
    fi
done

# --- BEGIN pkg-version-derivation (verified by scripts/build-pkg-version.test.ts) ---
# Derive version from the canonical app-version source if not overridden. tauri.conf.json
# is the file scripts/check-version-sync.ts treats as canonical, and the one
# `release:bump` (scripts/release.ts) keeps in sync on every version bump.
#
# This used to read packages/daemon/Cargo.toml instead — a file `release:bump` never
# touches — so the .pkg's filename (and its own pkgbuild/productbuild version metadata)
# silently drifted to a stale hardcoded value across releases while every other release
# artifact correctly tracked the bumped version (nodespace-core#2310).
if [[ -z "${PKG_VERSION:-}" ]]; then
    TAURI_CONFIG="${REPO_ROOT}/packages/desktop-app/src-tauri/tauri.conf.json"
    PKG_VERSION=$(jq -r '.version' "${TAURI_CONFIG}")
    if [[ -z "${PKG_VERSION}" || "${PKG_VERSION}" == "null" ]]; then
        echo "error: could not read .version from ${TAURI_CONFIG}" >&2
        exit 1
    fi
fi

PKG_NAME="NodeSpace_${PKG_VERSION}_${TRIPLE}.pkg"
# --- END pkg-version-derivation ---
COMPONENT_PKG="${BUILD_DIR}/NodeSpace-component.pkg"
FINAL_PKG="${OUTPUT_DIR}/${PKG_NAME}"

echo "==> Building NodeSpace .pkg installer"
echo "    Version : ${PKG_VERSION}"
echo "    Triple  : ${TRIPLE}"
echo "    Output  : ${FINAL_PKG}"

if [[ ! -d "${TAURI_APP_PATH}" ]]; then
    echo "error: NodeSpace.app not found at ${TAURI_APP_PATH}" >&2
    echo "       Run 'bunx tauri build --target ${TRIPLE}' first." >&2
    exit 1
fi

NODESPACED_BIN="${REPO_ROOT}/target/${TRIPLE}/release/nodespaced"
NODESPACE_BIN="${REPO_ROOT}/target/${TRIPLE}/release/nodespace"
for bin in "${NODESPACED_BIN}" "${NODESPACE_BIN}"; do
    if [[ ! -f "${bin}" ]]; then
        echo "error: binary not found: ${bin}" >&2
        echo "       Run 'cargo build --release --bin nodespaced --bin nodespace --target ${TRIPLE}' first." >&2
        exit 1
    fi
done

# ---------------------------------------------------------------------------
# Sign binaries with Developer ID Application cert
# ---------------------------------------------------------------------------
echo "==> Signing binaries"
APP_IDENTITY="${APPLE_SIGNING_IDENTITY:?APPLE_SIGNING_IDENTITY must be set}"
INSTALLER_IDENTITY="${APPLE_INSTALLER_IDENTITY:?APPLE_INSTALLER_IDENTITY must be set}"

codesign --force --options runtime --timestamp \
    --sign "${APP_IDENTITY}" \
    "${NODESPACED_BIN}"

codesign --force --options runtime --timestamp \
    --sign "${APP_IDENTITY}" \
    "${NODESPACE_BIN}"

# The .app bundle is already signed by Tauri during the build, but re-sign
# with --deep to ensure all nested executables carry the same identity.
codesign --force --deep --options runtime --timestamp \
    --sign "${APP_IDENTITY}" \
    "${TAURI_APP_PATH}"

echo "    Signed nodespaced, nodespace, NodeSpace.app"

# ---------------------------------------------------------------------------
# Assemble payload tree
# ---------------------------------------------------------------------------
echo "==> Assembling payload"
rm -rf "${BUILD_DIR}"
mkdir -p \
    "${PAYLOAD_ROOT}/usr/local/bin" \
    "${PAYLOAD_ROOT}/Applications" \
    "${PAYLOAD_ROOT}/Library/LaunchAgents"

cp "${NODESPACE_BIN}"  "${PAYLOAD_ROOT}/usr/local/bin/nodespace"
cp "${NODESPACED_BIN}" "${PAYLOAD_ROOT}/usr/local/bin/nodespaced"
cp -R "${TAURI_APP_PATH}" "${PAYLOAD_ROOT}/Applications/NodeSpace.app"
cp "${PKG_RESOURCES}/app.nodespace.daemon.plist" \
    "${PAYLOAD_ROOT}/Library/LaunchAgents/app.nodespace.daemon.plist"

chmod 755 "${PAYLOAD_ROOT}/usr/local/bin/nodespace"
chmod 755 "${PAYLOAD_ROOT}/usr/local/bin/nodespaced"

# --- BEGIN macos-chmod (verified by scripts/build-pkg-macos-chmod.test.ts) ---
# The upload-artifact/download-artifact round-trip that hands TAURI_APP_PATH
# between CI jobs does not reliably preserve the Unix executable bit, so the
# binaries inside the copied .app bundle can silently lose +x before they
# ever reach this script. The release workflow (.github/workflows/release.yml)
# already fixes this at the source by tarring the bundle before upload and
# untarring it after download — tar's own format embeds Unix permission bits,
# so that round-trip is lossless regardless of what upload/download-artifact
# does to a bare directory. This chmod is a second, independent line of
# defense kept deliberately in place alongside that fix, not leftover/dead
# code superseded by it: if the tar/untar step is ever reverted or bypassed
# (e.g. a caller other than the release workflow invokes this script against
# a bundle that went through some other handoff), this still guarantees the
# binaries are executable before packaging. Restore it explicitly on just the
# executables — never recursively on the whole bundle, which would also touch
# Info.plist/resources/etc.
#
# shopt/nullglob + explicit empty-check first, matching the pattern
# release.yml's verify_layout() already uses for this exact same glob: not
# reachable today given the current externalBin config (always exactly 3
# external binaries plus the app's own executable), but without this an
# empty Contents/MacOS would leave the glob unexpanded and hand chmod the
# literal, unmatched pattern — a confusing raw BSD-glob "No such file or
# directory" instead of a clear diagnostic naming the actual problem.
shopt -s nullglob
macos_bins=("${PAYLOAD_ROOT}/Applications/NodeSpace.app/Contents/MacOS/"*)
shopt -u nullglob
if [[ ${#macos_bins[@]} -eq 0 ]]; then
    echo "error: ${PAYLOAD_ROOT}/Applications/NodeSpace.app/Contents/MacOS has no files to chmod" >&2
    exit 1
fi
chmod 755 "${macos_bins[@]}"
# --- END macos-chmod ---

# ---------------------------------------------------------------------------
# Build component package
# ---------------------------------------------------------------------------
echo "==> Building component .pkg"
# --component-plist pins BundleIsRelocatable=false for NodeSpace.app so macOS's
# bundle-relocation logic can't redirect the install to some other on-disk copy
# of com.nodespace.desktop — it always lands at /Applications/NodeSpace.app,
# matching RootRelativeBundlePath in that plist.
pkgbuild \
    --root "${PAYLOAD_ROOT}" \
    --identifier "com.nodespace.pkg" \
    --version "${PKG_VERSION}" \
    --scripts "${PKG_RESOURCES}" \
    --install-location "/" \
    --component-plist "${PKG_RESOURCES}/component.plist" \
    "${COMPONENT_PKG}"

# ---------------------------------------------------------------------------
# Build flat distribution package
# ---------------------------------------------------------------------------
echo "==> Building distribution .pkg"
mkdir -p "${OUTPUT_DIR}"

# Write a minimal distribution XML so productbuild can produce a flat pkg
DIST_XML="${BUILD_DIR}/distribution.xml"
cat > "${DIST_XML}" <<DIST_XML_EOF
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="1">
    <title>NodeSpace ${PKG_VERSION}</title>
    <organization>com.nodespace</organization>
    <domains enable_localSystem="true"/>
    <options customize="never" require-scripts="true" rootVolumeOnly="true"/>
    <pkg-ref id="com.nodespace.pkg"/>
    <choices-outline>
        <line choice="default">
            <line choice="com.nodespace.pkg"/>
        </line>
    </choices-outline>
    <choice id="default"/>
    <choice id="com.nodespace.pkg" visible="false">
        <pkg-ref id="com.nodespace.pkg"/>
    </choice>
    <pkg-ref id="com.nodespace.pkg" version="${PKG_VERSION}" onConclusion="none">NodeSpace-component.pkg</pkg-ref>
</installer-gui-script>
DIST_XML_EOF

productbuild \
    --distribution "${DIST_XML}" \
    --package-path "${BUILD_DIR}" \
    --sign "${INSTALLER_IDENTITY}" \
    --timestamp \
    "${FINAL_PKG}"

# ---------------------------------------------------------------------------
# Notarize and staple
# ---------------------------------------------------------------------------
if [[ "${SKIP_NOTARIZATION:-0}" == "1" ]]; then
    echo "==> Skipping notarization (SKIP_NOTARIZATION=1)"
else
    echo "==> Notarizing (this takes 1-5 minutes)..."
    APPLE_ID="${APPLE_ID:?APPLE_ID must be set}"
    APPLE_PASSWORD="${APPLE_PASSWORD:?APPLE_PASSWORD must be set}"
    APPLE_TEAM_ID="${APPLE_TEAM_ID:?APPLE_TEAM_ID must be set}"

    xcrun notarytool submit "${FINAL_PKG}" \
        --apple-id "${APPLE_ID}" \
        --password "${APPLE_PASSWORD}" \
        --team-id "${APPLE_TEAM_ID}" \
        --wait

    echo "==> Stapling notarization ticket"
    xcrun stapler staple "${FINAL_PKG}"

    echo "==> Verifying Gatekeeper acceptance"
    # --- BEGIN spctl-exit-code (verified by scripts/build-pkg-spctl-exit-code.test.ts) ---
    # spctl's documented exit codes: 0 = accepted, 3 = assessment denied -- the
    # only two verdicts spctl itself defines. Under `set -euo pipefail`, a bare
    # `spctl ... && echo OK` treats ANY non-zero exit (a transient network/OCSP
    # hiccup, a bad invocation, spctl crashing) as if it were exit 3, aborting
    # the release build and misreporting a check failure as a genuine Gatekeeper
    # rejection. Capture the exit code explicitly instead and branch on it, the
    # same way scripts/verify-pkg-gatekeeper.ts's assessGatekeeperInstall does.
    set +e
    SPCTL_OUTPUT=$(spctl --assess --type install --verbose "${FINAL_PKG}" 2>&1)
    SPCTL_EXIT=$?
    set -e
    # Print spctl's own assessment output unconditionally -- the previous
    # unredirected `spctl ... && echo OK` always surfaced it in the build log
    # regardless of outcome, and this preserves that visibility now that the
    # output is captured into a variable to inspect the exit code first.
    echo "${SPCTL_OUTPUT}"
    if [[ ${SPCTL_EXIT} -eq 0 ]]; then
        echo "    ✓ Gatekeeper: OK"
    elif [[ ${SPCTL_EXIT} -eq 3 ]]; then
        echo "error: Gatekeeper rejected ${FINAL_PKG} (spctl exited 3)" >&2
        exit 1
    else
        echo "error: spctl exited ${SPCTL_EXIT} (expected 0 or 3) -- not a Gatekeeper verdict" >&2
        exit 1
    fi
    # --- END spctl-exit-code ---
fi

echo ""
echo "✓ Package ready: ${FINAL_PKG}"
