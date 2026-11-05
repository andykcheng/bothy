"""Article table for one extracted source (zim_extract.py output), for essentials selection.

  uv run python essentials/docs_table.py <source dir> <out dir>

Writes <out>/<source>.docs.tsv:  idx  page_id  title  html_bytes  n_images  n_chunks
and <out>/<source>.cats.tsv:     idx  category|category|...
idx is the line number in docs.jsonl. Reads docs.jsonl, then chunks.jsonl (only the chunk ids).
"""
import sys, time
from pathlib import Path
import orjson

src, out = Path(sys.argv[1]), Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
name = src.name
t0 = time.time()
ids = {}          # doc_id -> idx
rows = []
with open(src / "docs.jsonl", "rb") as f, open(out / f"{name}.cats.tsv.part", "w") as cf:
    for i, line in enumerate(f):
        d = orjson.loads(line)
        ids[d["doc_id"]] = i
        t = d["title"].replace("\t", " ").replace("\n", " ")
        rows.append([i, d.get("page_id") or 0, t, len(d["html"]), len(d.get("images") or [])])
        if d.get("categories"):
            cf.write(f"{i}\t{'|'.join(c.replace('|', ' ').replace(chr(9), ' ') for c in d['categories'])}\n")
        if i % 500000 == 0:
            print(f"docs {i:,} {time.time() - t0:.0f}s", flush=True)
print(f"docs done: {len(rows):,} in {time.time() - t0:.0f}s", flush=True)

n_chunks = [0] * len(rows)
key = b'"doc_id":"'
with open(src / "chunks.jsonl", "rb") as f:
    for k, line in enumerate(f):
        # fast path: the raw doc_id field; JSON-escaped ids (quotes, backslashes, \u) go through orjson
        s0 = line.index(key) + len(key)
        raw = line[s0:line.index(b'"', s0)]
        j = ids.get(raw.decode()) if b"\\" not in raw else None
        if j is None:
            j = ids.get(orjson.loads(line)["doc_id"])
        if j is not None:
            n_chunks[j] += 1
        if k % 5000000 == 0:
            print(f"chunks {k:,} {time.time() - t0:.0f}s", flush=True)
with open(out / f"{name}.docs.tsv.part", "w") as f:
    f.write("idx\tpage_id\ttitle\thtml_bytes\tn_images\tn_chunks\n")
    for r, c in zip(rows, n_chunks):
        f.write("\t".join(map(str, r)) + f"\t{c}\n")
(out / f"{name}.docs.tsv.part").rename(out / f"{name}.docs.tsv")
(out / f"{name}.cats.tsv.part").rename(out / f"{name}.cats.tsv")
print(f"done {time.time() - t0:.0f}s", flush=True)
