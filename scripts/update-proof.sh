#!/usr/bin/env bash
# Proves on macOS, with the real app, that gg updates itself: an "installed" copy of the perf
# build at an old version checks a local update source, downloads a newer version, verifies
# it, and on Restart to update (clicked in its own notice) is replaced in place and restarted;
# the newer version then runs from the same path with the tabs that were open. Then the cases
# where nothing may be installed: a tampered download, a download signed with another key, one
# signed for another version, and an update source offering the same or an older version.
#
#   scripts/update-proof.sh [--no-build] <outdir>
#
# Builds the perf bundle (`--features perf`, identifier com.shakirul.gg.perf: it shares no
# storage, single-instance lock or LaunchServices identity with an installed gg, which is
# never started, stopped or read) twice, at the fake versions 0.3.9 (old) and 0.9.0 (new),
# into <outdir>/old and <outdir>/new, each with the gg launcher inside and signed ad hoc as a
# release is (--no-build reuses them). The new one is packed as the macOS update
# (scripts/update-archive.sh) and signed by the release's own signer (gg-release sign, as the
# release workflow's sign job runs it) with a throwaway, password-protected key that the Tauri
# CLI makes here for this run; the release key is never used. A Python HTTP server on 127.0.0.1
# serves latest.json and the archive; it logs every request.
#
# Each case copies the old app to <outdir>/case-<name>/install/gg.app and starts it with
# `open` and `--perf-env <file>`: the perf build reads its environment from that file, so the
# copy restarted by the update (through LaunchServices, which passes the arguments but not the
# environment) gets the same one: its own config folder (GG_CONFIG_DIR, with a saved session of
# two tabs), git without the user's config, a stand-in login shell, the test update source
# (GG_UPDATE_ENDPOINT, GG_UPDATE_PUBKEY: the perf build obeys them only on 127.0.0.1) and the
# update probe (GG_PERF_SCROLL=update, src/lib/perf/updateProbe.ts), which writes what it sees
# to the perf log and clicks the notice's Restart to update. Only the pids of apps under
# <outdir> are ever signalled.
#
# Prints each check, PASS or FAIL, and exits 1 if any failed. Keep the screen unlocked and the
# window uncovered (a covered window gets no animation frames, and the graph's probe waits).
set -uo pipefail

build=1
if [ "${1-}" = --no-build ]; then build=0; shift; fi
[ $# -eq 1 ] || { echo "usage: scripts/update-proof.sh [--no-build] <outdir>" >&2; exit 2; }
[ "$(uname -s)" = Darwin ] || { echo "macOS only" >&2; exit 2; }
out=$(mkdir -p "$1" && cd "$1" && pwd)
root=$(cd "$(dirname "$0")/.." && pwd)
perf_id=com.shakirul.gg.perf
old_version=0.3.9
new_version=0.9.0

failures=0
pass() { echo "PASS  $*"; }
fail() { echo "FAIL  $*"; failures=$((failures + 1)); }
die() { echo "update-proof: $*" >&2; exit 1; }

# --- The apps ------------------------------------------------------------------------------

# Builds the perf bundle at version $1 into $2/gg.app: launcher inside, signed ad hoc.
make_app() {
  (cd "$root" && CARGO_INCREMENTAL=0 pnpm tauri build --bundles app --features perf \
    --config "{\"identifier\":\"$perf_id\",\"version\":\"$1\"}" >&2) || die "the build of $1 failed"
  rm -rf "$2"
  mkdir -p "$2"
  cp -R "$root/target/release/bundle/macos/gg.app" "$2/"
  sh "$root/scripts/fill-launcher.sh" "$1" "$2/gg.app/Contents/Resources/gg"
  codesign --force --deep -s - "$2/gg.app" 2>/dev/null
  codesign --verify --deep --strict "$2/gg.app" || die "$2/gg.app does not verify"
}

plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist" 2>/dev/null; }

if [ "$build" = 1 ]; then
  make_app "$old_version" "$out/old"
  make_app "$new_version" "$out/new"
fi
for v in old new; do
  [ -d "$out/$v/gg.app" ] || die "no $out/$v/gg.app: run without --no-build"
  [ "$(plist "$out/$v/gg.app" CFBundleIdentifier)" = "$perf_id" ] || die "$out/$v/gg.app is not the perf build"
done
[ "$(plist "$out/old/gg.app" CFBundleShortVersionString)" = "$old_version" ] || die "old is not $old_version"
[ "$(plist "$out/new/gg.app" CFBundleShortVersionString)" = "$new_version" ] || die "new is not $new_version"
app_bin() { echo "$1/Contents/MacOS/$(plist "$1" CFBundleExecutable)"; }
# Nothing of this run's may already run.
if pgrep -f "^$out/" >/dev/null; then die "an app under $out is running; quit it first"; fi

# --- Keys, archive, server ------------------------------------------------------------------

keys=$out/keys
rm -rf "$keys"
mkdir -p "$keys"
for k in test other; do
  (cd "$root" && pnpm -s tauri signer generate --ci -p "proof-$k" -w "$keys/$k" >/dev/null 2>&1) || die "could not make a throwaway key"
done
# The release's signer.
(cd "$root" && cargo build -q --locked -p gg-release) || die "could not build gg-release"
signer=$root/target/debug/gg-release
serve=$out/serve
rm -rf "$serve"
mkdir -p "$serve"
sh "$root/scripts/update-archive.sh" "$out/new/gg.app" "$serve/gg.app.tar.gz" || die "update-archive failed"
sign() { # file version key
  env -i PATH=/usr/bin:/bin TAURI_SIGNING_PRIVATE_KEY="$(cat "$keys/$3")" \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD="proof-$3" \
    "$signer" sign --version "$2" "$1" >/dev/null || die "could not sign $1"
}
sign "$serve/gg.app.tar.gz" "$new_version" test
# The signer refuses, and writes nothing, with a wrong password or without the key.
cp "$serve/gg.app.tar.gz" "$out/refused.tar.gz"
rm -f "$out/refused.tar.gz.sig"
if env -i PATH=/usr/bin:/bin TAURI_SIGNING_PRIVATE_KEY="$(cat "$keys/test")" \
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD=wrong "$signer" sign --version "$new_version" \
  "$out/refused.tar.gz" 2>"$out/refused.err" || [ -e "$out/refused.tar.gz.sig" ]; then
  fail "signer: a wrong password signed"
else
  pass "signer: a wrong password signs nothing ($(cat "$out/refused.err"))"
fi
if env -i PATH=/usr/bin:/bin "$signer" sign --version "$new_version" "$out/refused.tar.gz" \
  2>"$out/refused.err" || [ -e "$out/refused.tar.gz.sig" ]; then
  fail "signer: signed without a key"
else
  pass "signer: without the key it signs nothing ($(head -1 "$out/refused.err" | cut -c1-90)…)"
fi
# A tampered copy (one byte changed) with the good signature; a copy signed for another version.
cp "$serve/gg.app.tar.gz" "$serve/tampered.tar.gz"
printf 'X' | dd of="$serve/tampered.tar.gz" bs=1 seek=4096 conv=notrunc 2>/dev/null
cp "$serve/gg.app.tar.gz.sig" "$serve/tampered.tar.gz.sig"
cp "$serve/gg.app.tar.gz" "$serve/other-version.tar.gz"
sign "$serve/other-version.tar.gz" 0.9.5 test

server_log=$out/server.log
served=0
python3 -u -I -m http.server 0 --bind 127.0.0.1 --directory "$serve" >"$out/server.out" 2>"$server_log" &
server=$!
pid=""
cleanup() {
  for p in $pid $server; do kill "$p" 2>/dev/null; done
  # Any app of this run's still up (a restarted one): only pids whose program is under $out.
  for p in $(pgrep -f "^$out/"); do kill "$p" 2>/dev/null; done
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP
port=""
for _ in $(seq 1 50); do
  port=$(sed -n 's/.*port \([0-9]*\).*/\1/p' "$out/server.out" | head -1)
  [ -n "$port" ] && break
  sleep 0.1
done
[ -n "$port" ] || die "the server did not start"
base=http://127.0.0.1:$port

# latest.json offering $1 from file $2 (its .sig the signature).
manifest() {
  local sig
  sig=$(cat "$serve/$2.sig")
  printf '{"version":"%s","notes":"proof","pub_date":"%s","platforms":{"darwin-aarch64":{"url":"%s/%s","signature":"%s"},"darwin-x86_64":{"url":"%s/%s","signature":"%s"}}}\n' \
    "$1" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$base" "$2" "$sig" "$base" "$2" "$sig" >"$serve/latest.json"
}

# --- Repositories, tools ------------------------------------------------------------------

fixture=$out/fixture
rm -rf "$fixture"
mkdir -p "$fixture/tools"
: >"$fixture/gitconfig-empty"
export GIT_CONFIG_GLOBAL=$fixture/gitconfig-empty GIT_CONFIG_NOSYSTEM=1
unset GIT_CONFIG_SYSTEM GIT_CONFIG_COUNT GIT_CONFIG_PARAMETERS
for name in alpha beta; do
  git init -q -b main "$fixture/$name"
  for i in 1 2 3; do
    echo "$name $i" >"$fixture/$name/file.txt"
    git -C "$fixture/$name" add file.txt
    git -C "$fixture/$name" -c user.name=Ada -c user.email=ada@example.invalid commit -q -m "$name $i"
  done
done
# Nothing that reaches the network may run.
for blocked in ssh open gh; do
  printf '#!/bin/sh\necho "%s $*" >> "%s/blocked.log"\nexit 97\n' "$blocked" "$fixture" >"$fixture/tools/$blocked"
  chmod +x "$fixture/tools/$blocked"
done
printf '#!/bin/sh\nPATH=%s:/usr/bin:/bin\nexport PATH\neval "$2"\n' "$fixture/tools" >"$fixture/tools/fake-shell"
chmod +x "$fixture/tools/fake-shell"

# --- A case ---------------------------------------------------------------------------------

# Waits up to $2 s for a line of perf log $1 starting with $3; prints it.
wait_mark() {
  local end=$(($(date +%s) + $2))
  while [ "$(date +%s)" -lt "$end" ]; do
    if [ -f "$1" ] && grep -q "^$3" "$1"; then
      grep "^$3" "$1" | head -1 | cut -f1
      return 0
    fi
    sleep 0.2
  done
  return 1
}

# A fingerprint of every file in app $1 (names and contents).
fingerprint() { (cd "$1" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1); }

# Starts a fresh install of the old app for case $1 with update key $2; sets case_dir, log,
# install, pid.
start_case() {
  case_dir=$out/case-$1
  rm -rf "$case_dir"
  mkdir -p "$case_dir/install" "$case_dir/config"
  cp -R "$out/old/gg.app" "$case_dir/install/"
  install=$case_dir/install/gg.app
  log=$case_dir/perf.log
  # The session to restore: two tabs, the second active.
  printf '{"session":{"tabs":["%s","%s"],"active":"%s"}}\n' \
    "$fixture/alpha" "$fixture/beta" "$fixture/beta" >"$case_dir/config/state.json"
  cat >"$case_dir/env" <<EOF
GG_CONFIG_DIR=$case_dir/config
GG_PERF_LOG=$log
GG_PERF_SCROLL=update
GG_PERF_LAUNCHES=$case_dir/launches.jsonl
GG_UPDATE_ENDPOINT=$base/latest.json
GG_UPDATE_PUBKEY=$(cat "$keys/$2.pub")
GIT_CONFIG_GLOBAL=$GIT_CONFIG_GLOBAL
GIT_CONFIG_NOSYSTEM=1
SHELL=$fixture/tools/fake-shell
PATH=$fixture/tools:/usr/bin:/bin
EOF
  served=$(wc -l <"$server_log")
  open -n -a "$install" --args --perf-env "$case_dir/env"
  pid=""
  for _ in $(seq 1 50); do
    pid=$(pgrep -n -f "^$(app_bin "$install")")
    [ -n "$pid" ] && break
    sleep 0.2
  done
  [ -n "$pid" ] || die "case $1: the app did not start"
}

# The server's log lines since the case started.
requests() { tail -n +"$((served + 1))" "$server_log"; }

stop_case() {
  for p in $(pgrep -f "^$case_dir/"); do kill "$p" 2>/dev/null; done
  for _ in $(seq 1 50); do pgrep -f "^$case_dir/" >/dev/null || break; sleep 0.1; done
  pid=""
}

# Case: nothing may be installed. $1 name, $2 key, $3 expected update-state prefix.
refused_case() {
  local name=$1 key=$2 want=$3 before seen
  start_case "$name" "$key"
  before=$(fingerprint "$install")
  seen=$(wait_mark "$log" 120 "update-final") || { fail "$name: the probe did not finish ($(grep '^update-' "$log" | cut -f1 | tr '\n' '|'))"; stop_case; return; }
  local states
  states=$(grep '^update-state' "$log" | cut -f1 | tr '\n' '|')
  if grep -q "^update-state $want" "$log"; then
    pass "$name: $(grep "^update-state $want" "$log" | head -1 | cut -f1)"
  else
    fail "$name: expected 'update-state $want', saw $states"
  fi
  sleep 1
  [ "$(fingerprint "$install")" = "$before" ] && pass "$name: the installed app is unchanged" ||
    fail "$name: the installed app changed"
  [ "$(plist "$install" CFBundleShortVersionString)" = "$old_version" ] && pass "$name: still $old_version" ||
    fail "$name: version is $(plist "$install" CFBundleShortVersionString)"
  [ "$(pgrep -f "^$(app_bin "$install")")" = "$pid" ] && pass "$name: no restart (same pid $pid)" ||
    fail "$name: the app restarted or quit"
  if [ "$want" = upToDate ]; then
    requests | grep -q 'GET /[a-z.-]*\.tar\.gz' && fail "$name: an update was downloaded" ||
      pass "$name: nothing was offered, nothing downloaded"
  else
    requests | grep -q 'GET /[a-z.-]*\.tar\.gz' && pass "$name: downloaded, then refused" ||
      fail "$name: the update was never downloaded"
  fi
  echo "      seen: $seen; states: $states"
  echo "      requests: $(requests | grep -o '"GET [^"]*"' | tr '\n' ' ')"
  stop_case
}

# --- The update, end to end -----------------------------------------------------------------

echo "update-proof: old $old_version, new $new_version, source $base/latest.json"
manifest "$new_version" gg.app.tar.gz
start_case update test
install_path=$install
old_pid=$pid
if seen=$(wait_mark "$log" 120 "update-restart-clicked"); then
  pass "update: started as $(grep '^native-version' "$log" | head -1 | cut -f1); $(grep '^update-tabs' "$log" | head -1 | cut -f1)"
  pass "update: states $(grep '^update-state' "$log" | cut -f1 | sed 's/update-state //' | tr '\n' '>' )"
  pass "update: $(grep '^update-notice' "$log" | head -1 | cut -f1), Restart to update clicked"
else
  fail "update: no restart ($(grep '^update-' "$log" | cut -f1 | tr '\n' '|'))"
fi
# The old process goes; a new one runs from the same path.
for _ in $(seq 1 100); do kill -0 "$old_pid" 2>/dev/null || break; sleep 0.2; done
kill -0 "$old_pid" 2>/dev/null && fail "update: the old app (pid $old_pid) did not quit" || pass "update: the old app (pid $old_pid) quit"
new_pid=""
for _ in $(seq 1 100); do
  new_pid=$(pgrep -n -f "^$(app_bin "$install_path")")
  [ -n "$new_pid" ] && [ "$new_pid" != "$old_pid" ] && break
  new_pid=""
  sleep 0.2
done
if [ -n "$new_pid" ]; then
  pass "update: a new process (pid $new_pid) runs $(ps -o command= -p "$new_pid" | cut -c1-200)"
else
  fail "update: nothing was started again"
fi
pid=$new_pid
# The version each run of the app is (the updater's own, logged by the perf build at start).
for _ in $(seq 1 300); do
  [ "$(grep -c '^native-version' "$log")" -ge 2 ] && break
  sleep 0.2
done
versions=$(grep '^native-version' "$log" | cut -f1 | sed 's/native-version //' | tr '\n' ' ')
[ "$versions" = "$old_version $new_version " ] && pass "update: the app ran as $old_version, then, restarted, as $new_version" ||
  fail "update: the app ran as: $versions"
# The restarted app marks its tabs once every tab of the session has reopened.
for _ in $(seq 1 300); do
  [ "$(grep -c '^update-tabs' "$log")" -ge 2 ] && break
  sleep 0.2
done
tabs=$(grep '^update-tabs' "$log" | sed -n 2p | cut -f1)
[ "$tabs" = "update-tabs alpha,beta active beta" ] && pass "update: state restored: $tabs" ||
  fail "update: state after the restart: '$tabs'"
if final=$(wait_mark "$log" 60 "update-final") && [ "$(grep '^update-final' "$log" | tail -1 | cut -f1)" = "update-final upToDate" ]; then
  pass "update: the new version checks again: up to date"
else
  fail "update: after the restart the check said '$(grep '^update-final' "$log" | tail -1 | cut -f1)'"
fi
[ "$(plist "$install_path" CFBundleShortVersionString)" = "$new_version" ] &&
  pass "update: $install_path says $new_version" || fail "update: the bundle says $(plist "$install_path" CFBundleShortVersionString)"
codesign --verify --deep --strict "$install_path" 2>/dev/null &&
  pass "update: the installed app's code signature verifies (codesign --verify --deep --strict)" ||
  fail "update: the installed app does not verify"
[ "$(fingerprint "$install_path")" = "$(fingerprint "$out/new/gg.app")" ] &&
  pass "update: the installed app is file for file the new build" || fail "update: the installed app differs from the new build"
[ -x "$install_path/Contents/Resources/gg" ] && [ "$("$install_path/Contents/Resources/gg" --version)" = "gg $new_version" ] &&
  pass "update: the gg launcher inside says gg $new_version" || fail "update: the launcher inside is missing or wrong"
echo "      requests: $(requests | grep -o '"GET [^"]*"' | tr '\n' ' ')"
[ -f "$fixture/blocked.log" ] && fail "update: something tried the network: $(cat "$fixture/blocked.log")"
stop_case

# --- Nothing installed ----------------------------------------------------------------------

manifest "$new_version" tampered.tar.gz
refused_case tampered test "failed download Could not download gg $new_version. Its signature does not match"
manifest "$new_version" gg.app.tar.gz
refused_case wrong-key other "failed download Could not download gg $new_version. Its signature does not match"
manifest "$new_version" other-version.tar.gz
refused_case signed-for-another-version test "failed download Could not download gg $new_version. Its signature does not match"
manifest "$old_version" gg.app.tar.gz
refused_case same-version test "upToDate"
manifest 0.3.8 gg.app.tar.gz
refused_case older-version test "upToDate"

echo
if [ "$failures" -eq 0 ]; then
  echo "update-proof: every check passed"
else
  echo "update-proof: $failures check(s) failed"
  exit 1
fi
