#!/usr/bin/env bash
# Generates a synthetic git repository for performance measurements.
#
#   scripts/gen-synthetic-repo.sh <dir> <commits> <branches>
#
# <dir> must not exist yet. The repository gets exactly <commits> commits and <branches>
# local branches (`main` plus `feature/1` ... `feature/<branches-1>`), written in one
# `git fast-import` run, so even 100,000 commits take seconds and the pack stays small.
#
# Shape, meant to look like a busy team repository rather than a straight line:
# - a dozen or more feature branches in flight at once, each forked from `main`;
# - a `--no-ff`-style merge into `main` every 30-50 commits; after a merge the branch is
#   either retired (its ref stays on the merged commit) or keeps going and is merged again;
# - 8 authors in different time zones (two with non-ASCII names);
# - subjects from 3 to about 110 characters, some with a body;
# - an annotated release tag on `main` every ~200 commits;
# - remote `origin` (pointing at the repository itself) with remote-tracking branches for
#   `main` and every other feature branch, some of them a commit behind.
#
# The output is deterministic: the same arguments always produce the same commit hashes
# (a fixed-seed generator, fixed dates and identities, and git config isolated from the
# host).
set -euo pipefail

if [ $# -ne 3 ]; then
  echo "usage: $0 <dir> <commits> <branches>" >&2
  exit 2
fi
dir=$1
commits=$2
branches=$3
case "$commits$branches" in
  *[!0-9]*) echo "commits and branches must be positive integers" >&2; exit 2 ;;
esac
if [ "$commits" -lt 2 ] || [ "$branches" -lt 2 ]; then
  echo "need at least 2 commits and 2 branches" >&2
  exit 2
fi
if [ -e "$dir" ]; then
  echo "$dir already exists" >&2
  exit 1
fi

# Keep the host's git config, identity and dates out of it.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_AUTHOR_DATE \
  GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL GIT_COMMITTER_DATE
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C

mkdir -p "$dir"
git -C "$dir" init -q -b main
git -C "$dir" config commit.gpgsign false
git -C "$dir" remote add origin "$(cd "$dir" && pwd)"

LC_ALL=C awk -v N="$commits" -v B="$branches" '
# Park-Miller minimal standard generator: exact in double precision, same on every awk.
function rnd(n) { seed = (seed * 16807) % 2147483647; return seed % n }

function data(s) { printf "data %d\n%s\n", length(s), s }

function subject(   s, r) {
  r = rnd(10)
  if (r == 0) return words[rnd(nwords)]                     # "wip", "typo" ...
  s = verbs[rnd(nverbs)] " " objects[rnd(nobjects)]
  if (r <= 3) s = prefixes[rnd(nprefixes)] "(" areas[rnd(nareas)] "): " s
  else if (r <= 5) s = s " in " areas[rnd(nareas)]
  if (rnd(3) == 0) s = s " " details[rnd(ndetails)]
  if (rnd(6) == 0) s = s " (#" (1000 + rnd(9000)) ")"
  return s
}

function message(s) {
  if (rnd(5) == 0) s = s "\n\n" bodies[rnd(nbodies)]
  return s
}

# Writes one commit on `ref` with first parent mark `from` (0 = root) and optional merge
# parent mark `merge`; returns its mark.
function commit(ref, from, merge, msg,   a, t) {
  mark++
  a = rnd(nauthors)
  clock += 120 + rnd(1800)
  t = clock " " zones[a]
  printf "commit %s\nmark :%d\n", ref, mark
  printf "author %s %s\ncommitter %s %s\n", authors[a], t, authors[a], t
  data(message(msg))
  if (from) printf "from :%d\n", from
  if (merge) printf "merge :%d\n", merge
  printf "M 100644 inline src/part%d.txt\n", mark % 8
  data("revision " mark)
  print ""
  parent[mark] = from
  if (ref == "refs/heads/main") {
    sinceTag++
    if (sinceTag >= 200) { sinceTag = 0; tag(mark) }
  }
  return mark
}

function tag(m,   name) {
  release++
  name = "v" int(release / 100) "." int(release / 10) % 10 "." release % 10
  printf "tag %s\nfrom :%d\ntagger %s %d +0000\n", name, m, authors[0], clock
  data("Release " name)
}

# Starts feature branch number `n` in active slot `s`, forked from main.
function start(s, n) {
  slotName[s] = "feature/" n
  slotTip[s] = mainTip
  slotWork[s] = 0
}

function setRef(ref, m) { printf "reset %s\nfrom :%d\n\n", ref, m }

BEGIN {
  seed = 20261112
  clock = 1600000000

  nauthors = split("Ada Lovelace <ada@example.com>|Grace Hopper <grace@example.com>|" \
    "Linus Pauling <linus@example.org>|Zo\303\253 Fischer <zoe@example.de>|" \
    "Kenji Watanabe <kenji@example.jp>|Priya Raman <priya@example.in>|" \
    "Jos\303\251 Alvarez <jose@example.es>|Sam O\047Neil <sam@example.ie>", tmp, "|")
  for (i = 1; i <= nauthors; i++) authors[i - 1] = tmp[i]
  split("-0800|-0500|+0000|+0100|+0900|+0530|+0200|+0000", tmp, "|")
  for (i = 1; i <= nauthors; i++) zones[i - 1] = tmp[i]

  nverbs = split("Add|Fix|Remove|Refactor|Update|Rename|Simplify|Document|Speed up|" \
    "Handle|Validate|Cache|Split|Inline|Revert|Tidy", tmp, "|")
  for (i = 1; i <= nverbs; i++) verbs[i - 1] = tmp[i]
  nobjects = split("the parser|login flow|error message|retry logic|config loader|" \
    "date formatting|search index|the toolbar|keyboard shortcuts|unit tests|" \
    "dependency versions|README|scroll position|memory leak|race on shutdown|" \
    "empty state|dark theme colours|CSV export|pagination|rate limiter", tmp, "|")
  for (i = 1; i <= nobjects; i++) objects[i - 1] = tmp[i]
  nareas = split("api|ui|core|db|auth|build|docs|cli|net|store", tmp, "|")
  for (i = 1; i <= nareas; i++) areas[i - 1] = tmp[i]
  nprefixes = split("feat|fix|chore|refactor|perf|test", tmp, "|")
  for (i = 1; i <= nprefixes; i++) prefixes[i - 1] = tmp[i]
  ndetails = split("when the list is empty|for very long file names on Windows|" \
    "after the window regains focus|so that the cache is not rebuilt on every request|" \
    "behind a feature flag|and add a regression test", tmp, "|")
  for (i = 1; i <= ndetails; i++) details[i - 1] = tmp[i]
  nwords = split("wip|typo|lint|fmt|bump", tmp, "|")
  for (i = 1; i <= nwords; i++) words[i - 1] = tmp[i]
  nbodies = split("The old code assumed a non-empty list.|" \
    "Measured on the large fixture: no change in the common case.|" \
    "Reported by a user; see the linked issue for the reproduction steps.\n\nCloses #42.", \
    tmp, "|")
  for (i = 1; i <= nbodies; i++) bodies[i - 1] = tmp[i]

  features = B - 1
  active = 12 + int(features / 50)
  if (active > features) active = features
  # A merge every ~40 commits; retire branches at the rate that uses up every name.
  merges = int(N / 40)
  retire = merges > 0 ? (features - active) / merges : 0

  mainTip = commit("refs/heads/main", 0, 0, "Initial commit")
  made = 1
  nextName = 1
  for (s = 0; s < active; s++) start(s, nextName++)
  untilMerge = 30 + rnd(21)

  while (made < N) {
    untilMerge--
    if (untilMerge <= 0) {
      s = rnd(active)
      if (slotWork[s] > 0) {
        mainTip = commit("refs/heads/main", mainTip, slotTip[s], \
          rnd(2) ? "Merge branch \047" slotName[s] "\047" \
                 : "Merge pull request #" (100 + made) " from team/" slotName[s])
        made++
        untilMerge = 30 + rnd(21)
        finalTip[slotName[s]] = slotTip[s]
        if (nextName <= features && rnd(1000000) < retire * 1000000) start(s, nextName++)
        else slotWork[s] = 0
        continue
      }
    }
    if (rnd(4) == 0) {
      mainTip = commit("refs/heads/main", mainTip, 0, subject())
    } else {
      s = rnd(active)
      slotTip[s] = commit("refs/heads/" slotName[s], slotTip[s], 0, subject())
      slotWork[s]++
      finalTip[slotName[s]] = slotTip[s]
    }
    made++
  }

  # Names never reached get a branch on a commit spread over history.
  for (; nextName <= features; nextName++) {
    finalTip["feature/" nextName] = 1 + rnd(mark)
    setRef("refs/heads/feature/" nextName, finalTip["feature/" nextName])
  }
  # Branches made but never committed on still need their ref.
  for (s = 0; s < active; s++) if (!(slotName[s] in finalTip)) {
    finalTip[slotName[s]] = slotTip[s]
    setRef("refs/heads/" slotName[s], slotTip[s])
  }

  # Remote-tracking branches: origin/main two commits behind, every other feature branch
  # tracked, every fifth tracked one a commit behind.
  m = mainTip
  for (k = 0; k < 2 && parent[m]; k++) m = parent[m]
  setRef("refs/remotes/origin/main", m)
  for (n = 1; n <= features; n += 2) {
    m = finalTip["feature/" n]
    if (n % 5 == 0 && parent[m]) m = parent[m]
    setRef("refs/remotes/origin/feature/" n, m)
  }
}
' | git -C "$dir" fast-import --quiet

git -C "$dir" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main
git -C "$dir" reset -q --hard main

echo "$dir: $(git -C "$dir" rev-list --all --count) commits," \
  "$(git -C "$dir" for-each-ref refs/heads | wc -l | tr -d ' ') branches," \
  "$(git -C "$dir" for-each-ref refs/tags | wc -l | tr -d ' ') tags," \
  "$(git -C "$dir" for-each-ref refs/remotes | wc -l | tr -d ' ') remote refs"
