"""Monthly Wikimedia pageview dumps -> total views per page id, for the usefulness score.

    uv run python pageviews.py /mnt/bigbelly/survivalpedia/sources/wikimedia/pageviews-*-user.bz2 \\
        --out /mnt/bigbelly/survivalpedia/build/pageviews

Dump lines: `domain title page_id access count hourly`. Only months with a `.ok` marker are read.
One `bzip2 -dc` per month runs in parallel (bzip2 decoding is the bottleneck).
Writes `{wiki}.tsv` (page_id, views over all months), sorted by views, plus months.json.
"""
import argparse, json, subprocess
from collections import Counter
from multiprocessing import Pool
from pathlib import Path

ap = argparse.ArgumentParser()
ap.add_argument("dumps", nargs="+")
ap.add_argument("--out", required=True)
ap.add_argument("--wikis", default="en.wikipedia")
a = ap.parse_args()
WIKIS = a.wikis.split(",")


def month(path):
    views = {w: Counter() for w in WIKIS}
    prefixes = tuple((w + " ").encode() for w in WIKIS)
    p = subprocess.Popen(["bzip2", "-dc", path], stdout=subprocess.PIPE, bufsize=1 << 20)
    for raw in p.stdout:
        if not raw.startswith(prefixes):
            continue
        f = raw.decode("utf-8", "replace").split(" ")
        if len(f) < 5 or f[2] == "null":
            continue
        try:
            pid, n = int(f[2]), int(f[4])
        except ValueError:
            continue
        views[f[0]][pid] += n
    if p.wait() != 0:
        raise RuntimeError(f"bzip2 failed on {path}")
    print(f"{Path(path).name}: " + ", ".join(f"{w} {len(views[w])} pages" for w in WIKIS), flush=True)
    return path, views


if __name__ == "__main__":
    files = sorted(d for d in a.dumps if Path(d + ".ok").exists())
    skipped = sorted(set(a.dumps) - set(files))
    print(f"{len(files)} months; skipped (no .ok): {[Path(s).name for s in skipped]}", flush=True)
    total = {w: Counter() for w in WIKIS}
    with Pool(len(files)) as pool:
        for path, v in pool.imap_unordered(month, files):
            for w in WIKIS:
                total[w].update(v[w])
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    for w in WIKIS:
        with open(out / f"{w}.tsv", "w") as fh:
            for pid, n in total[w].most_common():
                fh.write(f"{pid}\t{n}\n")
    (out / "months.json").write_text(json.dumps({"months": [Path(f).name for f in files]}, indent=1))
    print("DONE", flush=True)
