"""Essentials v2: a usefulness score for every Wikipedia article, learnt from Jev's scores of the v0 top 400k.

  uv run python essentials/classify_all.py <work dir> [--leads shard-leads] [--k 25] [--alpha 10] [--out scores-all.tsv]

Inputs in <work dir>: shard-leads/*.npz (shard_leads.py: page ids and lead embeddings), jev-top20k-v5.jsonl and
jev-rand-v5.jsonl (Jev v5, 0..3, for v0 ranks 1..400k), wikipedia-en.docs.tsv (titles, chunk counts) and
essentials-wikipedia-en.tsv (v0 ranks). The model sees the lead's content only: v0 rank exists just for the top
1.3M, and the labels stop at 400k, so a rank feature would not carry over to the rest.

Model: a blend of kNN (cosine^4-weighted mean Jev score of the k nearest labelled leads) and ridge regression,
weight chosen on 5-fold held-out data. Also reports how it does on ranks 200k-400k when trained on 1-200k (the
deeper half, closer to the unlabelled tail). Output: page_id, title, chunks, v0_rank, jev, pred, final (= jev when
scored, else pred), sorted by final.
"""
import argparse, json, time
from pathlib import Path

import numpy as np
import torch
from scipy.stats import spearmanr

ap = argparse.ArgumentParser()
ap.add_argument("work")
ap.add_argument("--leads", default="shard-leads")
ap.add_argument("--k", type=int, default=25)
ap.add_argument("--alpha", type=float, default=10.0)
ap.add_argument("--out", default="scores-all.tsv")
ap.add_argument("--labels", nargs="*", default=["jev-top20k-v5.jsonl", "jev-rand-v5.jsonl", "jev-new-v5.jsonl",
                "jev-new-1k-v5.jsonl", "jev-more-v5.jsonl"], help="Jev v5 result files to learn from (missing ones are skipped)")
a = ap.parse_args()
W = Path(a.work)
t0 = time.time()
log = lambda *x: print(f"[{time.time() - t0:6.0f}s]", *x, flush=True)

ids, embs = [], []
for f in sorted((W / a.leads).glob("*.npz")):
    z = np.load(f)
    ids.append(z["ids"]); embs.append(z["emb"])
ids = np.concatenate(ids)
X = np.concatenate(embs)
ids, first = np.unique(ids, return_index=True)   # an article appears once; keep its first lead
X = X[first]
pos = {int(p): i for i, p in enumerate(ids)}
log(f"{len(ids):,} articles with lead embeddings, {X.shape[1]} dims")

jev = {}
for fn in a.labels:
    if not (W / fn).exists():
        log(f"labels: {fn} missing, skipped")
        continue
    for l in open(W / fn):
        d = json.loads(l)
        if d["page_id"] in pos:
            jev[d["page_id"]] = d["score"]
lab = np.array([pos[p] for p in jev]); y = np.array(list(jev.values()), dtype=np.float32)
v0 = {}
with open(W / "essentials-wikipedia-en.tsv", encoding="utf8") as f:
    next(f)
    for line in f:
        r = line.split("\t", 3)
        v0[int(r[1])] = int(r[0])
rank_lab = np.array([v0.get(int(ids[i]), 10**7) for i in lab])
log(f"{len(y):,} Jev-scored articles with embeddings")

dev = "cuda"
Xt = torch.from_numpy(X).to(dev)            # float16
F32 = lambda idx: Xt[torch.from_numpy(idx).to(dev)].float()


def ridge_fit(idx, yy):
    F = torch.cat([F32(idx), torch.ones(len(idx), 1, device=dev)], 1).double()
    A = F.T @ F + a.alpha * torch.eye(F.shape[1], device=dev, dtype=torch.float64)
    return torch.linalg.solve(A, F.T @ torch.from_numpy(yy).to(dev).double()).float()


def ridge_pred(w, idx):
    out = []
    for i in range(0, len(idx), 200000):
        q = F32(idx[i: i + 200000])
        out.append((q @ w[:-1] + w[-1]).cpu().numpy())
    return np.concatenate(out)


def knn_pred(train_idx, yy, query_idx, exclude=None):
    T = Xt[torch.from_numpy(train_idx).to(dev)]
    yt = torch.from_numpy(yy).to(dev)
    tid = torch.from_numpy(train_idx).to(dev)
    out = []
    for i in range(0, len(query_idx), 2048):
        qi = torch.from_numpy(query_idx[i: i + 2048]).to(dev)
        s = (Xt[qi] @ T.T).float()
        if exclude:
            s[qi[:, None] == tid[None, :]] = -1   # an article never votes for itself
        v, j = s.topk(a.k, dim=1)
        w = v.clamp(min=0) ** 4
        out.append(((w * yt[j]).sum(1) / w.sum(1).clamp(min=1e-6)).cpu().numpy())
    return np.concatenate(out)


def report(name, p, yy):
    r = spearmanr(p, yy)[0]
    out = [f"{name}: spearman {r:.3f}  MAE {np.abs(p - yy).mean():.3f}"]
    for thr in (1.0, 1.5):
        top = p >= np.quantile(p, 1 - (yy >= thr).mean())   # as many picked as there are good ones
        out.append(f"jev>={thr}: precision=recall {(yy[top] >= thr).mean():.2f}")
    log("  ".join(out))
    return r


# 1. held-out: 5 folds over all labels
rng = np.random.default_rng(0)
perm = rng.permutation(len(y)); folds = np.array_split(perm, 5)
pk, pr, yy = [], [], []
for f in folds:
    tr = np.setdiff1d(perm, f)
    w = ridge_fit(lab[tr], y[tr])
    pr.append(ridge_pred(w, lab[f])); pk.append(knn_pred(lab[tr], y[tr], lab[f])); yy.append(y[f])
pk, pr, yy = map(np.concatenate, (pk, pr, yy))
best = max((report(f"5-fold blend knn={b:.2f}", b * pk + (1 - b) * pr, yy), b) for b in (0, .25, .5, .6, .75, 1))
B = best[1]
log(f"using knn weight {B}")

# 2. the deeper half, as a stand-in for the unlabelled tail
shallow, deep = rank_lab <= 200000, (rank_lab > 200000) & (rank_lab <= 400000)
w = ridge_fit(lab[shallow], y[shallow])
p = B * knn_pred(lab[shallow], y[shallow], lab[deep]) + (1 - B) * ridge_pred(w, lab[deep])
report("train ranks<=200k, test 200k-400k", p, y[deep])

# 3. everything
w = ridge_fit(lab, y)
allidx = np.arange(len(ids))
pred = B * knn_pred(lab, y, allidx, exclude=True) + (1 - B) * ridge_pred(w, allidx)
log("predicted all")

title, chunks = {}, {}
with open(W / "wikipedia-en.docs.tsv", encoding="utf8") as f:
    next(f)
    for line in f:
        r = line.rstrip("\n").split("\t")
        title[int(r[1])] = r[2]; chunks[int(r[1])] = int(r[5])
final = np.array([jev.get(int(p), pred[i]) for i, p in enumerate(ids)], dtype=np.float32)
order = np.argsort(-final, kind="stable")
with open(W / a.out, "w", encoding="utf8") as f:
    f.write("page_id\ttitle\tchunks\tv0_rank\tjev\tpred\tfinal\n")
    for i in order:
        p = int(ids[i]); j = jev.get(p)
        f.write(f"{p}\t{title.get(p, '')}\t{chunks.get(p, 0)}\t{v0.get(p, '')}\t{'' if j is None else round(j, 3)}\t{pred[i]:.3f}\t{final[i]:.3f}\n")
new = np.array([int(ids[i]) not in jev for i in range(len(ids))])
for thr in (1.0, 1.5, 2.0):
    log(f"pred >= {thr} among the {new.sum():,} unscored: {(pred[new] >= thr).sum():,}")
log("wrote", W / a.out)
