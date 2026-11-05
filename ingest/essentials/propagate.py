"""Extend Jev usefulness scores to every candidate article via lead embeddings.

  uv run python essentials/propagate.py <lead-emb dir> <out.tsv> --jev a.jsonl b.jsonl ... [--labels hand.jsonl ...]

Trains on the Jev-scored articles (score 0..3) and predicts a score for all candidates, using
  - kNN: cosine-weighted mean score of the k most similar Jev-scored leads, and
  - ridge regression on the embedding plus log(rank),
then a blend of the two. Reports held-out quality (5-fold on the random Jev sample, which represents the
unscored articles) and agreement with the hand labels. Output: page_id, rank, title, jev (if scored),
pred, final (= jev where scored, else the prediction).
"""
import argparse, json
from pathlib import Path

import numpy as np
import torch
from scipy.stats import spearmanr

ap = argparse.ArgumentParser()
ap.add_argument("emb"); ap.add_argument("out")
ap.add_argument("--leads", default=str(Path.home() / "sp-scratch/essentials/leads-400k.jsonl"))
ap.add_argument("--jev", nargs="+", required=True)
ap.add_argument("--labels", nargs="*", default=[])
ap.add_argument("--k", type=int, default=25)
ap.add_argument("--alpha", type=float, default=10.0, help="ridge penalty")
a = ap.parse_args()

E = Path(a.emb)
ids = np.load(E / "ids.npy")
blocks = sorted(E.glob("emb-*.npy"))
X = np.concatenate([np.load(b) for b in blocks]).astype(np.float32)
ids = ids[: len(X)]
pos = {int(p): i for i, p in enumerate(ids)}
meta = {}
for l in open(a.leads):
    r = json.loads(l); meta[r["page_id"]] = (r["rank"], r["title"])
rank = np.array([meta[int(p)][0] for p in ids], dtype=np.float32)
print(f"{len(X):,} embedded leads")

jev = {}
for fn in a.jev:   # later files win
    for l in open(fn):
        d = json.loads(l)
        if d["page_id"] in pos:
            jev[d["page_id"]] = d["score"]
lab_idx = np.array([pos[p] for p in jev]); y = np.array([jev[p] for p in jev], dtype=np.float32)
print(f"{len(y):,} Jev-scored leads with embeddings")

dev = "cuda" if torch.cuda.is_available() else "cpu"
Xt = torch.from_numpy(X).to(dev)
feat = lambda idx: np.hstack([X[idx], np.log(rank[idx])[:, None] / 10, np.ones((len(idx), 1), np.float32)])


def ridge_fit(idx, yy):
    F = torch.from_numpy(feat(idx)).to(dev).double()
    A = F.T @ F + a.alpha * torch.eye(F.shape[1], device=dev, dtype=torch.float64)
    return torch.linalg.solve(A, F.T @ torch.from_numpy(yy).to(dev).double()).float()


def ridge_pred(w, idx):
    out = []
    for i in range(0, len(idx), 50000):
        out.append((torch.from_numpy(feat(idx[i: i + 50000])).to(dev) @ w).cpu().numpy())
    return np.concatenate(out)


def knn_pred(train_idx, yy, query_idx, exclude_self=False):
    T = Xt[torch.from_numpy(train_idx).to(dev)]
    yt = torch.from_numpy(yy).to(dev)
    out = []
    for i in range(0, len(query_idx), 4096):
        q = Xt[torch.from_numpy(query_idx[i: i + 4096]).to(dev)]
        s = q @ T.T
        if exclude_self:
            s[s > 0.9999] = -1
        v, j = s.topk(a.k, dim=1)
        w = torch.clamp(v, min=0) ** 4
        out.append(((w * yt[j]).sum(1) / w.sum(1).clamp(min=1e-6)).cpu().numpy())
    return np.concatenate(out)


# Held-out check on the random Jev sample (ranks > 20000): 5 folds.
rand = np.array([i for i, p in enumerate(lab_idx) if rank[p] > 20000])
rng = np.random.default_rng(0); rng.shuffle(rand)
folds = np.array_split(rand, 5)
pk, pr, yt_all = [], [], []
for f in folds:
    tr = np.setdiff1d(np.arange(len(y)), f)
    w = ridge_fit(lab_idx[tr], y[tr])
    pr.append(ridge_pred(w, lab_idx[f])); pk.append(knn_pred(lab_idx[tr], y[tr], lab_idx[f])); yt_all.append(y[f])
pk, pr, yt_all = map(np.concatenate, (pk, pr, yt_all))
best = None
for b in (0, .25, .5, .75, 1):
    p = b * pk + (1 - b) * pr
    r = spearmanr(p, yt_all)[0]; mae = np.abs(p - yt_all).mean()
    # how well the prediction finds Jev >= 1.5 articles among the top 10% predicted
    top = p >= np.quantile(p, .9); prec = (yt_all[top] >= 1.5).mean(); rec = (yt_all[top] >= 1.5).sum() / max(1, (yt_all >= 1.5).sum())
    print(f"blend knn={b:.2f}: spearman {r:.3f}  MAE {mae:.3f}  top-10%: precision(jev>=1.5) {prec:.2f} recall {rec:.2f}")
    if best is None or r > best[1]:
        best = (b, r)
B = best[0]
print(f"using knn weight {B}")

# Fit on everything, predict all.
w = ridge_fit(lab_idx, y)
allidx = np.arange(len(X))
pred = B * knn_pred(lab_idx, y, allidx, exclude_self=True) + (1 - B) * ridge_pred(w, allidx)

for fn in a.labels:   # agreement with hand labels (prediction only, not Jev)
    L = [json.loads(l) for l in open(fn)]
    L = [d for d in L if d["page_id"] in pos]
    print(f"hand labels {Path(fn).name}: n={len(L)} spearman(pred) {spearmanr([pred[pos[d['page_id']]] for d in L], [d['label'] for d in L])[0]:.3f}")

with open(a.out, "w") as f:
    f.write("page_id\trank\ttitle\tjev\tpred\tfinal\n")
    for i, p in enumerate(ids):
        p = int(p); j = jev.get(p)
        f.write(f"{p}\t{int(rank[i])}\t{meta[p][1]}\t{'' if j is None else round(j, 3)}\t{pred[i]:.3f}\t{(j if j is not None else pred[i]):.3f}\n")
print("wrote", a.out)
