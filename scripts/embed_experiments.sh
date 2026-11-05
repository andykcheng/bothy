#!/usr/bin/env bash
# Embedding speed/quality experiments on wikimed-ja, run once the GPU is free (after proto_pack.sh).
# Run detached: setsid nohup scripts/embed_experiments.sh > ~/sp-scratch/packs/embed_exp.log 2>&1 &
set -uo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
B=$HOME/sp-scratch/build/proto
until grep -q '^DONE' $HOME/sp-scratch/packs/proto_pack.log 2>/dev/null; do sleep 60; done
cd "$REPO/ingest"
run() { echo "== $(date +%T) $*"; uv run python embed.py "$B/wikimed-ja" "$@" 2>&1 | grep '^{'; }
# speed on an idle GPU (torch, full chunks) as the reference
run --out-name exp-torch-full
run --out-name exp-torch-t128 --max-tokens 128
run --out-name exp-http-full --engine http --concurrency 4
run --out-name exp-http-full-c8 --engine http --concurrency 8
# quality: build ja-only packs per variant, evaluate on the ja synthetic + golden sets
GG=$(ls $HOME/.cache/huggingface/hub/models--Qwen--Qwen3-Embedding-0.6B-GGUF/snapshots/*/Qwen3-Embedding-0.6B-Q8_0.gguf)
port=8120
for v in exp-torch-full exp-torch-t128 exp-http-full; do
  P=$HOME/sp-scratch/packs/ja-$v
  rm -rf "$P"
  "$REPO/core/target/release/sp" build --out "$P" --model "$GG" --emb-name $v "$B/wikimed-ja" > /dev/null 2>&1 || { echo "build $v failed"; continue; }
  "$REPO/core/target/release/sp-server" "$P" --listen 127.0.0.1:$port > /dev/null 2>&1 &
  pid=$!; sleep 5
  echo "== quality $v"
  uv run python eval_search.py --server http://127.0.0.1:$port --golden ../eval/synth-wikimed-ja.jsonl --quiet \
    --out "$HOME/sp-scratch/packs/exp-$v.json" 2>/dev/null | python3 -c "
import json,sys; s=json.load(sys.stdin)
for m,x in s['overall'].items(): print('  ',m,x)"
  kill $pid; port=$((port+1))
done
echo "DONE $(date +%T)"
