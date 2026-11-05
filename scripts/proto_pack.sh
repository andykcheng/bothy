#!/usr/bin/env bash
# Milestone 1: wait for embed.py, build packs/proto, run the golden-query eval, copy to BigBelly.
# Run detached: setsid nohup scripts/proto_pack.sh > ~/sp-scratch/packs/proto_pack.log 2>&1 &
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
B=${B:-$HOME/sp-scratch/build/proto}
P=${P:-$HOME/sp-scratch/packs/proto}
GGUF=${GGUF:-$(ls $HOME/.cache/huggingface/hub/models--Qwen--Qwen3-Embedding-0.6B-GGUF/snapshots/*/Qwen3-Embedding-0.6B-Q8_0.gguf)}
SRCS="wikimed-en wikimed-zh wikimed-ja mdwiki-en"
for s in $SRCS; do
  until [ -f "$B/$s/emb/manifest.json" ]; do sleep 60; done
done
echo "== $(date +%T) embeddings present"
rm -rf "$P"
"$REPO/core/target/release/sp" build --out "$P" --model "$GGUF" $(for s in $SRCS; do echo "$B/$s"; done)
echo "== $(date +%T) eval"
cd "$REPO/ingest"
uv run python eval_search.py "$P" --out "$P/../proto_eval.json" > "$P/../proto_eval.txt"
echo "== $(date +%T) copying to BigBelly"
mkdir -p /mnt/bigbelly/survivalpedia/packs
rsync -a "$P" /mnt/bigbelly/survivalpedia/packs/
rsync -a "$B/" /mnt/bigbelly/survivalpedia/build/proto/
echo "DONE $(date +%T)"
