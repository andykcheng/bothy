"""Search eval: golden queries (expected titles) or synthetic known-item queries (source title).

    uv run python eval_search.py <pack> [--golden ../eval/golden.jsonl ...] [--out results.json]
    uv run python eval_search.py --server http://127.0.0.1:8080 --golden ../eval/synth-*.jsonl --quiet

Reports hit@1/5/10 and MRR@10 per mode and language, plus latency. A hit is a result whose
normalised title is in the row's "expect" list (or equals its "title" for synthetic rows).
"""
import argparse, json, statistics, subprocess, unicodedata, urllib.parse, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODES = ("bm25", "dense", "hybrid")  # narrowed by --modes
ap = argparse.ArgumentParser()
ap.add_argument("pack", nargs="?")
ap.add_argument("--golden", nargs="+", default=[str(ROOT / "eval/golden.jsonl")])
ap.add_argument("--sp", default=str(ROOT / "core/target/release/sp"))
ap.add_argument("--server", help="use a running sp-server instead of the CLI")
ap.add_argument("--out")
ap.add_argument("-k", type=int, default=10)
ap.add_argument("--quiet", action="store_true", help="summary only, no per-query tables")
ap.add_argument("--modes", default="bm25,dense,hybrid")
a = ap.parse_args()
MODES = tuple(a.modes.split(","))
assert a.pack or a.server, "give a pack or --server"

norm = lambda t: unicodedata.normalize("NFKC", t).casefold().replace("_", " ").strip()


def search(q, mode):
    if a.server:
        url = f"{a.server}/api/search?" + urllib.parse.urlencode({"q": q, "mode": mode, "k": a.k})
        with urllib.request.urlopen(url, timeout=60) as r:
            j = json.load(r)
        return j["hits"], {"embed_ms": j["embed_ms"], "search_ms": j["search_ms"]}
    out = subprocess.run([a.sp, "search", a.pack, q, "--mode", mode, "-k", str(a.k), "--json"],
                         check=True, capture_output=True, text=True).stdout
    j = json.loads(out)
    return j["hits"], {k: v for k, v in j["timing"].items() if k != "open_ms"}


rows = []
for g in a.golden:
    for line in open(g):
        if line.strip():
            r = json.loads(line)
            r["set"] = Path(g).stem
            rows.append(r)

results, lat = [], {}
for g in rows:
    want = {norm(t) for t in (g.get("expect") or [g["title"]])}
    row = {"q": g["q"], "lang": g["lang"], "set": g["set"], "expect": sorted(want)}
    for mode in MODES:
        hits, timing = search(g["q"], mode)
        rank = next((i + 1 for i, h in enumerate(hits) if norm(h["title"]) in want), None)
        row[mode] = {"top": [f'{h["title"]} [{h["source"]}]' for h in hits[:5]], "rank": rank}
        for k, v in timing.items():
            lat.setdefault(f"{mode}.{k}", []).append(v)
    results.append(row)


def metrics(rs, mode):
    ranks = [r[mode]["rank"] for r in rs]
    n = len(ranks)
    at = lambda k: sum(1 for x in ranks if x and x <= k) / n
    return {"n": n, "hit@1": round(at(1), 3), "hit@5": round(at(5), 3), f"hit@{a.k}": round(at(a.k), 3),
            "mrr": round(sum(1 / x for x in ranks if x) / n, 3)}


def pct(v, p):
    v = sorted(v); return v[min(len(v) - 1, int(round(p * (len(v) - 1))))]


groups = {}
for r in results:
    groups.setdefault(f'{r["set"]}/{r["lang"]}', []).append(r)
summary = {
    "overall": {m: metrics(results, m) for m in MODES},
    "by_group": {g: {m: metrics(rs, m) for m in MODES} for g, rs in sorted(groups.items())},
    "latency_ms": {k: {"p50": round(statistics.median(v), 2), "p95": round(pct(v, 0.95), 2)} for k, v in lat.items()},
}
print(json.dumps(summary, indent=1, ensure_ascii=False))
if not a.quiet:
    for r in results:
        marks = " ".join(f"{m}:{r[m]['rank'] or '-'}" for m in MODES)
        print(f"\n[{r['lang']}] {r['q']}   {marks}")
        for i in range(5):
            cells = [(r[m]["top"][i] if i < len(r[m]["top"]) else "") for m in MODES]
            print("   " + " | ".join(c[:38].ljust(38) for c in cells))
if a.out:
    Path(a.out).write_text(json.dumps({"summary": summary, "results": results}, indent=1, ensure_ascii=False))
