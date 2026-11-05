"""Wikipedia link graph + talk-page assessment categories from the SQL dumps, for essentials selection.

  uv run python essentials/sql_graph.py <dump dir> <out dir> [wiki=enwiki]

Reads <wiki>-latest-{page,redirect,linktarget,pagelinks,categorylinks}.sql.gz. Writes to <out>:
  edges.npy       int32 (n, 2): article page_id -> article page_id (namespace 0, redirects resolved, no self links)
  assess.tsv      article page_id \t category  (talk-page categories: vital-article levels, WikiProject importance)
Rows in these dumps are one per line, so they are parsed line by line (pagelinks via numpy, in blocks).
"""
import gzip, re, subprocess, sys, time
from pathlib import Path
import numpy as np

dump, out = Path(sys.argv[1]), Path(sys.argv[2])
wiki = sys.argv[3] if len(sys.argv) > 3 else "enwiki"
out.mkdir(parents=True, exist_ok=True)
t0 = time.time()
log = lambda *a: print(f"[{time.time() - t0:6.0f}s]", *a, flush=True)


def lines(table):
    """Data lines ('(...),') of a dump, decompressed by pigz."""
    p = subprocess.Popen(["pigz", "-dc", str(dump / f"{wiki}-latest-{table}.sql.gz")], stdout=subprocess.PIPE, bufsize=1 << 24)
    for line in p.stdout:
        if line.startswith(b"("):
            yield line
    p.wait()


# --- pages: (ns, title) -> page_id for articles (0) and their talk pages (1)
row = re.compile(rb"^\((\d+),(-?\d+),'((?:[^'\\]|\\.)*)',(\d)")
title0, title1, is_redir = {}, {}, set()
for line in lines("page"):
    m = row.match(line)
    if not m:
        continue
    ns = m.group(2)
    if ns == b"0":
        pid = int(m.group(1)); title0[m.group(3)] = pid
        if m.group(4) == b"1":
            is_redir.add(pid)
    elif ns == b"1":
        title1[int(m.group(1))] = m.group(3)
log(f"pages: {len(title0):,} ns0 ({len(is_redir):,} redirects), {len(title1):,} talk")

# --- redirects: redirect page_id -> target article page_id (one hop; double redirects are rare and broken anyway)
rd = re.compile(rb"^\((\d+),0,'((?:[^'\\]|\\.)*)'")
redirect = {}
for line in lines("redirect"):
    m = rd.match(line)
    if m:
        t = title0.get(m.group(2))
        if t is not None and t not in is_redir:
            redirect[int(m.group(1))] = t
log(f"redirects resolved: {len(redirect):,}")


def resolve(pid):
    if pid in is_redir:
        return redirect.get(pid)
    return pid


# --- link targets: lt_id -> article page_id (ns 0), and lt_id -> category title (ns 14) for assessment categories
lt = re.compile(rb"^\((\d+),(-?\d+),'((?:[^'\\]|\\.)*)'\)")
want_cat = re.compile(rb"^(Wikipedia_level-\d_vital_articles.*|(Top|High|Mid|Low|Bottom|NA|Unknown)-importance_.+_articles)$")
max_lt = 0
lt_ids, lt_pids, cats = [], [], {}
for line in lines("linktarget"):
    m = lt.match(line)
    if not m:
        continue
    ns = m.group(2)
    if ns == b"0":
        pid = title0.get(m.group(3))
        if pid is not None:
            pid = resolve(pid)
            if pid is not None:
                lt_ids.append(int(m.group(1))); lt_pids.append(pid)
    elif ns == b"14" and want_cat.match(m.group(3)):
        cats[int(m.group(1))] = m.group(3).decode("utf8", "replace")
lt_map = np.full(max(lt_ids) + 1, -1, dtype=np.int64)
lt_map[np.array(lt_ids)] = np.array(lt_pids)
del lt_ids, lt_pids
log(f"link targets: {int((lt_map >= 0).sum()):,} articles, {len(cats):,} assessment categories")

# --- category links from talk pages -> article
cl_from = re.compile(rb"^\((\d+),")
cl_to = re.compile(rb",(\d+)\)[,;]?\s*$")
n = 0
with open(out / "assess.tsv.part", "w") as f:
    for line in lines("categorylinks"):
        m2 = cl_to.search(line)
        if not m2:
            continue
        c = cats.get(int(m2.group(1)))
        if c is None:
            continue
        talk = title1.get(int(cl_from.match(line).group(1)))
        if talk is None:
            continue
        pid = title0.get(talk)
        pid = resolve(pid) if pid is not None else None
        if pid is not None:
            f.write(f"{pid}\t{c}\n"); n += 1
(out / "assess.tsv.part").rename(out / "assess.tsv")
log(f"assessment rows: {n:,}")
del title1

# --- page links, article -> article
ok_src = np.zeros(max(title0.values()) + 1, dtype=bool)
for pid in title0.values():
    ok_src[pid] = True
for pid in is_redir:
    ok_src[pid] = False
del title0, redirect, is_redir, cats
tr = bytes.maketrans(b"(),;\n", b"     ")
p = subprocess.Popen(f"pigz -dc {dump}/{wiki}-latest-pagelinks.sql.gz | grep '^('", shell=True, stdout=subprocess.PIPE, bufsize=1 << 24)
parts, total, buf = [], 0, b""
while True:
    block = p.stdout.read(1 << 27)
    if not block:
        break
    block = buf + block
    cut = block.rindex(b"\n") + 1
    buf = block[cut:]
    a = np.fromstring(block[:cut].translate(tr), dtype=np.int64, sep=" ").reshape(-1, 3)
    total += len(a)
    a = a[a[:, 1] == 0]
    src = a[:, 0]; tgt = a[:, 2]
    keep = (src < len(ok_src)) & (tgt < len(lt_map))
    src, tgt = src[keep], tgt[keep]
    keep = ok_src[src]
    src, tgt = src[keep], lt_map[tgt[keep]]
    keep = (tgt >= 0) & (tgt != src)
    parts.append(np.stack([src[keep], tgt[keep]], 1).astype(np.int32))
    if len(parts) % 20 == 0:
        log(f"pagelinks rows {total:,}, kept {sum(len(x) for x in parts):,}")
p.wait()
# dedupe (redirect resolution creates repeats) with little extra memory: one int64 key array, sorted in place
k = np.empty(sum(len(x) for x in parts), np.int64)
o = 0
while parts:
    x = parts.pop(0)
    k[o:o + len(x)] = (x[:, 0].astype(np.int64) << 32) | x[:, 1].astype(np.int64); o += len(x)
    del x
k.sort()
keep = np.empty(len(k), bool); keep[0] = True; np.not_equal(k[1:], k[:-1], out=keep[1:])
k = k[keep]; del keep
e = np.empty((len(k), 2), np.int32); e[:, 0] = k >> 32; e[:, 1] = k & 0xFFFFFFFF
del k
np.save(out / "edges.npy", e)
log(f"edges: {len(e):,} (from {total:,} rows)")
