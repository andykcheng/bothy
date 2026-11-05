"""Lead embeddings of every Wikipedia article, taken from the full-text embedding shards (embed.py), for the
essentials classifier (classify_all.py). An article's first chunk (#0) is its title, infobox and opening text,
much like what Jev scored (leads.py), so nothing needs embedding again.

  uv run python essentials/shard_leads.py <extract dir> <docs.tsv> <out dir> [--dims 512]

<docs.tsv> is docs_table.py's table (page_id, title). For each finished shard <extract>/emb/shards/NNNNN/ it writes
<out>/NNNNN.npz with `ids` (page ids) and `emb` (float16, the first --dims dimensions, L2-normalised again: the
model is Matryoshka-trained). Shards already done are skipped, so rerun it as more shards finish.
"""
import argparse, os
from pathlib import Path

import numpy as np

ap = argparse.ArgumentParser()
ap.add_argument("extract"); ap.add_argument("docs"); ap.add_argument("out")
ap.add_argument("--dims", type=int, default=512)
a = ap.parse_args()

out = Path(a.out); out.mkdir(parents=True, exist_ok=True)
shards = sorted(p for p in (Path(a.extract) / "emb" / "shards").iterdir() if (p / "meta.json").exists())
todo = [s for s in shards if not (out / f"{s.name}.npz").exists()]
print(f"{len(shards)} finished shards, {len(todo)} to do", flush=True)
if not todo:
    raise SystemExit

pid = {}
with open(a.docs, encoding="utf8") as f:
    next(f)
    for line in f:
        r = line.rstrip("\n").split("\t")
        pid[r[2].replace(" ", "_")] = int(r[1])
print(f"{len(pid):,} articles in {a.docs}", flush=True)

missing = 0
for s in todo:
    rows, ids = [], []
    with open(s / "chunk_ids.txt", encoding="utf8") as f:
        for i, line in enumerate(f):
            cid = line.rstrip("\n")
            if not cid.endswith("#0"):
                continue
            p = pid.get(cid[len("wikipedia-en/"): -2])
            if p is None:
                missing += 1
                continue
            rows.append(i); ids.append(p)
    m = np.load(s / "full_f16.npy", mmap_mode="r")
    e = np.asarray(m[np.array(rows, dtype=np.int64), : a.dims], dtype=np.float32)
    e /= np.linalg.norm(e, axis=1, keepdims=True).clip(1e-6)
    tmp = out / f"{s.name}.tmp.npz"
    np.savez(tmp, ids=np.array(ids, dtype=np.int64), emb=e.astype(np.float16))
    os.replace(tmp, out / f"{s.name}.npz")
    print(f"{s.name}: {len(ids):,} leads", flush=True)
print(f"done; {missing:,} first chunks without a page id", flush=True)
