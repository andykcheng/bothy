"""Cosine agreement between torch (embed.py) vectors and the GGUF query embedder (sp embed).

    uv run python check_gguf.py <extract_dir> <gguf> [--sp ../core/target/release/sp] [-n 20]
"""
import argparse, json, subprocess, sys
from pathlib import Path
import numpy as np
import orjson

ap = argparse.ArgumentParser()
ap.add_argument("extract_dir")
ap.add_argument("gguf")
ap.add_argument("--sp", default=str(Path(__file__).resolve().parent.parent / "core/target/release/sp"))
ap.add_argument("-n", type=int, default=20)
ap.add_argument("--queries", nargs="*", default=["how to stop bleeding", "止血の方法", "如何治疗烧伤", "signs of dehydration in children"])
a = ap.parse_args()

d = Path(a.extract_dir)
full = np.load(d / "emb/full_f16.npy", mmap_mode="r")
rng = np.random.default_rng(0)
pick = sorted(rng.choice(full.shape[0], a.n, replace=False))
texts, want = [], set(int(i) for i in pick)
with open(d / "chunks.jsonl", "rb") as f:
    for i, line in enumerate(f):
        if i in want:
            texts.append(orjson.loads(line)["text"])
out = subprocess.run([a.sp, "embed", "--model", a.gguf, *texts], check=True, capture_output=True, text=True).stdout
g = np.array(json.loads(out), dtype=np.float32)
t = full[pick].astype(np.float32)
cos = (g * t).sum(1) / np.linalg.norm(t, axis=1)
print(f"docs   n={len(cos)} cos min={cos.min():.4f} mean={cos.mean():.4f}")

sys.path.insert(0, str(Path(__file__).parent))
import embed
e = embed.Embedder(device="cpu")  # the GPU may be busy with embed.py
tq = e.encode([embed.QUERY_INSTRUCTION + q for q in a.queries])
out = subprocess.run([a.sp, "embed", "--model", a.gguf, *[embed.QUERY_INSTRUCTION + q for q in a.queries]],
                     check=True, capture_output=True, text=True).stdout
gq = np.array(json.loads(out), dtype=np.float32)
cq = (gq * tq).sum(1)
for q, c in zip(a.queries, cq):
    print(f"query  cos={c:.4f}  {q}")
# 256-dim truncated agreement, the dims the index actually uses
tr = lambda v: v[:, :embed.DIMS] / np.linalg.norm(v[:, :embed.DIMS], axis=1, keepdims=True)
print(f"256d   docs mean={(tr(g) * tr(t)).sum(1).mean():.4f}  queries mean={(tr(gq) * tr(tq)).sum(1).mean():.4f}")
