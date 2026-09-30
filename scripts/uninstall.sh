#!/bin/sh
# NodeSpace uninstaller — POSIX sh
# Stops the daemon, removes binaries and service files.
# User data at ~/.nodespace/database/ is PRESERVED.
set -e

# ── Constants ──────────────────────────────────────────────────────────────────
INSTALL_DIR="$HOME/.nodespace/bin"
SOCKET_PATH="$HOME/.nodespace/daemon.sock"
LOCK_PATH="$HOME/.nodespace/daemon.lock"
PLIST_PATH="$HOME/Library/LaunchAgents/app.nodespace.daemon.plist"
SYSTEMD_SERVICE="$HOME/.config/systemd/user/nodespace.service"
LAUNCHD_LABEL="app.nodespace.daemon"

# Skill install dirs for every harness the installer supports (kept in sync
# by hand with the AGENTS table in packages/skill/src/agents.ts).
SKILL_DIRS="
Claude Code:$HOME/.claude/skills/nodespace
Codex:$HOME/.codex/skills/nodespace
Gemini:$HOME/.gemini/skills/nodespace
OpenCode:$HOME/.opencode/skills/nodespace
"

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

# ── Remove binaries ───────────────────────────────────────────────────────────
if [ -d "$INSTALL_DIR" ]; then
    rm -f "$INSTALL_DIR/nodespaced" "$INSTALL_DIR/nodespace"
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

# ── Remove installed skills ───────────────────────────────────────────────────
IFS='
'
for entry in $SKILL_DIRS; do
    [ -z "$entry" ] && continue
    name=${entry%%:*}
    dir=${entry#*:}
    if [ -d "$dir" ]; then
        if rm -rf "$dir"; then
            printf 'Removed %s skill at %s\n' "$name" "$dir"
        else
            printf 'Warning: failed to remove %s skill at %s\n' "$name" "$dir" >&2
        fi
    fi
done
unset IFS

# ── Done ──────────────────────────────────────────────────────────────────────
printf '\nNodeSpace uninstalled. Your data at ~/.nodespace/database/ has been preserved.\n'
