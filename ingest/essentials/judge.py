"""Precision check for an essentials list: an LLM judges a random sample of a tier's articles as useful or not
for an offline survival library (UK). Titles only, in batches.

  uv run python essentials/judge.py <essentials tsv> [--tier essentials] [-n 300]
"""
import argparse, json, random, re, urllib.request
from concurrent.futures import ThreadPoolExecutor

ap = argparse.ArgumentParser()
ap.add_argument("tsv")
ap.add_argument("--tier", default="essentials")
ap.add_argument("-n", type=int, default=300)
ap.add_argument("--seed", type=int, default=1)
ap.add_argument("--url", default="http://localhost:19000/v1/chat/completions")
ap.add_argument("--model", default="qwen3.6-35b-a3b-claude-4.7-opus-reasoning-distilled-apex-i-balanced")
a = ap.parse_args()
rows = [l.rstrip("\n").split("\t") for l in open(a.tsv, encoding="utf8")]
hdr = rows[0]; rows = [dict(zip(hdr, r)) for r in rows[1:] if dict(zip(hdr, r)).get("tier") == a.tier]
random.Random(a.seed).shuffle(rows)
sample = rows[: a.n]
PROMPT = """We are choosing which English Wikipedia articles go into a small offline library (about 1 GB) for people in the
United Kingdom during a long emergency: no internet, maybe no doctor, no shops. Space is tight.
For each title below, answer 1 if the article would be genuinely useful to have in that small library (practical
knowledge: health, first aid, water, food, farming, wild plants and fungi, shelter, energy, tools, repair, radio,
chemistry, navigation, basic science and maths, coping), else 0.

{items}

Answer with one line per title: the number, a space, then 1 or 0. Nothing else."""


def judge(batch):
    items = "\n".join(f"{i + 1}. {r['title']}" for i, r in enumerate(batch))
    req = {"model": a.model, "temperature": 0, "max_tokens": 8000, "messages": [{"role": "user", "content": PROMPT.format(items=items)}]}
    for _ in range(3):
        try:
            r = urllib.request.Request(a.url, json.dumps(req).encode(), {"content-type": "application/json"})
            text = json.load(urllib.request.urlopen(r, timeout=900))["choices"][0]["message"]["content"]
            text = re.sub(r"<think>.*?</think>", "", text, flags=re.S)
            got = {int(m.group(1)): int(m.group(2)) for m in re.finditer(r"^\s*(\d+)[.):]?\s+([01])\s*$", text, re.M)}
            if len(got) >= len(batch) * .8:
                return [(r, got.get(i + 1)) for i, r in enumerate(batch)]
        except Exception as e:
            print("retry:", e, flush=True)
    return [(r, None) for r in batch]


batches = [sample[i:i + 25] for i in range(0, len(sample), 25)]
with ThreadPoolExecutor(3) as ex:
    res = [x for b in ex.map(judge, batches) for x in b]
scored = [(r, v) for r, v in res if v is not None]
yes = sum(v for _, v in scored)
print(json.dumps({"tsv": a.tsv, "tier": a.tier, "judged": len(scored), "useful": round(yes / max(1, len(scored)), 3),
                  "not_useful_examples": [r["title"] for r, v in scored if v == 0][:40]}, ensure_ascii=False))
