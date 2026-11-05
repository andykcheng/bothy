"""Leads (title + opening text, as leads.py makes them) for a random sample of articles the classifier rates
highly but Jev never scored, so Jev can check what classify_all.py would add.

  uv run python essentials/sample_leads.py <work dir> <extract dir> <out.jsonl> [-n 1000] [--min 1.0] [--max 99]

Samples from <work>/scores-all.tsv rows without a Jev score and with --min <= pred < --max, leaving out
disambiguation pages. Each article's text is read from its embedding shard's byte range of chunks.jsonl (the
shard meta.json records it), so the ~100 GB file is never scanned whole.
"""
import argparse, json, random, re
from pathlib import Path

import numpy as np
import orjson

ap = argparse.ArgumentParser()
ap.add_argument("work"); ap.add_argument("extract"); ap.add_argument("out")
ap.add_argument("-n", type=int, default=1000)
ap.add_argument("--min", type=float, default=1.0)
ap.add_argument("--max", type=float, default=99.0)
ap.add_argument("--chars", type=int, default=1200)
ap.add_argument("--seed", type=int, default=1)
ap.add_argument("--exclude", help="exclude.tsv (exclude.py): leave these articles out")
a = ap.parse_args()
W, X = Path(a.work), Path(a.extract)

skip = set()
if a.exclude:
    with open(W / a.exclude, encoding="utf8") as f:
        next(f)
        skip = {int(l.split("\t", 1)[0]) for l in f}
pool = []
with open(W / "scores-all.tsv", encoding="utf8") as f:
    next(f)
    for line in f:
        pid, title, chunks, v0, jev, pred, final = line.rstrip("\n").split("\t")
        if jev == "" and a.min <= float(pred) < a.max and "(disambiguation)" not in title and int(pid) not in skip:
            pool.append((int(pid), title, int(v0) if v0 else 10**7, float(pred)))
random.seed(a.seed)
pick = {p[0]: p for p in random.sample(pool, min(a.n, len(pool)))}
print(f"{len(pool):,} candidates, {len(pick):,} sampled")

shard_of = {}
for f in sorted((W / "shard-leads").glob("*.npz")):
    for p in np.load(f)["ids"]:
        if int(p) in pick:
            shard_of[int(p)] = f.stem
want = {}   # shard -> {title: page id}
for p, s in shard_of.items():
    want.setdefault(s, {})[pick[p][1]] = p

TABLEISH = re.compile(r"(\|.*){3,}")
rows = []
with open(X / "chunks.jsonl", "rb") as fh:
    for s, titles in sorted(want.items()):
        m = json.load(open(X / "emb" / "shards" / s / "meta.json"))
        fh.seek(m["start_offset"])
        got = {}
        for line in fh.read(m["end_offset"] - m["start_offset"]).splitlines():
            c = orjson.loads(line)
            n = int(c["chunk_id"].rsplit("#", 1)[1])
            if n > 2:
                continue
            title, _, body = c["text"].partition("\n")
            title = title.split(" > ")[0]
            if title in titles:
                got.setdefault(title, []).append((n, body))
        for title, parts in got.items():
            parts.sort()
            prose = [b for _, b in parts if not TABLEISH.search(b)] or [b for _, b in parts]
            p = titles[title]
            rows.append({"rank": pick[p][2], "page_id": p, "title": title, "lead": " ".join(prose)[: a.chars], "pred": pick[p][3]})
with open(a.out, "wb") as f:
    for r in rows:
        f.write(orjson.dumps(r) + b"\n")
print(f"wrote {len(rows):,} leads to {a.out}")
