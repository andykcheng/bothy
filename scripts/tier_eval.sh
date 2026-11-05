#!/usr/bin/env bash
# Evaluate install choices on a tiered pack: which vector components are present.
# Each scenario is a symlink view of the pack holding only the chosen index/vec components.
# Run detached: setsid nohup scripts/tier_eval.sh PACK > ~/sp-scratch/packs/tier_eval.log 2>&1 &
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
PACK=$(realpath "$1")
OUT=$(dirname "$PACK")/tier-views
port=8140
for sc in "all:*.d256" "all128:*.d128" "core:*.core.d256" "core128:*.core.d128"; do
  name=${sc%%:*}; pat=${sc#*:}
  V=$OUT/$name
  rm -rf "$V"; mkdir -p "$V/index/vec"
  for f in "$PACK"/*; do [ "$(basename "$f")" = index ] || ln -s "$f" "$V/"; done
  for f in "$PACK"/index/bm25-*; do ln -s "$f" "$V/index/"; done
  for f in "$PACK"/index/vec/$pat; do ln -s "$f" "$V/index/vec/"; done
  "$REPO/core/target/release/sp-server" "$V" --listen 127.0.0.1:$port > /dev/null 2>&1 &
  pid=$!
  until curl -s localhost:$port/api/info > /dev/null; do sleep 1; done
  echo "== $(date +%T) $name: $(ls "$V/index/vec" | tr '\n' ' ')"
  (cd "$REPO/ingest" && uv run python eval_search.py --server http://127.0.0.1:$port --modes dense,hybrid \
    --golden ../eval/synth-wikimed-*.jsonl ../eval/golden.jsonl --quiet --out "$OUT/$name.json" > /dev/null)
  kill $pid; port=$((port+1))
done
echo "DONE $(date +%T)"
