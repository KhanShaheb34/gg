#!/bin/sh
# Deploys the site. Refuses to publish without the download that latest.json names,
# and fetches it from the live site when it is missing here (it is not kept in git).
# Refuses too when the changelog page differs from changelog.md, or has no entry for the
# version in latest.json.
set -eu
cd "$(dirname "$0")/.."
node scripts/changelog.mjs --check
file=$(sed -n 's/.*"file":"\([^"]*\)".*/\1/p' public/latest.json)
want=$(sed -n 's/.*"sha256":"\([^"]*\)".*/\1/p' public/latest.json)
[ -n "$file" ] && [ -n "$want" ] || { echo "deploy: latest.json names no file or checksum" >&2; exit 1; }
mkdir -p public/download
if [ ! -f "public/download/$file" ]; then
  echo "deploy: public/download/$file is missing here; fetching the published copy"
  curl -fsSL -o "public/download/$file" "https://gg.shakirul.com/download/$file"
fi
have=$(shasum -a 256 "public/download/$file" | cut -d' ' -f1)
[ "$have" = "$want" ] || { echo "deploy: public/download/$file does not match latest.json (sha256 $have)" >&2; exit 1; }
exec npx --yes wrangler@4 deploy "$@"
