"""Essentials: kinds of Wikipedia article never worth a place in a survival library, by exact category patterns.

  uv run python essentials/exclude.py <work dir> [--out exclude.tsv] [--sample 15]

Inputs in <work dir>: wikipedia-en.docs.tsv and wikipedia-en.cats.tsv (docs_table.py), the Jev results
(jev-*.jsonl, for the report only). Writes <out>: page_id, title, rule, the category that matched.

Excluded: people (birth/death-year categories, "Living people"), films, albums, songs, TV, video games, sport
(seasons, clubs, competitions), battles and wars, elections. Patterns are exact (e.g. "2004 films", never any
category containing "film"), because loose words catch practical articles ("Thin film", "2-meter band").
History as such is NOT excluded: history categories sit on many practical articles (Mason jar, Cloth face mask).

Never excluded (protected): the hand seeds (seeds.txt), every answer article of the essentials questions, and
MUST_KEEP below. The report lists protected articles a rule would have caught, and every excluded article Jev
scored >= 1.0, so the rules can be checked by eye.
"""
import argparse, collections, json, random, re, sys, unicodedata
from pathlib import Path

HERE = Path(__file__).resolve().parent
RULES = [
    ("people", r"^(\d{1,4}s?( BC)? (births|deaths)|Living people|Year of (birth|death) (missing|unknown)( \(living people\))?"
               r"|Possibly living people|Date of (birth|death) (missing|unknown)|Place of (birth|death) missing)$"),
    ("film", r"^(\d{4} films|Films (directed|produced|written|scored|set|shot) .+|.+ films? (series|franchises))$"),
    ("music", r"^(\d{4} (debut |live |compilation |soundtrack |video )?(albums|EPs|songs|singles|mixtapes)"
              r"|Songs (written|composed|recorded) by .+|.+ (albums|songs) produced by .+|Musical groups (established|from) .+)$"),
    ("tv", r"^(\d{4} .*television (series|episodes|films|specials)( debuts| endings)?|.+ television seasons"
           r"|.+ \(TV series\) episodes|.+ television series episodes|.+ television programs)$"),
    ("video game", r"^(\d{4} video games|Video games (developed|published|set|scored) .+)$"),
    ("sport", r"^(\d{4}([–-]\d{2,4})? in (sports?|.*football|baseball|basketball|ice hockey|cricket|tennis|golf"
              r"|motorsport|rugby (union|league)|boxing|cycling|athletics|swimming)( .+)?"
              r"|(?!(?i:.*\b(hurricane|cyclone|typhoon|storm|influenza|flu|monsoon|wildfire|fire|tornado|rainy|dry|growing|harvest|breeding|hunting|fishing)\b)).+ seasons"
              r"|.+ (football|soccer|basketball|baseball|hockey|rugby|cricket|handball|volleyball) clubs( .+)?"
              r"|Olympic .+ (for|of) .+|.+ at the \d{4} .+ (Olympics|Games|Championships)|.+ Grand Prix)$"),
    ("battle/war", r"^(Battles (of|involving|in) .+|Conflicts in \d{3,4}s?|Military operations (of|involving) .+"
                   r"|Sieges (of|involving) .+|Naval battles (of|involving) .+)$"),
    ("election", r"^(\d{4} elections in .+|Elections in .+|\d{4} .+ elections|Referendums in .+)$"),
]
RULES = [(name, re.compile(rx)) for name, rx in RULES]

# Core knowledge that must survive any rule (checked by name in the report); hand seeds and question answers
# are protected too.
MUST_KEEP = """Paracetamol|Ibuprofen|Aspirin|Distillation|Water purification|Boiling|Oral rehydration therapy
|Cardiopulmonary resuscitation|Tourniquet|Penicillin|Antibiotic|Insulin|Vaccine|Sanitation|Cholera|Fire|Knot
|Food preservation|Canning|Fermentation|Soap|Bleach|Water filter|Compass|Battery (electricity)|Solar panel
|Electric generator|Radio|Morse code|Bread|Potato|Germ theory of disease|Wound|Fracture|Hypothermia""".replace("\n", "")


def norm(t: str) -> str:
    t = unicodedata.normalize("NFKC", t).replace("_", " ").strip()
    return " ".join(t.split()).lower()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("work")
    ap.add_argument("--out", default="exclude.tsv")
    ap.add_argument("--questions", default=str(HERE.parents[1] / "eval/essentials-questions.jsonl"))
    ap.add_argument("--sample", type=int, default=15)
    ap.add_argument("--keep-jev", type=float, default=1.3, help="never exclude an article Jev scored at least this")
    a = ap.parse_args()
    W = Path(a.work)

    idx2pid, title_of, pid_of, exact = {}, {}, {}, {}
    with open(W / "wikipedia-en.docs.tsv", encoding="utf8") as f:
        next(f)
        for line in f:
            i, pid, title, _ = line.split("\t", 3)
            pid = int(pid)
            idx2pid[i] = pid
            title_of[pid] = title
            exact[title] = pid
            pid_of.setdefault(norm(title), pid)

    # exact title first ("Meat grinder" the device, not "Meat Grinder" the film), else case-insensitive
    def find(t):
        t = " ".join(t.replace("_", " ").split())
        return exact.get(t) or exact.get(t[:1].upper() + t[1:]) or pid_of.get(norm(t))
    protect = {}
    seeds = [l.strip() for l in open(HERE / "seeds.txt", encoding="utf8") if l.strip() and not l.startswith("#")]
    for t in seeds:
        protect[t] = "seed"
    for l in open(a.questions, encoding="utf8"):
        protect.setdefault(json.loads(l)["title"], "question")
    for t in MUST_KEEP.split("|"):
        protect.setdefault(t, "must-keep")
    protected = {find(t): why for t, why in protect.items() if find(t)}
    missing = [t for t in protect if not find(t)]

    jev = {}
    for fn in sorted(W.glob("jev-*.jsonl")):
        for l in open(fn, encoding="utf8"):
            try:
                d = json.loads(l)
                jev[d["page_id"]] = max(jev.get(d["page_id"], 0), d["score"])
            except (ValueError, KeyError):
                pass

    out, saved, kept = [], [], []
    counts = collections.Counter()
    with open(W / "wikipedia-en.cats.tsv", encoding="utf8") as f:
        for line in f:
            i, _, cats = line.rstrip("\n").partition("\t")
            pid = idx2pid.get(i)
            if pid is None:
                continue
            hit = None
            for c in cats.split("|"):
                for name, rx in RULES:
                    if rx.match(c):
                        hit = (name, c)
                        break
                if hit:
                    break
            if not hit:
                continue
            if pid in protected:
                saved.append((pid, hit))
                continue
            if jev.get(pid, 0) >= a.keep_jev:
                kept.append((pid, hit))
                continue
            counts[hit[0]] += 1
            out.append((pid, title_of[pid], *hit))

    with open(W / a.out, "w", encoding="utf8") as f:
        f.write("page_id\ttitle\trule\tcategory\n")
        for r in out:
            f.write("\t".join(map(str, r)) + "\n")

    n = len(idx2pid)
    print(f"{len(out):,} of {n:,} articles excluded ({100 * len(out) / n:.1f}%) -> {W / a.out}")
    for name, k in counts.most_common():
        print(f"  {name:12} {k:>10,}")
    print(f"\nprotected: {len(protected):,} articles ({len(missing)} protected titles not found as articles,"
          f" e.g. redirects: {', '.join(missing[:8])})")
    print(f"protected articles a rule would have caught ({len(saved)}):")
    for pid, (name, c) in saved:
        print(f"  {title_of[pid]}  [{protected[pid]}]  <- {name}: {c}")
    gone = {r[0] for r in out}
    print(f"kept because Jev >= {a.keep_jev}: " + "; ".join(f"{title_of[p]} ({jev[p]:.2f}, {h[0]})" for p, h in kept))
    must = [t for t in MUST_KEEP.split("|") if find(t) in gone]
    print(f"must-keep articles excluded: {must or 'none'}")

    scored = [r for r in out if r[0] in jev]
    good = sorted((r for r in scored if jev[r[0]] >= 1.0), key=lambda r: -jev[r[0]])
    print(f"\nJev-scored articles excluded: {len(scored):,}; of them Jev >= 1.0: {len(good)}"
          f" ({100 * len(good) / max(len(scored), 1):.2f}%)")
    for r in good[:40]:
        print(f"  {jev[r[0]]:.2f}  {r[1]}  <- {r[2]}: {r[3]}")
    random.seed(1)
    for name, _ in counts.most_common():
        rs = [r for r in out if r[2] == name]
        print(f"\nsample of {name}: " + "; ".join(f"{r[1]} ({r[3]})" for r in random.sample(rs, min(a.sample, len(rs)))))


if __name__ == "__main__":
    main()
