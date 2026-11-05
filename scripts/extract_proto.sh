#!/usr/bin/env bash
# Milestone 1, step 1: extract the four prototype ZIMs into NVMe scratch, then copy to BigBelly.
# Run detached: setsid nohup scripts/extract_proto.sh > ~/sp-scratch/build/proto/extract.log 2>&1 &
set -euo pipefail
SRC=${SRC:-$HOME/sp-scratch/sources}
OUT=${OUT:-$HOME/sp-scratch/build/proto}
cd "$(dirname "$0")/../ingest"
mkdir -p "$OUT"
run() {  # zim source lang
  echo "== $(date +%T) $2 ($3)"
  uv run python zim_extract.py "$SRC/$1" "$OUT/$2-$3" --source "$2" --lang "$3"
}
run wikipedia_en_medicine_maxi_2026-10.zim wikimed en
run wikipedia_zh_medicine_maxi_2026-09.zim wikimed zh
run wikipedia_ja_medicine_maxi_2026-10.zim wikimed ja
run mdwiki_en_all_maxi_2025-11.zim mdwiki en
echo "== $(date +%T) copying to BigBelly"
rsync -a "$OUT/" /mnt/bigbelly/survivalpedia/build/proto/
echo "DONE $(date +%T)"
