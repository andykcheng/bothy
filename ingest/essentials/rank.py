"""Rank every English Wikipedia article by survival usefulness and cut size tiers: the essentials list.

  uv run python essentials/rank.py <work dir> [--questions ../eval/essentials-questions.jsonl]

Work dir holds the outputs of docs_table.py (wikipedia-en.docs.tsv, .cats.tsv) and sql_graph.py (edges.npy,
assess.tsv). Also reads seeds.txt (next to this script), pageviews and the ZIM redirects.

Method (DESIGN §6):
  1. Seeds per topic: hand-picked hubs (seeds.txt), WikiProject importance ratings of relevant projects,
     Vital Articles levels (as a small "general knowledge" topic), and category rules (British flora/fungi...).
  2. Personalised PageRank from each topic's seeds over the article link graph (all topics at once, one
     sparse matrix product per iteration), plus a global PageRank.
  3. relevance = sum over topics (topic weight x PPR) / global PageRank^beta: articles every page links to
     (countries, years) don't win just by being central.
  4. x quality (pageviews) x penalties (biographies, sport, entertainment, lists, ...).
  5. Order by score, add up the estimated pack bytes (chunks x measured bytes per chunk), cut tiers.
Writes <work>/essentials-wikipedia-en.tsv (the dataset) and <work>/essentials-summary.json.
"""
import argparse, json, re, time
from collections import defaultdict
from pathlib import Path

import numpy as np
import orjson
import scipy.sparse as sp

ap = argparse.ArgumentParser()
ap.add_argument("work")
ap.add_argument("--questions")
ap.add_argument("--pageviews", default="/mnt/bigbelly/survivalpedia/build/pageviews/en.wikipedia.tsv")
ap.add_argument("--redirects", default="/mnt/bigbelly/survivalpedia/build/wikipedia-en/redirects.jsonl")
ap.add_argument("--alpha", type=float, default=0.25, help="PPR teleport probability (higher = stays nearer the seeds)")
ap.add_argument("--beta", type=float, default=0.5, help="exponent of the global-PageRank correction")
ap.add_argument("--gamma", type=float, default=0.4, help="size penalty: score / chunks^gamma")
ap.add_argument("--pv-exp", type=float, default=0.0, help="0: quality = log(pageviews); else quality = (pageviews + 100)^pv_exp")
ap.add_argument("--tag", default="", help="suffix for output files (parameter sweeps)")
ap.add_argument("--iters", type=int, default=40)
ap.add_argument("--tiers", default="essentials:600e6,standard:6e9", help="Wikipedia text+BM25 byte budget per tier")
a = ap.parse_args()
W = Path(a.work)
t0 = time.time()
log = lambda *x: print(f"[{time.time() - t0:5.0f}s]", *x, flush=True)

BYTES_PER_CHUNK = 267 + 150    # compressed text + BM25, measured (MEASUREMENTS §2)
TOPIC_W = {"medicine": 2.0, "water": 1.6, "food": 1.4, "agriculture": 1.2, "foraging": 1.2, "shelter": 1.3, "hazards": 1.3,
           "energy": 1.0, "tools": 1.0, "radio": 0.8, "chemistry": 0.8, "navigation": 0.8, "science": 0.7, "society": 0.5,
           "general": 0.3}
TOPICS = list(TOPIC_W)
# WikiProjects (lower-case names as in "Top-importance <name> articles") -> topic, project weight
PROJECTS = {   # names as they appear in the dump (matched lower-case); counts are in <work>/wikiprojects.tsv
    "medicine": ("medicine", 1), "emergency medicine and ems": ("medicine", 1), "pharmacology": ("medicine", .7),
    "anatomy": ("medicine", .4), "physiology": ("medicine", .3), "microbiology": ("medicine", .4), "virus": ("medicine", .4),
    "toxicology": ("medicine", .7), "dentistry": ("medicine", .5), "nursing": ("medicine", .6), "psychiatry": ("medicine", .4),
    "psychology": ("medicine", .15), "society and medicine": ("medicine", .2), "reproductive medicine": ("medicine", .4),
    "veterinary medicine": ("agriculture", .8), "livestock": ("agriculture", .8), "poultry task force": ("agriculture", .8),
    "agriculture": ("agriculture", 1), "horticulture and gardening": ("agriculture", 1), "forestry": ("agriculture", .4),
    "water": ("water", 1), "sanitation": ("water", 1), "environment": ("water", .05),
    "food and drink": ("food", .6), "fishing": ("food", .6),
    "plant": ("foraging", .3), "fungi": ("foraging", .5), "ecology": ("foraging", .2),
    "disaster management": ("hazards", 1), "firefighting": ("hazards", .8), "wildfire": ("hazards", .4),
    "drought and wildfire": ("hazards", .4), "weather": ("hazards", .4),
    "energy": ("energy", .8), "electrical engineering": ("energy", 1), "electronic": ("energy", .5),
    "engineering": ("tools", .6), "civil engineering": ("shelter", .4), "textile arts": ("tools", .6), "cycling": ("tools", .05),
    "scouting": ("shelter", .3), "climbing": ("shelter", .2), "mountain": ("shelter", .02),
    "amateur radio": ("radio", 1), "telecommunications": ("radio", .4), "broadcast engineering and technology": ("radio", .5),
    "chemistry": ("chemistry", .7), "chemicals": ("chemistry", .5), "chemical elements": ("chemistry", .6), "materials": ("chemistry", .6),
    "maps": ("navigation", .7), "general meteorology": ("navigation", .6), "meteorological instrument and data": ("navigation", .4),
    "physics": ("science", .4), "biology": ("science", .5), "education": ("science", .2), "statistics": ("science", .05),
}
IMPORTANCE = {"Top": 1.0, "High": .6, "Mid": .25}
VITAL = {1: .8, 2: .8, 3: .6, 4: .35, 5: .1}
# article categories -> (topic, weight); regexes over the category text
CAT_RULES = [
    (r"^(Flora of (Great )?Britain|Flora of the United Kingdom|Flora of England|Flora of Scotland|Flora of Wales|Flora of Ireland|Flora of Europe)$", "foraging", .15),
    (r"^(Fungi of (Europe|Great Britain|the United Kingdom))$", "foraging", .2),
    (r"^(Edible [a-z ]+|Leaf vegetables|Root vegetables|Stem vegetables|Berries|Herbs|Medicinal plants|Medicinal plants of Europe|Poisonous fungi|Poisonous plants|Plant toxins|Mycotoxins|Halophytes)$", "foraging", .5),
    (r"^(Fauna of (Great Britain|the British Isles)|Mammals of (Europe|the United Kingdom)|Birds of Europe)$", "foraging", .08),
    (r"^(World Health Organization essential medicines.*)$", "medicine", 1.0),
    (r"^(Agricultural pest insects|Animal diseases|Poultry diseases|Cattle diseases|Sheep and goat diseases|Horse diseases|Plant pathogens and diseases|Vegetable diseases)$", "agriculture", .5),
    (r"^(Knives|Sharpening|Scoutcraft|Binding knots|Hitch knots|Loop knots|Bend knots|Woodworking hand tools|Hand tools|Metalworking hand tools)$", "tools", .6),
    (r"^(Food preservation|Food safety|Fermented foods|Survival skills|Camping equipment|Firelighting|Knots|Water treatment|Drinking water|Sanitation|Toilets|First aid)$", None, .8),
]
# penalties: an article in any of these categories (or with this title) is multiplied by the factor
PENALTY_CATS = [
    (r"Living people|\d{3,4}s? births|\d{3,4}s? deaths|People from |alumni|Musicians|singers|actors|actresses|politicians|footballers|cricketers|players|writers|novelists|poets|painters|journalists", .02),
    (r"(films|albums|songs|singles|television series|TV series|episodes|video games|novels|comics|characters|bands|soundtracks|anime|manga|podcasts|radio programmes|board games)$", .02),
    (r"(football|cricket|rugby|baseball|basketball|ice hockey|tennis|golf|Olympic|sports|sportspeople|racing|wrestling|boxing)", .05),
    (r"(elections|political parties|battles|military units|regiments|wars|treaties|royal|monarchs|nobility|companies|brands|clubs|schools|universities|churches|cathedrals|railway stations|airports|ships|aircraft|villages|towns|municipalities|census-designated)", .15),
]
PENALTY_CATS += [
    (r"(Populated places|^Cities |^Towns |Capitals |^Countries |Member states|^States of |^Provinces of |^Counties of |^Regions of |Former countries)", .1),
    (r"(newspapers|magazines|hospitals|organizations|organisations|Buildings and structures|museums|bridges|dams|software|websites|Internet properties|religious|Christian|Islamic|Bible|mythology|universities and colleges|laws$|Acts of)", .2),
    (r"^(Disambiguation pages|Set index articles)", .05),
]
PENALTY_TITLE = [(r"^List of |^Lists of |^\d{3,4} in |^Timeline of |\(disambiguation\)$|^Outline of |^Index of |^Glossary of ", .1)]

# ---------- articles
pids, titles, nch, nimg = [], [], [], []
with open(W / "wikipedia-en.docs.tsv", encoding="utf8") as f:
    next(f)
    for line in f:
        i, pid, t, hb, ni, nc = line.rstrip("\n").split("\t")
        pids.append(int(pid)); titles.append(t); nimg.append(int(ni)); nch.append(int(nc))
N = len(titles)
pids = np.array(pids, np.int64); nch = np.array(nch, np.int64); nimg = np.array(nimg, np.int64)
log(f"articles: {N:,}, chunks {int(nch.sum()):,}")
pid2idx = np.full(int(pids.max()) + 1, -1, np.int64)
pid2idx[pids] = np.arange(N)
by_title = {t: i for i, t in enumerate(titles)}
cats = [()] * N
with open(W / "wikipedia-en.cats.tsv", encoding="utf8") as f:
    for line in f:
        i, c = line.rstrip("\n").split("\t", 1)
        cats[int(i)] = tuple(c.split("|"))
redir = {}
with open(a.redirects, "rb") as f:
    for line in f:
        r = orjson.loads(line)
        redir[r["from_title"]] = r["to_path"].replace("_", " ")
log(f"titles {len(by_title):,}, redirects {len(redir):,}")


def find(title):
    t = title.strip().replace("_", " ")
    for c in (t, t[:1].upper() + t[1:]):
        if c in by_title:
            return by_title[c]
        if c in redir and redir[c] in by_title:
            return by_title[redir[c]]
    return None


# ---------- seeds
K = len(TOPICS); col = {t: k for k, t in enumerate(TOPICS)}
S = defaultdict(float)          # (idx, k) -> weight
why = defaultdict(list)         # idx -> reasons
missing = []
topic = None
for line in open(Path(__file__).with_name("seeds.txt"), encoding="utf8"):
    line = line.strip()
    if line.startswith("## "):
        topic = line[3:].strip(); continue
    if not line or line.startswith("#"):
        continue
    i = find(line)
    if i is None:
        missing.append(line); continue
    S[i, col[topic]] += 1.0; why[i].append(f"seed:{topic}")
log(f"hand seeds: {sum(1 for _ in why):,} found, {len(missing)} missing: {missing[:40]}")

imp_re = re.compile(r"^(Top|High|Mid|Low|Bottom|NA|Unknown)-importance_(.+)_articles$")
vital_re = re.compile(r"^Wikipedia_level-(\d)_vital_articles")
proj_count = defaultdict(int)
with open(W / "assess.tsv", encoding="utf8") as f:
    for line in f:
        pid, c = line.rstrip("\n").split("\t")
        pid = int(pid)
        i = pid2idx[pid] if pid < len(pid2idx) else -1
        if i < 0:
            continue
        m = vital_re.match(c)
        if m:
            lv = int(m.group(1)); S[i, col["general"]] = max(S[i, col["general"]], VITAL[lv])
            if lv <= 4: why[i].append(f"vital{lv}")
            continue
        m = imp_re.match(c)
        if not m:
            continue
        name = m.group(2).replace("_", " ").lower()
        proj_count[name] += 1
        if name in PROJECTS and m.group(1) in IMPORTANCE:
            tp, pw = PROJECTS[name]
            w = pw * IMPORTANCE[m.group(1)]
            S[i, col[tp]] = max(S[i, col[tp]], w)
            if m.group(1) in ("Top", "High"): why[i].append(f"wp:{name}:{m.group(1).lower()}")
used = sorted((n, c) for n, c in proj_count.items() if n in PROJECTS)
log(f"WikiProjects used: {used}")
(W / "wikiprojects.tsv").write_text("".join(f"{n}\t{c}\n" for n, c in sorted(proj_count.items(), key=lambda x: -x[1])))
cat_rules = [(re.compile(r, re.M), col[t or "shelter"], w) for r, t, w in CAT_RULES]
for i, cs in enumerate(cats):
    if not cs:
        continue
    joined = "\n".join(cs)
    for r, k, w in cat_rules:
        for m in r.finditer(joined):
            S[i, k] = max(S[i, k], w)
            why[i].append(f"cat:{m.group(0)}")
log(f"seed entries: {len(S):,}")
ij = np.array(list(S.keys()), np.int64); vals = np.array(list(S.values()), np.float32)
Smat = np.zeros((N, K + 1), np.float32)
Smat[ij[:, 0], ij[:, 1]] = vals
Smat[:, K] = 1.0                       # last column: global PageRank (uniform teleport)
Smat /= Smat.sum(0, keepdims=True)

# ---------- graph and PageRank (cached: the seeds and settings are hashed, so tuning the scoring below is quick)
import hashlib
key = hashlib.sha1(Smat.tobytes() + f"{a.alpha},{a.iters}".encode()).hexdigest()[:12]
cache = W / f"ppr-{key}.npy"
if cache.exists():
    x = np.load(cache); log(f"PPR from {cache.name}")
    indeg = np.load(W / "indeg.npy")
else:
    e = np.load(W / "edges.npy")
    src = pid2idx[np.minimum(e[:, 0], len(pid2idx) - 1)]; dst = pid2idx[np.minimum(e[:, 1], len(pid2idx) - 1)]
    ok = (src >= 0) & (dst >= 0) & (e[:, 0] < len(pid2idx)) & (e[:, 1] < len(pid2idx))
    src, dst = src[ok].astype(np.int32), dst[ok].astype(np.int32)
    del e, ok
    outdeg = np.bincount(src, minlength=N).astype(np.float32)
    indeg = np.bincount(dst, minlength=N); np.save(W / "indeg.npy", indeg)
    log(f"edges in articles: {len(src):,}")
    # M[dst, src] = 1/outdeg(src): x_new = M @ x moves mass along links
    M = sp.csr_matrix((1.0 / outdeg[src], (dst, src)), shape=(N, N), dtype=np.float32)
    del src, dst
    dangling = outdeg == 0
    x = Smat.copy()
    for it in range(a.iters):
        lost = x[dangling].sum(0)
        xn = (1 - a.alpha) * (M @ x + Smat * lost[None, :]) + a.alpha * Smat
        d = np.abs(xn - x).sum(0).max(); x = xn
        if it % 5 == 0 or d < 1e-5:
            log(f"iter {it} delta {d:.2e}")
        if d < 1e-5:
            break
    del M
    np.save(cache, x)
glob = x[:, K]; ppr = x[:, :K]

# ---------- score
tw = np.array([TOPIC_W[t] for t in TOPICS], np.float32)
rel = (ppr * tw[None, :]).sum(1)
spec = rel / np.power(glob + 1e-12, a.beta)
pv = np.zeros(N, np.float64)
with open(a.pageviews) as f:
    for line in f:
        p, v = line.split("\t")
        p = int(p)
        if p < len(pid2idx) and pid2idx[p] >= 0:
            pv[pid2idx[p]] = int(v)
quality = np.log1p(pv) + 1.0 if a.pv_exp == 0 else np.power(pv + 100.0, a.pv_exp)
pen = np.ones(N)
handseed_pre = np.zeros(N, bool)
for i, rs in why.items():
    if any(r.startswith("seed:") for r in rs): handseed_pre[i] = True
pc = [(re.compile(r, re.M), f) for r, f in PENALTY_CATS]; pt = [(re.compile(r), f) for r, f in PENALTY_TITLE]
for i in range(N):
    if handseed_pre[i]:
        continue
    f = 1.0
    for r, v in pt:
        if r.search(titles[i]): f = min(f, v)
    if cats[i]:
        joined = "\n".join(cats[i])
        for r, v in pc:
            if r.search(joined): f = min(f, v)
    pen[i] = f
seeded = Smat[:, :K].sum(1) > 0
# hand seeds are always in: a big floor under their score
score = np.sqrt(spec * rel) * quality * pen
# usefulness per byte (DESIGN §6), softened so short stubs don't crowd out the substantial articles
score = score / np.power(np.maximum(nch, 1), a.gamma)
handseed = np.zeros(N, bool)
for i, rs in why.items():
    if any(r.startswith("seed:") for r in rs): handseed[i] = True
score[handseed] = score.max() * 10 + score[handseed]
score[nch == 0] = 0
order = np.argsort(-score)
log("scored")

# ---------- tiers
cum = np.cumsum(nch[order] * BYTES_PER_CHUNK)
tiers = [(n, float(b)) for n, b in (x.split(":") for x in a.tiers.split(","))]
tier_of = np.full(N, "", object)
cut = {}
for n, b in reversed(tiers):
    k = int(np.searchsorted(cum, b)); cut[n] = k
    tier_of[order[:k]] = n
rank_of = np.empty(N, np.int64); rank_of[order] = np.arange(N)
top_topic = np.array(TOPICS)[ppr.argmax(1)]

out = W / f"essentials-wikipedia-en{a.tag}.tsv"
with open(out, "w", encoding="utf8") as f:
    f.write("rank\tpage_id\ttitle\ttopic\tscore\tchunks\test_bytes\tcum_bytes\ttier\tpageviews_4mo\twhy\n")
    for r, i in enumerate(order[:cut[tiers[-1][0]]]):
        rs = why.get(i, [])
        f.write(f"{r + 1}\t{pids[i]}\t{titles[i]}\t{top_topic[i]}\t{score[i]:.4g}\t{nch[i]}\t{nch[i] * BYTES_PER_CHUNK}\t{cum[r]}\t{tier_of[i]}\t{int(pv[i])}\t{','.join(dict.fromkeys(rs)) or 'links'}\n")
log(f"wrote {out}")

summary = {"articles": N, "alpha": a.alpha, "beta": a.beta, "bytes_per_chunk": BYTES_PER_CHUNK, "missing_hand_seeds": missing,
           "tiers": {n: {"articles": cut[n], "bytes": int(cum[cut[n] - 1]), "chunks": int(nch[order[:cut[n]]].sum()),
                         "images": int(nimg[order[:cut[n]]].sum()),
                         "topics": {t: int(c) for t, c in zip(*np.unique(top_topic[order[:cut[n]]], return_counts=True))}}
                     for n, _ in tiers}}
if a.questions:
    qs = [json.loads(l) for l in open(a.questions)]
    found = [(q, find(q["title"])) for q in qs]
    dab = lambda i: any(c.startswith(("Disambiguation pages", "Set index articles")) or "disambiguation pages" in c for c in cats[i])
    ok = [(q, i) for q, i in found if i is not None and not dab(i)]
    cov = {n: round(sum(rank_of[i] < cut[n] for _, i in ok) / max(1, len(ok)), 3) for n, _ in tiers}
    by_topic = {}
    for t in sorted({q["topic"] for q, _ in ok}):
        sub = [i for q, i in ok if q["topic"] == t]
        by_topic[t] = {n: round(sum(rank_of[i] < cut[n] for i in sub) / len(sub), 2) for n, _ in tiers}
    misses = sorted({titles[i] for _, i in ok if rank_of[i] >= cut[tiers[0][0]]}, key=lambda t: rank_of[by_title[t]])
    summary["questions"] = {"total": len(qs), "title_found": len(ok), "title_ambiguous": sum(1 for q, i in found if i is not None and dab(i)), "coverage": cov, "by_topic": by_topic,
                            "essentials_misses": [(t, int(rank_of[by_title[t]]) + 1) for t in misses]}
(W / f"essentials-summary{a.tag}.json").write_text(json.dumps(summary, indent=1, ensure_ascii=False))
print(json.dumps({k: v for k, v in summary.items() if k != "questions"} | {"coverage": summary.get("questions", {}).get("coverage")}, indent=1)[:3000])
