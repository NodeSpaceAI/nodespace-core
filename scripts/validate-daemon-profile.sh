#!/bin/bash
# validate-daemon-profile.sh — refuse a provisioning profile that cannot sign
# the daemon, before build-pkg.sh embeds it.
#
# Usage: validate-daemon-profile.sh <profile> [--decoded]
#   <profile>   the .provisionprofile file (or, with --decoded, its already
#               decoded plist; used by the tests)
#
# Environment:
#   PROFILE_APP_ID       the daemon's App ID, default app.nodespace.daemon
#   PROFILE_TEAM_ID      the team, default F4M5J7YNLS
#   SIGNING_IDENTITY     the signing certificate's common name, looked up in the
#                        keychain search list; its SHA-1 must be among the
#                        profile's certificates
#   SIGNING_CERT_SHA1    the signing certificate's SHA-1, instead of the lookup
#   VALIDATE_NOW         seconds since the epoch to judge expiry at (tests)
#
# Checks: the application identifier and keychain access group are this
# team's App ID, the team is listed, the profile has not expired, and the
# signing certificate is one the profile allows. Prints which check failed and
# nothing of the profile's contents. Exit 0 only when every check passes.

set -euo pipefail
umask 077

PROFILE="${1:?usage: validate-daemon-profile.sh <profile> [--decoded]}"
MODE="${2:-}"
APP_ID="${PROFILE_APP_ID:-app.nodespace.daemon}"
TEAM_ID="${PROFILE_TEAM_ID:-F4M5J7YNLS}"
NOW="${VALIDATE_NOW:-$(date -u +%s)}"

fail() {
    echo "::error::the daemon provisioning profile is not usable: $1" >&2
    exit 1
}

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
DECODED="${WORK}/profile.plist"
if [[ "${MODE}" == "--decoded" ]]; then
    cp "${PROFILE}" "${DECODED}"
else
    security cms -D -i "${PROFILE}" -o "${DECODED}" 2>/dev/null || fail "it is not a signed provisioning profile"
fi

pb() { /usr/libexec/PlistBuddy -c "Print :$1" "${DECODED}" 2>/dev/null; }

full_id="${TEAM_ID}.${APP_ID}"
app_identifier="$(pb 'Entitlements:com.apple.application-identifier' || true)"
if [[ "${app_identifier}" != "${full_id}" && "${app_identifier}" != "${TEAM_ID}.*" ]]; then
    fail "its application identifier is not ${full_id}"
fi

pb 'TeamIdentifier' | grep -qx "[[:space:]]*${TEAM_ID}" || fail "it is not for team ${TEAM_ID}"

groups="$(pb 'Entitlements:keychain-access-groups' || true)"
if ! grep -Eqx "[[:space:]]*(${TEAM_ID//./\\.}\\.${APP_ID//./\\.}|${TEAM_ID}\\.\\*)" <<<"${groups}"; then
    fail "it does not grant the keychain access group ${full_id}"
fi

expiry="$(/usr/bin/plutil -extract ExpirationDate raw -o - "${DECODED}" 2>/dev/null || true)"
expiry_epoch="$(date -j -u -f '%Y-%m-%dT%H:%M:%SZ' "${expiry}" +%s 2>/dev/null || true)"
[[ -n "${expiry_epoch}" ]] || fail "its expiry date cannot be read"
(( expiry_epoch > NOW )) || fail "it has expired"
# Still valid, but soon not: the next release would fail here.
if (( expiry_epoch - NOW < 30 * 86400 )); then
    echo "::warning::the daemon provisioning profile expires within 30 days ($(date -j -u -f '%s' "${expiry_epoch}" '+%Y-%m-%d')): renew it" >&2
fi

signing_sha1="${SIGNING_CERT_SHA1:-}"
if [[ -z "${signing_sha1}" ]]; then
    : "${SIGNING_IDENTITY:?set SIGNING_IDENTITY or SIGNING_CERT_SHA1}"
    signing_sha1="$(security find-certificate -c "${SIGNING_IDENTITY}" -Z 2>/dev/null | awk '/^SHA-1 hash:/ {print $3; exit}')"
    [[ -n "${signing_sha1}" ]] || fail "the signing certificate was not found in the keychain"
fi
signing_sha1="$(tr '[:lower:]' '[:upper:]' <<<"${signing_sha1}")"
matched=0
index=0
while cert_b64="$(/usr/bin/plutil -extract "DeveloperCertificates.${index}" raw -o - "${DECODED}" 2>/dev/null)"; do
    sha1="$(base64 --decode <<<"${cert_b64}" | shasum -a 1 | awk '{print toupper($1)}')"
    if [[ "${sha1}" == "${signing_sha1}" ]]; then
        matched=1
        break
    fi
    index=$((index + 1))
done
(( matched == 1 )) || fail "none of its certificates is the signing certificate"

echo "the daemon provisioning profile is valid for ${full_id} (expires $(date -j -u -f '%s' "${expiry_epoch}" '+%Y-%m-%d'))"
