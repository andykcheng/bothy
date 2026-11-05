"""Fetch selected Project Gutenberg books (HTML edition, optionally with images) from public mirrors.

  uv run python books/gutenberg_fetch.py <ids.txt> <books dir> [--images] [--workers 8]

Writes <books dir>/<id>/<id>-h.htm (and images/… referenced by it with --images). Resumable: finished books
have a .done marker. Requests are spread over several mirrors of the main collection (Gutenberg asks automated
downloads to use mirrors, not www.gutenberg.org), a few connections each.
"""
import argparse, re, sys, threading, time, urllib.parse, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

MIRRORS = ["https://mirror.csclub.uwaterloo.ca/gutenberg", "https://mirrors.xmission.com/gutenberg"]
UA = {"User-Agent": "survivalpedia-books/1.0 (offline library build; low rate)"}


def book_dir(bid: int) -> str:
    s = str(bid)
    return "/".join(list(s[:-1]) + [s]) if len(s) > 1 else f"0/{s}"


def get(url, tries=3):
    for k in range(tries):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return None
            time.sleep(2 * (k + 1))
        except Exception:
            time.sleep(2 * (k + 1))
    raise RuntimeError(f"failed: {url}")


def fetch(bid, out: Path, images: bool, mirror: str):
    d = out / str(bid)
    if (d / ".done").exists():
        return "skip", 0
    d.mkdir(parents=True, exist_ok=True)
    base = f"{mirror}/{book_dir(bid)}/{bid}-h/"
    html = get(base + f"{bid}-h.htm") or get(base + f"{bid}-h.html")
    if html is None:
        (d / ".done").write_text("no html\n")
        return "nohtml", 0
    (d / f"{bid}-h.htm").write_bytes(html)
    n = len(html)
    if images:
        srcs = set(re.findall(rb'<img[^>]+src="([^"]+)"', html, re.I))
        for s in srcs:
            rel = urllib.parse.unquote(s.decode("utf-8", "replace"))
            if rel.startswith(("http:", "https:", "data:", "/")) or ".." in rel:
                continue
            dest = d / rel
            if dest.exists():
                continue
            data = get(base + urllib.parse.quote(rel))
            if data:
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_bytes(data); n += len(data)
    (d / ".done").write_text("ok\n")
    return "ok", n


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ids"); ap.add_argument("out")
    ap.add_argument("--images", action="store_true")
    ap.add_argument("--workers", type=int, default=8)
    a = ap.parse_args()
    ids = [int(x) for x in open(a.ids).read().split()]
    out = Path(a.out)
    lock, stats, t0 = threading.Lock(), {"ok": 0, "skip": 0, "nohtml": 0, "err": 0, "bytes": 0}, time.time()

    def job(i_bid):
        i, bid = i_bid
        try:
            st, n = fetch(bid, out, a.images, MIRRORS[i % len(MIRRORS)])
        except Exception as e:
            st, n = "err", 0
            print(f"{bid}: {e}", file=sys.stderr, flush=True)
        with lock:
            stats[st] += 1; stats["bytes"] += n
            done = sum(stats[k] for k in ("ok", "skip", "nohtml", "err"))
            if done % 50 == 0 or done == len(ids):
                print(f"{done}/{len(ids)} {stats} {stats['bytes'] / 1e6 / (time.time() - t0):.2f} MB/s", flush=True)

    with ThreadPoolExecutor(a.workers) as ex:
        list(ex.map(job, enumerate(ids)))
    print("done", stats)


if __name__ == "__main__":
    main()
