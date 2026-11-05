"""Essentials selection for wikiHow: by its own category breadcrumbs (e.g. "Health|First Aid and Emergency Health Care").

  uv run python essentials/wikihow_select.py <work dir>

Reads <work>/wikihow-en.docs.tsv and .cats.tsv (docs_table.py). An article is in the essentials tier if any of
its breadcrumbs contains a CORE category and none contains a SKIP category; in the standard tier if its top-level
category is in STANDARD_TOP. Writes <work>/essentials-wikihow-en.tsv.
"""
import sys
from pathlib import Path

W = Path(sys.argv[1])
BYTES_PER_CHUNK = 267 + 150
CORE = {
    "First Aid and Emergency Health Care", "Disaster Preparedness", "Outdoor Safety", "Outdoor Recreation", "Camping",
    "Medicines and Drugs", "Medical Equipment", "Conditions and Treatments", "Infectious Diseases", "Injury and Accidents",
    "Wound First Aid", "Soft Tissue Injuries", "Pregnancy", "Childbirth", "Babies", "Baby Care", "Raising Children",
    "Water", "Food Preservation Techniques", "Basic Cooking Skills", "Breads", "Jams Preserves and Condiments", "Cooking Appliances",
    "Gardening", "Planting and Growing", "Garden Pests and Weeds", "Farming", "Farm Animals and Livestock", "Chickens", "Chicken",
    "Bees", "Goats", "Sheep", "Horses", "Rabbits",
    "Tools", "Fastening Tools", "Woodworking", "Sewing", "Sewing Clothes", "Knitting and Crochet", "Knitting", "Home Repairs",
    "Plumbing", "Electrical Maintenance", "Maintenance and Repair", "Heating and Cooling", "Car Maintenance and Repair", "Car Engines",
    "Bicycles", "Bicycle Maintenance", "Fire Safety", "Home Safety", "Hygiene", "Personal Hygiene", "Cleaning", "Laundry",
    "Psychological Health", "Mental Health", "Coping with Grief", "Stress Management", "Nutrition", "Hunting", "Fishing",
}
SKIP = {"Love", "Dating", "Hair Care", "Beauty", "Makeup", "Video Games", "Social Media", "Smartphones", "Personality Traits",
        "Fashion", "Interior Design and Decorating", "Holidays and Traditions", "Cocktails", "Party Planning", "Surviving School"}
STANDARD_TOP = {"Health", "Home and Garden", "Food and Entertaining", "Pets and Animals", "Cars & Other Vehicles", "Family Life",
                "Hobbies and Crafts", "Sports and Fitness", "Education and Communications", "Personal Care and Style", "Finance and Business"}

rows = [l.rstrip("\n").split("\t") for l in open(W / "wikihow-en.docs.tsv", encoding="utf8")][1:]
crumbs = {}
for line in open(W / "wikihow-en.cats.tsv", encoding="utf8"):
    i, c = line.rstrip("\n").split("\t", 1)
    crumbs[int(i)] = c.split("|")   # docs_table joins categories with '|'; for wikiHow they are the breadcrumb
tiers = {"essentials": [0, 0], "standard": [0, 0]}
with open(W / "essentials-wikihow-en.tsv", "w", encoding="utf8") as f:
    f.write("page_id\ttitle\ttier\tchunks\test_bytes\tcategory\n")
    for idx, pid, title, hb, ni, nc in rows:
        cs = crumbs.get(int(idx), [])
        s = set(cs)
        if s & SKIP or int(nc) == 0:
            continue
        tier = "essentials" if s & CORE else "standard" if cs and cs[0] in STANDARD_TOP else ""
        if not tier:
            continue
        b = int(nc) * BYTES_PER_CHUNK
        for t in ("essentials", "standard") if tier == "essentials" else ("standard",):
            tiers[t][0] += 1; tiers[t][1] += b
        f.write(f"{pid}\t{title}\t{tier}\t{nc}\t{b}\t{' > '.join(cs)}\n")
for t, (n, b) in tiers.items():
    print(f"{t}: {n:,} articles, {b / 1e6:.0f} MB text+BM25")
