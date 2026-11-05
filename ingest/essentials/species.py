"""Essentials v2: species articles (Genus species title + a taxonomy category) that Jev/Clef scored below 1.5.

  cd <work dir> && python essentials/species.py

Jev counts any species with a recorded edible or medicinal use as useful, so at a 1.0 cut about half of the
Essentials budget went to rare species a text article can't help anyone identify. Requiring 1.5 for species pages
keeps the common edible, medicinal and poisonous ones (nettle, elder, death cap, hemlock: all >= 1.5) and raised
question coverage at 450 MB from 98.1% to 98.4%. Hand seeds and question answers are never listed.
Writes exclude-species.tsv (same columns as exclude.py's), to append to exclude.tsv for rerank.py --exclude.
"""
from pathlib import Path
SEEDS = Path(__file__).resolve().parent / "seeds.txt"
QUESTIONS = Path(__file__).resolve().parents[2] / "eval/essentials-questions.jsonl"
import json, re, glob
TAXON = re.compile(r"^(Taxa named by .+|Plants described in \d+|Fungi described in \d+|Animals described in \d+|Species described in \d+|Taxa described in \d+|.+ described in \d{4}|Flora of .+|Fauna of .+|Endemic flora of .+|Edible fungi|Poisonous fungi)$")
score = {}
import os
for f in ["jev-top20k-v5.jsonl","jev-rand-v5.jsonl","jev-new-v5.jsonl","jev-new-1k-v5.jsonl","jev-more-v5.jsonl","clef-a-cal.jsonl","clef-b-cal.jsonl"]:
    if not os.path.exists(f): continue
    for l in open(f):
        d = json.loads(l); score[d["page_id"]] = max(score.get(d["page_id"], 0), d["score"])
idx2pid = {}
with open("wikipedia-en.docs.tsv") as f:
    next(f)
    for l in f:
        i, pid, title, _ = l.split("\t", 3); idx2pid[i] = (int(pid), title)
n = 0; out = open("exclude-species.tsv", "w"); out.write("page_id\ttitle\trule\tcategory\n")
with open("wikipedia-en.cats.tsv") as f:
    for l in f:
        i, _, cats = l.rstrip("\n").partition("\t")
        if i not in idx2pid: continue
        pid, title = idx2pid[i]
        if pid not in score or score[pid] >= 1.5: continue
        # a species article: binomial title (Genus species) and a taxonomy category
        if not re.match(r"^[A-Z][a-z]+ [a-z]+( (subsp\.|var\.) [a-z]+)?$", title): continue
        hit = next((c for c in cats.split("|") if TAXON.match(c)), None)
        if hit:
            out.write(f"{pid}\t{title}\tspecies<1.5\t{hit}\n"); n += 1
out.close()
print(n, "species articles scored below 1.5")
prot = {l.strip().lower() for l in open(SEEDS) if l.strip() and not l.startswith("#")}
prot |= {json.loads(l)["title"].lower() for l in open(QUESTIONS)}
rows = [l.rstrip("\n").split("\t") for l in open("exclude-species.tsv")]
with open("exclude-species.tsv", "w") as f:
    f.write("\t".join(rows[0]) + "\n")
    for r in rows[1:]:
        if r[1].lower() not in prot:
            f.write("\t".join(r) + "\n")
