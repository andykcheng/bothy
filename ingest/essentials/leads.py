"""Opening text of candidate articles, for classification (jev_classify.py).

  uv run python essentials/leads.py <ranked tsv> <extract dir> <out jsonl> [-n 400000] [--chars 1200]

Takes the top N rows of a rank.py list and pulls each article's first chunks from <extract dir>/chunks.jsonl
(the title line is dropped; infobox-like chunks are skipped when prose follows). One pass over chunks.jsonl.
"""
import argparse, re
import orjson

ap = argparse.ArgumentParser()
ap.add_argument("ranked"); ap.add_argument("extract"); ap.add_argument("out")
ap.add_argument("-n", type=int, default=400000)
ap.add_argument("--chars", type=int, default=1200)
a = ap.parse_args()

want = {}   # doc title -> (rank, page_id)
with open(a.ranked, encoding="utf8") as f:
    next(f)
    for i, line in enumerate(f):
        if i >= a.n:
            break
        r = line.split("\t")
        want[r[2]] = (int(r[0]), int(r[1]))
TABLEISH = re.compile(r"(\|.*){3,}")
got = {}    # title -> list of chunk bodies (first few)
key = b'"doc_id":"wikipedia-en/'
with open(f"{a.extract}/chunks.jsonl", "rb") as f:
    for k, line in enumerate(f):
        c = orjson.loads(line)
        n = int(c["chunk_id"].rsplit("#", 1)[1])
        if n > 2:
            continue
        title, _, body = c["text"].partition("\n")
        title = title.split(" > ")[0]
        if title not in want:
            continue
        got.setdefault(title, []).append((n, body))
        if k % 5_000_000 == 0:
            print(f"chunks {k:,}, found {len(got):,}", flush=True)
with open(a.out, "wb") as f:
    for title, parts in got.items():
        parts.sort()
        prose = [b for _, b in parts if not TABLEISH.search(b)] or [b for _, b in parts]
        lead = " ".join(prose)[: a.chars]
        rank, pid = want[title]
        f.write(orjson.dumps({"rank": rank, "page_id": pid, "title": title, "lead": lead}) + b"\n")
print(f"leads: {len(got):,} of {len(want):,}")
