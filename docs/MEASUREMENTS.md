# Measurements — Milestone 1 prototype (2026-10-30)

Pack: `packs/proto` = WikiMed en/zh/ja + mdwiki-en (on NVMe scratch and BigBelly).
Built with `sp build` (default settings: table-only chunks excluded from the vector index, see §3).

## 1. Corpus

| source | docs | chunks | redirects (resolved) | docs.jsonl (raw) |
|---|---|---|---|---|
| wikimed-en | 72,866 | 570,520 | 298,275 | 1.13 GB |
| mdwiki-en | 71,589 | 570,930 | 292,157 | 1.11 GB |
| wikimed-ja | 13,164 | 134,141 | 17,132 | 226 MB |
| wikimed-zh | 15,810 | 104,345 | 31,073 | 174 MB |
| **total** | **173,429** | **1,379,936** | | **2.64 GB** |

Images: 202,262 unique (deduplicated across sources), almost all only the 320 px variant (mwoffliner thumbnails).

## 2. Pack sizes

| component | size | per unit | notes |
|---|---|---|---|
| docs (zstd + per-language dictionary) | 369 MB | 2.1 KB/doc, **267 B/chunk** | 7.2× smaller than raw JSONL (HTML included) |
| images (WebP q60, 320 px) | 2.43 GB | 12.0 KB/image | |
| BM25 en (tantivy, no positions) | 162 MB | 142 B/chunk | |
| BM25 zh (lindera CC-CEDICT) | 24 MB | 228 B/chunk | |
| BM25 ja (lindera UniDic) | 22 MB | 161 B/chunk | |
| vector index (IVF, 256-d sign bits + int8 + id) | 344 MB | **293 B/vector** | 1,174,036 vectors, 1,084 lists |
| titles (FST + table, incl. redirects) | 10 MB | | |
| chunks.bin, docs.idx, images.idx, dicts | 29 MB | | |
| model (Qwen3-Embedding-0.6B Q8_0 GGUF) | 610 MB | | |
| **total** | **3.9 GB** (3.3 GB without the model) | | |

### Against the DESIGN §6 budget

Per chunk, the vector index (293 B) costs as much as the compressed text it indexes (267 B). BM25 adds another ~150 B. The 50 GB table budgets 18 GB of text but only 4 GB of vectors and 5 GB of BM25. At the measured rates, 18 GB of text (~67M chunks) would need about **20 GB of vectors and 10 GB of BM25**. Full-text embedding of that many chunks is also ~8 GPU-days at the current ~95 chunks/s.

Ways to close the gap (not decided):
- **128-d vectors** (Matryoshka): 16 B codes + 128 B int8 ≈ 150 B/vector. Quality must be measured; we already have `full_f16.npy`.
- Keep only sign-bit codes on small phones and rerank with fewer int8 vectors or none: 36 B/vector, at a quality cost.
- Smaller text budget, or embed only the composed high-value subset and let BM25 cover the rest.
- Drop mdwiki-en for packs that already include wikimed-en: it is ~45% of this prototype and largely duplicates it.

## 3. Search quality

Evaluation sets:
- `eval/golden.jsonl`: 32 hand-written queries (en/zh/ja, plus 2 cross-lingual "x"), each with a list of expected titles.
- `eval/synth-wikimed-{en,zh,ja}.jsonl`: 150 known-item questions per language, written by Qwen3.6-35B-A3B from a random chunk. The target is that chunk's article.

Synthetic questions are often very specific (names, dates, numbers in one passage). That makes them harder than real queries and favours BM25. A miss can still return a reasonable answer from a related article.

Command: `eval_search.py --server … --golden ../eval/synth-wikimed-*.jsonl ../eval/golden.jsonl`. A hit means the title matches; duplicates from mdwiki count.

hit@1 / hit@5 / MRR@10:

| set | n | BM25 | dense | hybrid (RRF) |
|---|---|---|---|---|
| synth en | 150 | 0.69 / 0.81 / 0.74 | 0.49 / 0.70 / 0.59 | **0.74 / 0.92 / 0.80** |
| synth ja | 150 | 0.78 / 0.91 / 0.84 | 0.69 / 0.87 / 0.78 | **0.87 / 0.95 / 0.90** |
| synth zh | 150 | 0.80 / 0.91 / 0.85 | 0.78 / 0.93 / 0.85 | **0.88 / 0.98 / 0.93** |
| golden en | 10 | 0.60 / 0.70 / 0.65 | 0.70 / 0.90 / 0.77 | 0.80 / 0.80 / 0.83 |
| golden ja | 10 | 0.60 / 0.90 / 0.73 | 0.70 / 0.90 / 0.78 | 0.90 / 1.00 / 0.92 |
| golden zh | 10 | 0.70 / 1.00 / 0.82 | 0.90 / 0.90 / 0.91 | 0.90 / 1.00 / 0.93 |
| **all** | 482 | 0.75 / 0.87 / 0.80 | 0.66 / 0.84 / 0.75 | **0.83 / 0.95 / 0.88** |

Hybrid beats either method alone in every language. Dense does better on colloquial and cross-lingual golden queries. BM25 does better on the entity-heavy synthetic questions.

### Infobox chunks (fixed)

The first full build put every chunk in the vector index. Drug and chemical infobox chunks then appeared in dense results for unrelated queries; these are tables of IUPAC names, ATC codes and other identifiers. Ziconotide, Medrysone and Nicergoline appeared 14–18 times each in the dense top 5 across 482 queries. Chunks made only of `<table>` blocks are now left out of the vector index. That is 205,900 chunks (15%). They stay in BM25, where identifier lookups work well. `sp build --vec-tables` restores the old behaviour.

| dense, hit@1 / hit@5 | all chunks | without table-only chunks |
|---|---|---|
| synth en | 0.42 / 0.65 | 0.49 / 0.70 |
| synth ja | 0.66 / 0.79 | 0.69 / 0.87 |
| synth zh | 0.77 / 0.90 | 0.78 / 0.93 |
| hybrid, all sets | 0.83 / 0.94 | 0.83 / 0.95 |

The vector index also shrank from 404 MB to 344 MB.

### 256 vs 128 dims, without mdwiki (decided: ship both)

Pack: WikiMed en/zh/ja only (mdwiki dropped), 697,736 vectors. hit@1 / hit@5 / MRR:

| set | dense d256 | dense d128 | hybrid d256 | hybrid d128 |
|---|---|---|---|---|
| synth en | 0.50 / 0.75 / 0.61 | 0.50 / 0.75 / 0.61 | 0.74 / 0.96 / 0.83 | 0.73 / 0.94 / 0.82 |
| synth ja | 0.69 / 0.88 / 0.78 | 0.67 / 0.84 / 0.75 | 0.86 / 0.95 / 0.90 | 0.85 / 0.93 / 0.89 |
| synth zh | 0.78 / 0.93 / 0.85 | 0.74 / 0.89 / 0.81 | 0.89 / 0.98 / 0.93 | 0.85 / 0.98 / 0.91 |
| all 482 | 0.66 / 0.85 / 0.75 | 0.64 / 0.83 / 0.72 | 0.83 / 0.96 / 0.88 | 0.81 / 0.95 / 0.87 |
| vector index size | 196 MB | 99 MB | | |

- 128 dims halves the vector index and costs ~1–2 points of hybrid hit@5. English is unaffected; zh/ja lose a little more.
- Dropping mdwiki did not hurt: English hybrid hit@5 went 0.92 → 0.96, because duplicates no longer crowd the top 10.
- The d128 run used the component searcher, which scales nprobe with list count (27 vs 32 lists probed). The difference is small.

### Install choices: tiered vector components

Pack `packs/wikimed-tiered`: per source, `core` holds the most-viewed docs and fills 30% of the source's chunks. Pageviews are from Dec 2025–Mar 2026, by page id; `rest` is everything else. Each tier is built at 256 and 128 dims (12 components). Scenarios are pack views containing only the chosen components (`scripts/tier_eval.sh`). hit@1 / hit@5 / MRR over all 482 queries:

| installed vectors | size (en+zh+ja) | dense | hybrid |
|---|---|---|---|
| none (BM25 only) | 0 | — | 0.76 / 0.89 / 0.81 |
| core d128 | 31 MB | 0.25 / 0.32 / 0.28 | 0.74 / 0.89 / 0.81 |
| core d256 | 61 MB | 0.24 / 0.33 / 0.28 | 0.76 / 0.89 / 0.82 |
| core+rest d128 | 102 MB | 0.64 / 0.83 / 0.72 | 0.82 / 0.96 / 0.88 |
| core+rest d256 | 200 MB | 0.65 / 0.84 / 0.74 | 0.83 / 0.96 / 0.89 |

Two fixes were needed:
- **Coverage-aware fusion.** With core only, hybrid first fell to hit@1 0.41, below BM25 alone, because RRF gave every doc missing from the vectors a zero dense score. Now a doc with no installed vectors gets its BM25 rank counted for the dense ranker as well. Partial installs never do worse than BM25.
- **nprobe scales with √lists, not linearly.** Small components had too few probes; ja dense hit@1 was 0.63 split vs 0.69 single. With the change, the split index matches the single index.

**Caveat:** the synthetic questions are sampled uniformly over chunks, so ~70% of targets sit in `rest`. Core-only therefore shows no gain on this set. Real queries skew towards popular articles. A pageview-weighted query set is needed to measure what core-only buys.

### Vector index fidelity

Measured on the zh+ja pack with `sp vec-check`, against exact float search over the same 256-d vectors:
- recall@10 is 0.963 at nprobe 32 and rerank 400 (the default)
- quantisation alone, with every list probed, loses 0.012

Adding English (10× more vectors) lowered ja dense hit@1 slightly (0.70 → 0.66, before the table fix). The cause is cross-language distractors, not index recall.

### Embedding input (ja, decided)

| ja embedding input | dense hit@5 | hybrid hit@5 |
|---|---|---|
| every chunk, full text | 0.85 | 0.94 |
| first 1000 chars per article only | 0.69 | 0.89 |

**Decision: embed every chunk.**

## 4. Latency (GB10, server, warm)

| stage | p50 | p95 |
|---|---|---|
| query embedding, llama.cpp CPU, 16 threads, idle machine | 66 ms | 77 ms |
| query embedding while a GPU embedding job loads the CPU | 122 ms | 170 ms |
| BM25 (3 indexes, merge) | 5 ms | 16 ms |
| dense (IVF scan + int8 rerank) | 3 ms | 7 ms |
| hybrid search after embedding | 8 ms | 18 ms |

The query embedding dominates. Before reusing one llama context and building with native CPU flags and OpenMP, it took ~245 ms. Phone numbers come in Milestone 3.

## 5. Ingest and embedding throughput (GB10)

| step | rate |
|---|---|
| `zim_extract.py` (all four ZIMs, multiprocessing) | ~4 min total |
| embedding, torch bf16, GPU | ~19k tok/s, 86–100 chunks/s (wikimed-en 570k chunks in 1 h 51 min) |
| embedding, llama.cpp CPU (estimate) | ~500 tok/s, not worth adding next to the GPU |
| `sp build` of this pack | 5 min (BM25 4 min, k-means + vector index 1 min) |

Engine and token-cap experiments (torch vs llama.cpp HTTP server, 128-token cap): see §6.

## 6. Embedding engine experiments (ja, 134k chunks, 26.6M tokens)

Measured with `scripts/embed_experiments.sh`; quality is on the ja synthetic set (150 queries), ja-only packs.

| variant | time | chunks/s | tok/s | dense hit@1 / hit@5 | hybrid hit@1 / hit@5 |
|---|---|---|---|---|---|
| torch bf16, full chunks (current) | 24.0 min | 93 | 18.5k | 0.71 / 0.87 | 0.84 / 0.95 |
| torch bf16, first 128 tokens per chunk | 14.7 min | 152 | 18.3k | 0.67 / 0.84 | 0.80 / 0.93 |
| llama.cpp HTTP server (LAN endpoint), 4 parallel | 37.0 min | 60 | 12.0k | 0.71 / 0.85 | 0.84 / 0.94 |
| llama.cpp HTTP server, 8 parallel | 38.1 min | 59 | 11.6k | — | — |

- **torch on the GPU stays the engine.** llama.cpp serving the f16 GGUF on the same GB10 runs at ~65% of torch's speed. More parallel requests don't help, because that server is the bottleneck.
- **A 128-token cap is 1.6× faster but costs 3–4 points of hit@1.** Throughput in tokens/s is unchanged, so the time saved comes only from embedding fewer tokens. Not adopted. It remains a fallback if full Wikipedia embedding time becomes the blocker.
- Throughput on full Wikipedia still needs a plan (see §2). The candidates are batching tuned for the GB10, or a dedicated server such as vLLM or TEI, measured the same way.

## 7. Offline maps (2026-10-30)

Source: Protomaps daily build `20261030.pmtiles` (basemap v4.15.2, 139 GB planet), extracted remotely with `sp map-extract`.

| region | zooms | tiles (distinct) | size | extract time |
|---|---|---|---|---|
| World overview | 0–6 | 5,461 (2,895) | 45 MB | 6 s |
| United Kingdom (bbox −8.65,49.86,1.77,60.86) | 0–15 | 2,251,354 (833,908) | 2.99 GB | 4.5 min, 217 requests |
| Isle of Wight (test) | 0–15 | 1,990 (1,404) | 12 MB | 3 s |

- The UK rectangle also covers Ireland and slices of France and the Low Countries. A country-polygon cut will make it smaller.
- Web assets: MapLibre 1.7 MB, fonts 14 MB, sprites 0.3 MB.
- Rendering was checked in headless Chromium with software WebGL (`scripts/mapshot.js`): London at z14, and Japan with `lang=ja` labels from the world overview.

### Place search index (`sp map-places`)

| region | entries (places / POIs / streets) | keys | size | build |
|---|---|---|---|---|
| United Kingdom | 3.08M (121k / 1.52M / 1.43M) | 7.3M | 208 MB (fst 71, records 59, strings 78) | 12 s, 5.3 GB RAM |
| World overview | 6.9k (6.5k / 372 / 0) | 44k | 1 MB | 0.1 s |

The index is ~7% of the UK tile archive. Queries take 0.05–5 ms on the GB10 (common words like "high street", ranked by distance, are the slow end).

