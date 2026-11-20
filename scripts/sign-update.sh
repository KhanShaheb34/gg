#!/bin/sh
# Signs gg's update files with the release key, for the updater to verify (against the public
# key in src-tauri/tauri.conf.json) before it installs anything: writes <file>.sig beside each,
# bound to version <version> (tauri signer sign --app-version), which the app requires.
#
#   TAURI_SIGNING_PRIVATE_KEY=... TAURI_SIGNING_PRIVATE_KEY_PASSWORD=... \
#     scripts/sign-update.sh <version> <file>...
#
# Fails, signing nothing, without the key: a release is never made with unsigned updates.
# The release workflow runs it in a step of its own, the only one that sees the key.
set -eu

fail() {
  echo "sign-update: $*" >&2
  [ -n "${GITHUB_ACTIONS:-}" ] && echo "::error::sign-update: $*"
  exit 1
}

[ $# -ge 2 ] || fail "usage: scripts/sign-update.sh <version> <file>..."
version=$1
shift
[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] ||
  fail "TAURI_SIGNING_PRIVATE_KEY is not set: the updates cannot be signed (set the repository secrets TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD)"
for file in "$@"; do
  [ -f "$file" ] || fail "$file is missing"
done
root=$(cd "$(dirname "$0")/.." && pwd)
for file in "$@"; do
  file=$(cd "$(dirname "$file")" && pwd)/$(basename "$file")
  rm -f "$file.sig"
  (cd "$root" && pnpm -s tauri signer sign --app-version "$version" "$file" </dev/null >/dev/null) ||
    fail "could not sign $file"
  [ -s "$file.sig" ] || fail "signing $file wrote no $file.sig"
  # The signature says the version it was made for (the app refuses one that does not).
  base64 -d <"$file.sig" 2>/dev/null | grep -q "version:$version\$" ||
    base64 -D <"$file.sig" 2>/dev/null | grep -q "version:$version\$" ||
    fail "$file.sig does not name version $version"
  echo "sign-update: $file.sig (version $version)"
done
