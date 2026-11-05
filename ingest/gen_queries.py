"""Synthetic known-item queries: an LLM writes one question per sampled chunk.

    uv run python gen_queries.py <extract_dir> -n 150 --out ../eval/synth-wikimed-zh.jsonl

Each output row: q, lang, title, doc_id, chunk_id, passage (first 300 chars).
Uses an OpenAI-compatible endpoint (default: the LAN llama.cpp server).
"""
import argparse, json, random, re, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import orjson

ap = argparse.ArgumentParser()
ap.add_argument("extract_dir")
ap.add_argument("-n", type=int, default=150)
ap.add_argument("--out", required=True)
ap.add_argument("--url", default="http://localhost:19000/v1/chat/completions")
ap.add_argument("--model", default="qwen3.6-35b-a3b-claude-4.7-opus-reasoning-distilled-apex-i-balanced")
ap.add_argument("--seed", type=int, default=7)
ap.add_argument("--workers", type=int, default=4)
a = ap.parse_args()

d = Path(a.extract_dir)
lang = json.loads((d / "source.json").read_text())["lang"]
LANG_NAME = {"en": "English", "zh": "Chinese", "ja": "Japanese"}[lang]
TABLEISH = re.compile(r"(\|.*){3,}|ICD|^\s*[\d.()\s–-]+$", re.M)

# Reservoir-sample substantive prose chunks (skip tiny, table-like and list-of-codes chunks).
rng = random.Random(a.seed)
pool = []
with open(d / "chunks.jsonl", "rb") as f:
    for i, line in enumerate(f):
        c = orjson.loads(line)
        body = c["text"].split("\n", 1)[-1]
        if c["n_tokens"] < 120 or TABLEISH.search(body):
            continue
        if len(pool) < a.n * 3:
            pool.append(c)
        else:
            j = rng.randrange(i + 1)
            if j < len(pool):
                pool[j] = c
rng.shuffle(pool)

PROMPT = """Here is a passage from an encyclopedia article titled "{title}".

{body}

Write ONE question in {lang_name} that a person might type into a search box, which this passage answers.
- Phrase it the way an ordinary person would ask, not by copying sentences from the passage.
- It may mention the topic, but must be answerable mainly from this passage.
Output only the question."""


def ask(c):
    title, body = c["text"].split("\n", 1)
    title = title.split(" > ")[0]
    req = {
        "model": a.model, "temperature": 0.7, "max_tokens": 120,
        "chat_template_kwargs": {"enable_thinking": False},
        "messages": [{"role": "user", "content": PROMPT.format(title=title, body=body[:2500], lang_name=LANG_NAME)}],
    }
    r = urllib.request.Request(a.url, data=json.dumps(req).encode(), headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r, timeout=300) as resp:
            q = json.load(resp)["choices"][0]["message"]["content"].strip().strip('"「」').splitlines()[0].strip()
    except Exception as ex:
        print("error:", ex)
        return None
    if not q or len(q) > 200:
        return None
    return {"q": q, "lang": lang, "title": title, "doc_id": c["doc_id"], "chunk_id": c["chunk_id"], "passage": body[:300]}


rows = []
with ThreadPoolExecutor(a.workers) as ex:
    for r in ex.map(ask, pool[: int(a.n * 1.1)]):
        if r and len(rows) < a.n:
            rows.append(r)
            if len(rows) % 25 == 0:
                print(len(rows), r["q"])
Path(a.out).write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows))
print(f"wrote {len(rows)} -> {a.out}")
