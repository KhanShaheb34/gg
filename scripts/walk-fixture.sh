#!/usr/bin/env bash
# Makes the throwaway repository the walk (and the actions probe) runs on, in a new folder
# under <parent>, and prints the path of its work tree:
#
#   scripts/walk-fixture.sh [--github] <parent>
#
# <parent>/walkrepo.XXXX/work: `main` checked out; `staging` 2 behind `origin/staging`;
# `feature/login` from staging; `clash` and `topic` change `app.txt` like main does (a merge
# and a rebase that conflict); `side`; a tag `v1`. origin.git, a bare repository beside it, is
# its `origin`; other/ is a second clone that pushed to it. Auto-maintenance is off.
#
# --github: `origin` is https://github.com/octo/cat.git to the app, which git's `insteadOf`
# sends to origin.git (pushes stay on disk), so the app offers pull requests. Only for a run
# whose `gh` is a fake (scripts/walk.sh --mode walk3): the app would otherwise ask the real one.
#
# The work tree's git dir gets `gg-perf-fixture`, holding the work tree's path: scripts/perf.sh
# runs the probes that change a repository (walk, actions) only on a repository marked so,
# never on one of the user's.
set -euo pipefail
github=0
if [ "${1-}" = --github ]; then github=1; shift; fi
parent=$(cd "$1" && pwd)
W=$(mktemp -d "$parent/walkrepo.XXXX")
g() { git -C "$W/work" -c user.name="Ada Lovelace" -c user.email=ada@example.invalid -c commit.gpgsign=false "$@"; }
o() { git -C "$W/other" -c user.name="Grace Hopper" -c user.email=grace@example.invalid -c commit.gpgsign=false "$@"; }
c() { g add -A && g commit -q -m "$1"; }
git init -q --bare -b main "$W/origin.git"
git init -q -b main "$W/work"
g config maintenance.auto false; g config gc.auto 0
g config user.name "Ada Lovelace"; g config user.email ada@example.invalid
echo "app base" > "$W/work/app.txt"; echo "main notes" > "$W/work/notes.txt"; c "Initial commit"
g tag v1
if [ "$github" = 1 ]; then
  g remote add origin https://github.com/octo/cat.git
  g config "url.$W/origin.git.insteadOf" https://github.com/octo/cat.git
else
  g remote add origin "$W/origin.git"
fi
g checkout -q -b staging; echo "staging notes" > "$W/work/notes.txt"; echo staging > "$W/work/staging.txt"; c "Prepare staging"
g push -q -u origin main staging
g checkout -q -b feature/login staging; echo login > "$W/work/login.txt"; c "Add login form"
echo valid >> "$W/work/login.txt"; c "Validate login"; g push -q -u origin feature/login
git clone -q "$W/origin.git" "$W/other"
o checkout -q staging; echo one > "$W/other/hotfix1.txt"; o add -A; o commit -q -m "Hotfix one"
echo two > "$W/other/hotfix2.txt"; o add -A; o commit -q -m "Hotfix two"; o push -q origin staging
o checkout -q -b remote-only main; echo remote > "$W/other/remote.txt"; o add -A; o commit -q -m "Remote work"
o push -q origin remote-only
g fetch -q origin
g checkout -q -b side main; echo side > "$W/work/side.txt"; c "Side note"
g checkout -q -b clash main; echo "app from clash" > "$W/work/app.txt"; c "Clash on app"
g checkout -q -b topic main; echo "app from topic" > "$W/work/app.txt"; c "Topic on app"
g checkout -q main; echo "app from main" > "$W/work/app.txt"; c "Rework app on main"; g push -q origin main
work=$(cd "$W/work" && pwd)
printf '%s\n' "$work" > "$W/work/.git/gg-perf-fixture"
echo "$work"
