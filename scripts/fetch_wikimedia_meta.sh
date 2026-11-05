#!/usr/bin/env bash
# Wikipedia metadata (SQL dumps) + monthly pageviews, for topic labelling and importance scoring.
# dumps.wikimedia.org allows few connections per IP: download sequentially, one connection.
set -uo pipefail
DEST=${DEST:-/mnt/bigbelly/survivalpedia/sources/wikimedia}
mkdir -p "$DEST"; cd "$DEST"
get() { [ -f "$2.ok" ] && { echo "SKIP $2"; return; }; echo "GET $2 $(date +%T)"
  curl -sSLf -C - --retry 10 --retry-delay 30 -o "$2" "$1" && touch "$2.ok" || echo "FAILED $2"; }
for w in enwiki; do  # English only (DESIGN §1)
  for t in page page_props redirect categorylinks linktarget pagelinks; do
    get "https://dumps.wikimedia.org/$w/latest/$w-latest-$t.sql.gz" "$w-latest-$t.sql.gz"
  done
done
# 12 months of user pageviews (all projects/languages in one file per month)
for m in 2025-10 2025-11 2025-12 2026-01 2026-02 2026-03 2026-04 2026-05 2026-06 2026-07 2026-08 2026-09; do
  y=${m%-*}; f="pageviews-${m/-/}-user.bz2"
  get "https://dumps.wikimedia.org/other/pageview_complete/monthly/$y/$m/$f" "$f"
done
echo "DONE $(date +%T)"
