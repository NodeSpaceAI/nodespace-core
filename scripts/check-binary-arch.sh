#!/usr/bin/env bash
# Usage: check-binary-arch.sh <binary> <rust-target-triple>
# Fails unless the executable format of <binary> matches <rust-target-triple>.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <binary> <rust-target-triple>" >&2
  exit 2
fi
bin="$1"
triple="$2"

case "$triple" in
  aarch64-unknown-linux-gnu) expect='ELF 64-bit.*(ARM aarch64|ARM64)' ;;
  x86_64-unknown-linux-gnu) expect='ELF 64-bit.*x86-64' ;;
  aarch64-apple-darwin) expect='Mach-O 64-bit.*arm64' ;;
  x86_64-apple-darwin) expect='Mach-O 64-bit.*x86_64' ;;
  x86_64-pc-windows-msvc) expect='PE32\+.*x86-64' ;;
  *) echo "check-binary-arch: no expectation for target '$triple'" >&2; exit 2 ;;
esac

desc="$(file -b "$bin")"
if printf '%s' "$desc" | grep -Eq "$expect"; then
  echo "ok: $bin is $desc"
else
  echo "error: $bin does not match $triple: $desc" >&2
  exit 1
fi
