"""Survival test questions for the essentials selection: an LLM writes realistic questions per topic, each
with the English Wikipedia article that best answers it. Written without seeing our seed list.

  uv run python essentials/gen_questions.py --out ../eval/essentials-questions.jsonl

Output rows: {"topic", "angle", "q", "title"}. Titles are checked against the article table later (rank.py).
"""
import argparse, json, re, urllib.request
from concurrent.futures import ThreadPoolExecutor

ap = argparse.ArgumentParser()
ap.add_argument("--out", required=True)
ap.add_argument("--url", default="http://localhost:19000/v1/chat/completions")
ap.add_argument("--model", default="qwen3.6-35b-a3b-claude-4.7-opus-reasoning-distilled-apex-i-balanced")
ap.add_argument("-n", type=int, default=25, help="questions per (topic, angle)")
ap.add_argument("--workers", type=int, default=4)
a = ap.parse_args()

TOPICS = {
    "medicine": "first aid, injuries, illness, medicines, childbirth, mental health",
    "water": "finding, purifying and storing drinking water; toilets, sanitation and hygiene",
    "food": "cooking without mains power, preserving and storing food, nutrition, fishing, hunting and butchery",
    "agriculture": "growing vegetables and grain, soil, seeds, keeping chickens, goats and bees, animal health",
    "foraging": "wild food and poisonous plants and fungi in Britain, seashore food, medicinal plants",
    "shelter": "staying warm and dry, fire lighting, building and repairing shelters and houses, knots and rope",
    "hazards": "floods, storms, heatwaves, cold, fires, pandemics, power cuts, radiation and chemical accidents, rescue",
    "energy": "electricity basics, batteries, solar panels, generators, wiring safely, lighting and heating without the grid",
    "tools": "hand tools, sharpening, metalwork, woodwork, sewing, bicycles, small engines, repairs",
    "radio": "radio and communications when phones and internet are down, antennas, Morse code, signalling for help",
    "chemistry": "useful practical chemistry: soap, disinfectants, fermentation, alcohol, metals, corrosion, materials",
    "navigation": "maps, compasses, finding direction without GPS, weather signs, tides",
    "science": "maths, measurement and units, basic physics and biology, teaching children",
}
ANGLES = {
    "howto": "practical how-to questions (\"how do I ...\")",
    "symptom": "questions describing a situation or symptom in everyday words, without naming the technical term",
    "what": "\"what is ...\" or identification questions about a specific thing, substance, plant, disease or device",
}
PROMPT = """You are helping build an offline library for people in the United Kingdom during a long emergency
(no internet, no shops, possibly no doctor). Topic: {topic} ({desc}).

Write {n} different questions that a real person might type into the library's search box. Style: {angle}.
Vary them widely across the topic. For each, give the title of the ONE existing English Wikipedia article
that best answers it (an exact article title, e.g. "Oral rehydration therapy", not a section or a made-up title).

Output exactly {n} lines, each a JSON object: {{"q": "...", "title": "..."}}. No other text."""


def ask(job):
    for attempt in range(3):
        try:
            return ask1(job)
        except Exception as e:  # timeouts, server restarts: try again, then give up on this job
            print(job, "failed:", e, flush=True)
    return []


def ask1(job):
    topic, angle = job
    req = {"model": a.model, "temperature": 0.8, "max_tokens": 12000,
           "messages": [{"role": "user", "content": PROMPT.format(topic=topic, desc=TOPICS[topic], n=a.n, angle=ANGLES[angle])}]}
    r = urllib.request.Request(a.url, json.dumps(req).encode(), {"content-type": "application/json"})
    text = json.load(urllib.request.urlopen(r, timeout=900))["choices"][0]["message"]["content"]
    text = re.sub(r"<think>.*?</think>", "", text, flags=re.S)
    out = []
    for m in re.finditer(r'\{[^{}]*"q"[^{}]*\}', text):   # one object per line, or a JSON array, or fenced
        try:
            o = json.loads(m.group(0))
        except ValueError:
            continue
        if o.get("q") and o.get("title"):
            out.append({"topic": topic, "angle": angle, "q": o["q"].strip(), "title": o["title"].strip()})
    if not out:
        print(topic, angle, "unparsed:", text[-300:].replace("\n", " "), flush=True)
    out = out[: a.n * 2]   # the model sometimes loops; keep topics balanced
    print(topic, angle, len(out), flush=True)
    return out


# resumable: (topic, angle) pairs already in the output are skipped
import os
seen, done = set(), set()
if os.path.exists(a.out):
    for line in open(a.out):
        o = json.loads(line); seen.add(o["q"].lower()); done.add((o["topic"], o["angle"]))
jobs = [(t, g) for t in TOPICS for g in ANGLES if (t, g) not in done]
print("jobs:", jobs, flush=True)
with ThreadPoolExecutor(a.workers) as ex, open(a.out, "a") as f:
    for rows in ex.map(ask, jobs):
        for o in rows:
            k = o["q"].lower()
            if k not in seen:
                seen.add(k); f.write(json.dumps(o, ensure_ascii=False) + "\n"); f.flush()
