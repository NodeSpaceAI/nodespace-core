#!/bin/sh
# NodeSpace uninstaller — POSIX sh
# Stops the daemon, removes the skill from agent harnesses, then removes
# binaries and service files.
# User data at ~/.nodespace/database/ is PRESERVED.
set -e

# ── Constants ──────────────────────────────────────────────────────────────────
INSTALL_DIR="$HOME/.nodespace/bin"
SOCKET_PATH="$HOME/.nodespace/daemon.sock"
LOCK_PATH="$HOME/.nodespace/daemon.lock"
PLIST_PATH="$HOME/Library/LaunchAgents/app.nodespace.daemon.plist"
SYSTEMD_SERVICE="$HOME/.config/systemd/user/nodespace.service"
LAUNCHD_LABEL="app.nodespace.daemon"

OS=$(uname -s)

# ── Stop daemon ───────────────────────────────────────────────────────────────
printf 'Stopping nodespaced...\n'
case "$OS" in
    Darwin)
        launchctl bootout "gui/$(id -u)/$LAUNCHD_LABEL" 2>/dev/null || true
        ;;
    Linux)
        systemctl --user stop nodespace 2>/dev/null || true
        systemctl --user disable nodespace 2>/dev/null || true
        ;;
esac

# ── Remove service files ──────────────────────────────────────────────────────
case "$OS" in
    Darwin)
        if [ -f "$PLIST_PATH" ]; then
            rm -f "$PLIST_PATH"
            printf 'Removed %s\n' "$PLIST_PATH"
        fi
        ;;
    Linux)
        if [ -f "$SYSTEMD_SERVICE" ]; then
            rm -f "$SYSTEMD_SERVICE"
            systemctl --user daemon-reload 2>/dev/null || true
            printf 'Removed %s\n' "$SYSTEMD_SERVICE"
        fi
        ;;
esac

# ── Remove the skill from agent harnesses ─────────────────────────────────────
# The skill installer knows what it put where: the skill, a harness's plugin,
# and the marked block in a harness's own instructions file. It sits beside the
# CLI, so this runs before the binaries are removed, and no list of harness
# folders is kept here.
NODESPACE_CLI="$INSTALL_DIR/nodespace"
if [ ! -x "$NODESPACE_CLI" ]; then
    # Installed some other way: whichever `nodespace` is on the path.
    NODESPACE_CLI=$(command -v nodespace 2>/dev/null || true)
fi
if [ -n "$NODESPACE_CLI" ]; then
    "$NODESPACE_CLI" skill uninstall ||
        printf 'Warning: the skill was not removed from every agent harness\n' >&2
else
    printf 'Warning: no nodespace command found; the skill was not removed from any agent harness\n' >&2
fi

# ── Remove binaries ───────────────────────────────────────────────────────────
if [ -d "$INSTALL_DIR" ]; then
    rm -f "$INSTALL_DIR/nodespaced" "$INSTALL_DIR/nodespace" \
        "$INSTALL_DIR/nodespace-skill-installer"
    rm -rf "$INSTALL_DIR/skill"
    # Remove the bin dir only if empty
    rmdir "$INSTALL_DIR" 2>/dev/null || true
    printf 'Removed binaries from %s\n' "$INSTALL_DIR"
fi

# ── Remove socket and its single-instance lock file ───────────────────────────
if [ -e "$SOCKET_PATH" ]; then
    rm -f "$SOCKET_PATH"
    printf 'Removed socket %s\n' "$SOCKET_PATH"
fi
if [ -e "$LOCK_PATH" ]; then
    rm -f "$LOCK_PATH"
    printf 'Removed lock file %s\n' "$LOCK_PATH"
fi

# ── Done ──────────────────────────────────────────────────────────────────────
printf '\nNodeSpace uninstalled. Your data at ~/.nodespace/database/ has been preserved.\n'
