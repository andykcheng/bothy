"""Essentials v1: re-rank the v0 candidates (rank.py) by Jev usefulness, fill the Wikipedia byte budget.

  uv run python essentials/rerank.py <work dir> [--prior 0.3] [--gamma 0.15] [--budget 600e6] [--sweep] [--out v1.tsv]

Inputs in <work dir>: essentials-wikipedia-en.tsv (v0 ranking: rank, page_id, title, chunks, est_bytes…),
jev-top20k-v5.jsonl and jev-rand-v5.jsonl (Jev v5 scores 0..3 for v0 ranks 1..400k).
Stage 0: v0's seed-driven head (rank <= --head, default 2000) unless Jev is sure it is junk (< --head-min 0.15):
Jev underrates some hand-seeded survival topics (Whetstone 0.25, Snare 0.22, Orienteering 0.22, Polaris 0.24).
Stage 1: articles with jev >= --floor (default 1.0), by key = jev + prior * (1 - ln(rank) / ln(400k))
- gamma * log10(chunks). Stage 2 fills the rest of the budget in v0 order (link graph + pageviews) among articles
with jev >= --fill-min: below ~1.0 Jev barely separates (an audit of 100 found obscure chemicals and species just
inside a pure-Jev cut, and Pacific Ocean, Hinduism, major languages just outside), while v0 prefers broad, much-read
topics. Articles without a Jev score (v0 rank > 400k) are left out.
Coverage = share of the essentials questions (eval/essentials-questions.jsonl) whose answer article is selected.
"""
import argparse, csv, json, math, sys
from pathlib import Path

csv.field_size_limit(sys.maxsize)
ap = argparse.ArgumentParser()
ap.add_argument("work")
ap.add_argument("--questions", default=str(Path(__file__).resolve().parents[2] / "eval/essentials-questions.jsonl"))
ap.add_argument("--redirects", default="/mnt/bigbelly/survivalpedia/build/wikipedia-en/redirects.jsonl")
ap.add_argument("--prior", type=float, default=0.3)
ap.add_argument("--gamma", type=float, default=0.15)
ap.add_argument("--budget", type=float, default=float("inf"), help="byte cap (default none: the score rules decide)")
ap.add_argument("--head", type=int, default=2000, help="stage 0: v0 ranks up to this…")
ap.add_argument("--head-min", type=float, default=0.15, help="…with a Jev score at least this")
ap.add_argument("--floor", type=float, default=1.0, help="stage 1: Jev score at least this")
ap.add_argument("--fill-min", type=float, default=0.4, help="stage 2: fill in v0 order among Jev >= this")
ap.add_argument("--fill-rank", type=int, default=10**9, help="stage 2: only articles up to this v0 rank (widely read and linked)")
ap.add_argument("--sweep", action="store_true")
ap.add_argument("--out")
ap.add_argument("--extra", nargs="*", default=[],
                help="v2: Jev scores of articles beyond v0's 400k (classify_all.py candidates), e.g. jev-new-v5.jsonl")
ap.add_argument("--exclude", help="exclude.tsv (exclude.py): kinds of article never selected (people, films, sport…)")
ap.add_argument("--scores", default="scores-all.tsv", help="v2: chunk counts for --extra articles outside the v0 table")
a = ap.parse_args()
W = Path(a.work)

rows = []
with open(W / "essentials-wikipedia-en.tsv") as f:
    for r in csv.DictReader(f, delimiter="\t"):
        rows.append((int(r["rank"]), int(r["page_id"]), r["title"], int(r["chunks"]), int(r["est_bytes"]), r["topic"]))
jev = {}
for fn in ("jev-top20k-v5.jsonl", "jev-rand-v5.jsonl"):
    for l in open(W / fn):
        d = json.loads(l); jev[d["page_id"]] = d["score"]
# v2: articles found by the lead classifier (classify_all.py) and then scored by Jev join the candidates. Those past
# v0's 1.3M rows get an estimated size (chunks x 417 B, as rank.py) and a rank past the end, so the rank prior is
# slightly negative for them and the stage-2 fill (v0 order) reaches them last.
n_extra = 0
if a.extra:
    have = {r[1] for r in rows}
    extra = {}
    for fn in a.extra:
        for l in open(W / fn):
            d = json.loads(l); extra[d["page_id"]] = d["score"]
    chunks = {}
    with open(W / a.scores, encoding="utf8") as f:
        next(f)
        for line in f:
            pid, title, ch, *_ = line.split("\t")
            if int(pid) in extra:
                chunks[int(pid)] = (title, int(ch))
    for pid, sc in extra.items():
        if pid in jev or "(disambiguation)" in chunks.get(pid, ("", 0))[0]:
            continue
        jev[pid] = sc
        n_extra += 1
        if pid not in have and pid in chunks:
            t, ch = chunks[pid]
            rows.append((2_000_000, pid, t, ch, ch * 417, ""))
    print(f"{n_extra:,} extra Jev-scored candidates from {', '.join(a.extra)}", file=sys.stderr)
n_excluded = 0
if a.exclude:
    with open(W / a.exclude, encoding="utf8") as f:
        next(f)
        drop = {int(l.split("\t", 1)[0]) for l in f}
    n_excluded = sum(r[1] in drop for r in rows)
    rows = [r for r in rows if r[1] not in drop]
    print(f"{n_excluded:,} candidates excluded by {a.exclude}", file=sys.stderr)
by_title = {r[2]: r for r in rows}
print(f"{len(rows):,} v0 rows, {len(jev):,} Jev scores", file=sys.stderr)

# question titles -> article (through redirects)
redir = {}
need = {json.loads(l)["title"] for l in open(a.questions)}
with open(a.redirects) as f:
    for l in f:
        d = json.loads(l)
        t = d.get("from_title")
        if t in need:
            redir[t] = d.get("to_path", "").replace("_", " ")
qs = []
for l in open(a.questions):
    q = json.loads(l)
    r = by_title.get(q["title"]) or by_title.get(redir.get(q["title"], ""))
    qs.append((q, r))
found = [(q, r) for q, r in qs if r]
LOG_N = math.log(400_001)


def select(prior, gamma):
    cand = [r for r in rows if r[1] in jev]
    key = {r[1]: jev[r[1]] + prior * (1 - math.log(r[0]) / LOG_N) - gamma * math.log10(max(1, r[3])) for r in cand}
    cand.sort(key=lambda r: -key[r[1]])
    stage0 = [r for r in sorted(cand, key=lambda r: r[0]) if r[0] <= a.head and jev[r[1]] >= a.head_min]
    head = {r[1] for r in stage0}
    stage1 = [r for r in cand if jev[r[1]] >= a.floor and r[1] not in head]
    stage2 = sorted((r for r in cand if a.fill_min <= jev[r[1]] < a.floor and r[0] <= a.fill_rank and r[1] not in head),
                    key=lambda r: r[0])
    cand = stage0 + stage1 + stage2
    out, total = [], 0
    for r in cand:
        if total + r[4] > a.budget:
            continue   # skip what doesn't fit, keep filling with smaller ones
        out.append(r); total += r[4]
    return out, key, total


def report(prior, gamma):
    sel, key, total = select(prior, gamma)
    ids = {r[1] for r in sel}
    cov = sum(r[1] in ids for _, r in found) / len(found)
    js = [jev[r[1]] for r in sel]
    bands = [sum(1 for x in js if x >= t) for t in (2.5, 1.5, 1.0)]
    unscored_q = sum(1 for _, r in found if r[1] not in jev)
    return {"prior": prior, "gamma": gamma, "articles": len(sel), "MB": round(total / 1e6), "coverage": round(cov, 3),
            "jev>=2.5/1.5/1.0": bands, "mean_jev": round(sum(js) / len(js), 2), "min_jev": round(min(js), 2),
            "questions_unscored": unscored_q}, sel, key


if a.sweep:
    for fl, fm in ((1.0, 0.3), (1.0, 0.4), (1.0, 0.5), (1.5, 0.4), (9, 0.0)):
        a.floor, a.fill_min = fl, fm
        print(f"floor {fl} fill-min {fm}:", json.dumps(report(a.prior, a.gamma)[0]))
    sys.exit()
res, sel, key = report(a.prior, a.gamma)
print(json.dumps(res, indent=1))
ids = {r[1] for r in sel}
misses = sorted(((q["title"], r[0], jev.get(r[1])) for q, r in found if r[1] not in ids), key=lambda x: x[1])
print(f"{len(misses)} questions missed (title, v0 rank, jev):", misses[:60])
v0_ids = {r[1] for r in rows if r[0] <= 123_701}
print(f"vs v0 essentials: {len(ids & v0_ids):,} kept, {len(ids - v0_ids):,} added, {len(v0_ids - ids):,} dropped")
if n_extra:
    beyond = [r for r in sel if r[0] > 400_000]
    print(f"from beyond v0's 400k: {len(beyond):,} articles, {sum(r[4] for r in beyond) / 1e6:.0f} MB")
if a.out:
    with open(a.out, "w") as f:
        f.write("rank\tpage_id\ttitle\ttopic\tjev\tkey\tv0_rank\tchunks\test_bytes\n")
        for i, r in enumerate(sel, 1):
            f.write(f"{i}\t{r[1]}\t{r[2]}\t{r[5]}\t{jev[r[1]]}\t{key[r[1]]:.3f}\t{r[0]}\t{r[3]}\t{r[4]}\n")
    print("wrote", a.out)
