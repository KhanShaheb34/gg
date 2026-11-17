#!/bin/sh
# Checks that the tracked files are fit to publish. Fails (exit 1), listing file:line, if any
# tracked text file contains:
#
#   home   an absolute home path of a real user (/Users/<name>, /home/<name>), other than the
#          fixture names in HOME_ALLOW
#   email  an email address, other than noreply addresses and the reserved example domains
#   token  a token-looking string (GitHub tokens, sk- keys, AWS key ids, private-key headers)
#          outside the test fixtures in TOKEN_ALLOW_PATHS
#   name   the retired names "GitGraph", "Git Graph", "git-graph" or "gitgraph" (any case),
#          outside the README's one credit line and the lines in NAME_ALLOW
#   notes  pointers to internal planning material that is not published (docs/superpowers,
#          docs/perf, plan and review-round references such as "Task 9 review" or "final
#          review M3"), and tool attribution lines (Co-Authored-By, "Generated with")
#
#   scripts/audit-public.sh
#
# Binary files are skipped. Exit 0 with "audit-public: clean" when nothing is found.
set -eu

# Fixture user names that may appear in home paths (tests, examples).
HOME_ALLOW="ada adam a ab me you runner Shared"
# Files whose token-looking strings are test fixtures (fake credentials that tests check
# are masked).
TOKEN_ALLOW_PATHS="crates/gg-core/src/git/runner.rs crates/gg-core/src/hosts.rs crates/gg-core/tests/gh_tokens.rs"
# Lines that may name the retired app name, as "path|text the line contains": the code that
# moves the settings over from the old app's config folder, which has that name.
NAME_ALLOW="crates/gg-core/src/store.rs|const OLD_CONFIG_DIR_NAME: &str = \"dev.gitgraph.desktop\";"
# The one README line that may name the extension gg is inspired by.
README_CREDIT="inspired by the Git Graph extension"

SELF=scripts/audit-public.sh

root=$(git rev-parse --show-toplevel)
cd "$root"

# Prints "path:line:match" for every match of ERE $1 in tracked text files, except this one.
matches() {
  git grep -n -o -I -E -e "$1" -- . ":(exclude)$SELF" || true
}

found=$(
  {
    # A path that starts there: not "/config/home/x", a folder that only ends in "home".
    matches '(^|[^A-Za-z0-9._/-])/(Users|home)/[A-Za-z0-9._-]+' |
      awk -v allow="$HOME_ALLOW" '
        BEGIN { n = split(allow, a, " "); for (i = 1; i <= n; i++) ok[a[i]] = 1 }
        {
          loc = $0; match(loc, /^[^:]*:[0-9]+:/); m = substr(loc, RLENGTH + 1)
          sub(/^[^\/]/, "", m)
          name = m; sub(/^\/(Users|home)\//, "", name)
          if (!(name in ok)) print "home  " substr(loc, 1, RLENGTH) m
        }'

    matches '(^|[^A-Za-z0-9._%+_-])[A-Za-z0-9._%+_-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}' |
      awk '
        {
          loc = $0; match(loc, /^[^:]*:[0-9]+:/); m = substr(loc, RLENGTH + 1)
          lead = substr(m, 1, 1)
          # Userinfo in a URL (https://user:token@host) is not an email; the token check
          # covers what it may hold.
          if (lead == ":" || lead == "/") next
          if (lead !~ /[A-Za-z0-9._%+_-]/) m = substr(m, 2)
          at = index(m, "@"); user = substr(m, 1, at - 1); host = tolower(substr(m, at + 1))
          if (user ~ /noreply$/ || host ~ /(^|\.)noreply\./) next
          if (user == "git") next                              # git@host:owner/repo (SSH)
          if (host ~ /^[0-9]+x\./) next                         # icon@2x.png
          if (host ~ /^example\.[a-z]+$/) next                  # example.com, .org, .de, ...
          if (host ~ /\.(invalid|test|example|localhost)$/) next
          print "email " substr(loc, 1, RLENGTH) m
        }'

    matches '(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9_]+|github_pat_[A-Za-z0-9_]+|(^|[^A-Za-z0-9_-])sk-[A-Za-z0-9_-]{16,}|(AKIA|ASIA)[0-9A-Z]{16}|-----BEGIN [A-Z ]*PRIVATE KEY-----' |
      awk -v allow="$TOKEN_ALLOW_PATHS" '
        BEGIN { n = split(allow, a, " "); for (i = 1; i <= n; i++) ok[a[i]] = 1 }
        { p = $0; sub(/:.*/, "", p); if (!(p in ok)) print "token " $0 }'

    { git grep -n -I -i -E -e 'git ?graph|git-graph|git_graph' -- . ":(exclude)$SELF" || true; } |
      awk -v allow="$NAME_ALLOW" -v credit="$README_CREDIT" '
        BEGIN { split(allow, a, "|"); okpath = a[1]; oktext = a[2] }
        {
          p = $0; sub(/:.*/, "", p)
          if (p == okpath && index($0, oktext) > 0) next
          if (p == "README.md" && index($0, credit) > 0) next
          print "name  " substr($0, 1, 160)
        }'

    # .gitignore names the planning tool's scratch folder only to keep it out of the tree.
    { git grep -n -I -E -e 'docs/superpowers|docs/perf\b|\.superpowers|[Tt]ask [0-9]+ review|[Ff]inal (boundary )?review [A-Z][0-9]|[Rr]e-review [A-Z][0-9]|[Pp]lan [123]\b|[Cc]o-[Aa]uthored-[Bb]y|[Gg]enerated with' \
        -- . ":(exclude)$SELF" ":(exclude).gitignore" || true; } |
      awk '{ print "notes " substr($0, 1, 160) }'
  }
)

if [ -n "$found" ]; then
  printf '%s\n' "$found"
  count=$(printf '%s\n' "$found" | wc -l | tr -d ' ')
  echo "audit-public: $count finding(s)" >&2
  exit 1
fi
echo "audit-public: clean"
