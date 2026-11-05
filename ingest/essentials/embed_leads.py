"""Embed each candidate article's lead (title + opening text) for the essentials classifier.

  uv run python essentials/embed_leads.py <leads.jsonl> <out dir> [--batch 64] [--block 20000]

Same model as the search index (Qwen3-Embedding-0.6B, document side, no prompt), full 1024 dims, L2-normalised,
stored as float16. Writes <out>/ids.npy (page ids, in leads order) and <out>/emb-NNNN.npy blocks, so it resumes
after a crash (finished blocks are skipped). Shares the GPU with the main embedding job.
"""
import argparse, json, time
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F
from transformers import AutoModel, AutoTokenizer

ap = argparse.ArgumentParser()
ap.add_argument("leads"); ap.add_argument("out")
ap.add_argument("--batch", type=int, default=64)
ap.add_argument("--block", type=int, default=20000)
ap.add_argument("--max-tokens", type=int, default=384)
a = ap.parse_args()

MODEL = "Qwen/Qwen3-Embedding-0.6B"
out = Path(a.out); out.mkdir(parents=True, exist_ok=True)
rows = [json.loads(l) for l in open(a.leads)]
np.save(out / "ids.npy", np.array([r["page_id"] for r in rows], dtype=np.int64))
texts = [f"{r['title']}\n\n{r['lead']}" for r in rows]

tok = AutoTokenizer.from_pretrained(MODEL, padding_side="left", local_files_only=True)
model = AutoModel.from_pretrained(MODEL, dtype=torch.bfloat16, attn_implementation="sdpa", local_files_only=True).cuda().eval()

t0 = time.time(); n_done = 0
for b0 in range(0, len(texts), a.block):
    path = out / f"emb-{b0 // a.block:04d}.npy"
    if path.exists():
        continue
    block = texts[b0: b0 + a.block]
    order = sorted(range(len(block)), key=lambda i: len(block[i]))
    vec = np.zeros((len(block), 1024), dtype=np.float16)
    for i in range(0, len(order), a.batch):
        idx = order[i: i + a.batch]
        enc = tok([block[j] for j in idx], padding=True, truncation=True, max_length=a.max_tokens, return_tensors="pt").to("cuda")
        with torch.inference_mode():
            h = model(**enc).last_hidden_state[:, -1].float()
        vec[idx] = F.normalize(h, dim=-1).cpu().numpy().astype(np.float16)
    np.save(str(path) + ".tmp.npy", vec); Path(str(path) + ".tmp.npy").rename(path)
    n_done += len(block)
    print(f"block {b0 // a.block}: {b0 + len(block):,}/{len(texts):,}  {n_done / (time.time() - t0):.0f}/s", flush=True)
print("done", flush=True)
