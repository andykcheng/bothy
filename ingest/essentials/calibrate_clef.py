"""Essentials: put local Clef scores on Jev's scale, so rerank.py can mix them with Jev's.

  cd <work dir> && python essentials/calibrate_clef.py clef-a-v5.jsonl clef-a-cal.jsonl

Fits an isotonic (monotone) map from Clef to Jev on the audit set both scored (jev-audit600-v5.jsonl and
jev-audit600-clef5.jsonl, 614 articles) and writes each input row with `score` replaced by the calibrated value
(`clef_raw` keeps the original). On that set Clef 0.9 ~ Jev 0.88 and Clef 1.0 ~ Jev 1.05; correlation 0.89.
"""
import json, sys
import numpy as np


def load(f):
    return {json.loads(l)["page_id"]: json.loads(l)["score"] for l in open(f)}


def fit(jev_file="jev-audit600-v5.jsonl", clef_file="jev-audit600-clef5.jsonl"):
    j, c = load(jev_file), load(clef_file)
    ids = [i for i in j if i in c]
    x, y = np.array([c[i] for i in ids]), np.array([j[i] for i in ids])
    o = np.argsort(x); x, y = x[o], y[o]
    st = []  # pool adjacent violators: [sum, n, x_lo, x_hi]
    for v, xi in zip(y, x):
        st.append([v, 1, xi, xi])
        while len(st) > 1 and st[-2][0] / st[-2][1] > st[-1][0] / st[-1][1]:
            b2, b1 = st.pop(), st.pop()
            st.append([b1[0] + b2[0], b1[1] + b2[1], b1[2], b2[3]])
    kx, ky = [], []
    for s, n, lo, hi in st:
        kx += [lo, hi]; ky += [s / n, s / n]
    return lambda v: float(np.interp(v, kx, ky))


def main():
    src, dst = sys.argv[1], sys.argv[2]
    f = fit()
    n = good = 0
    with open(dst, "w") as out:
        for l in open(src):
            d = json.loads(l)
            d["clef_raw"] = d["score"]
            d["score"] = round(f(d["score"]), 3)
            d["calibrated"] = "isotonic on jev-audit600 (Jev v5 vs Clef v5)"
            out.write(json.dumps(d) + "\n")
            n += 1; good += d["score"] >= 1.0
    print(f"{n:,} written to {dst}; calibrated >= 1.0: {good:,}")


if __name__ == "__main__":
    main()
