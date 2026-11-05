#!/usr/bin/env bash
# Extract a map region listed in maps/regions.json from the remote Protomaps build,
# then build its place-search index (<file>.places/).
# Usage: scripts/map_extract.sh REGION_ID [OUT_DIR]   (run detached for big regions)
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
ID=$1; OUT=${2:-$HOME/sp-scratch/maps}
mkdir -p "$OUT"
eval "$(python3 - "$REPO/maps/regions.json" "$ID" <<'PY'
import json, sys, shlex
c = json.load(open(sys.argv[1])); r = next(r for r in c["regions"] if r["id"] == sys.argv[2])
print(f'URL={shlex.quote(c["source"].format(build=c["build"]))}')
print(f'BBOX={",".join(map(str, r["bbox"]))}')
print(f'TITLE={shlex.quote(r["title"])}')
print(f'MAXZ={r.get("max_zoom", 15)}')
print(f'BUILD={c["build"]}')
PY
)"
echo "== $(date +%T) $ID $BBOX z0-$MAXZ from $URL"
"$REPO/core/target/release/sp" map-extract --url "$URL" --bbox="$BBOX" --max-zoom "$MAXZ" \
  --region "$ID" --title "$TITLE" "$OUT/$ID-$BUILD.pmtiles"
echo "== $(date +%T) place index"
"$REPO/core/target/release/sp" map-places "$OUT/$ID-$BUILD.pmtiles"
echo "DONE $(date +%T)"
