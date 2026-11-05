#!/usr/bin/env bash
# Build WikiMed en/zh/ja packs (no mdwiki) at 256 and 128 vector dims and evaluate both.
# Run detached: setsid nohup scripts/dims_compare.sh > ~/sp-scratch/packs/dims_compare.log 2>&1 &
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
B=$HOME/sp-scratch/build/proto
GG=$(ls $HOME/.cache/huggingface/hub/models--Qwen--Qwen3-Embedding-0.6B-GGUF/snapshots/*/Qwen3-Embedding-0.6B-Q8_0.gguf)
port=8130
for d in 256 128; do
  P=$HOME/sp-scratch/packs/wikimed-d$d
  rm -rf "$P"
  echo "== $(date +%T) build d$d"
  "$REPO/core/target/release/sp" build --out "$P" --model "$GG" --dims $d "$B/wikimed-en" "$B/wikimed-zh" "$B/wikimed-ja" 2>&1 | grep -E 'vec:|done in'
  "$REPO/core/target/release/sp-server" "$P" --listen 127.0.0.1:$port > /dev/null 2>&1 &
  pid=$!
  until curl -s localhost:$port/api/info > /dev/null; do sleep 1; done
  echo "== $(date +%T) eval d$d"
  (cd "$REPO/ingest" && uv run python eval_search.py --server http://127.0.0.1:$port \
    --golden ../eval/synth-wikimed-*.jsonl ../eval/golden.jsonl --quiet --out "$P/../wikimed-d$d-eval.json" > /dev/null)
  kill $pid; port=$((port+1))
done
echo "DONE $(date +%T)"
