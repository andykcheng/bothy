# Bothy (formerly Survivalpedia) — Design

Offline, searchable library of practical human knowledge for when the internet is gone.
Runs on a phone (Android app) and on a small Wi-Fi hub (any phone's browser). No network needed after setup.

Status: design agreed 2026-10-30. Numbers marked ~ are estimates to be validated by the WikiMed prototype.

---

## 1. Goals and constraints

- **Fully offline.** Everything (content, indexes, models) ships in one pack.
- **Search:** BM25 (keyword) + dense (Qwen3-Embedding-0.6B), fused with RRF. Optional local RAG answering.
- **Language (decided 2026-10-30, permanent): English only**, for content and search. There is no Chinese or Japanese content or search support: no zh/ja sources, BM25 indexes or tokenizers. Because the embedding model is multilingual, a non-English query may still find English articles through dense search, but nothing is built or tested for it.
- **Curated, not complete.** Content is chosen by usefulness in a crisis, under a byte budget. Default full pack: **50 GB**.
- **Text + images.** No video by default.
- **Reference phone:** Samsung Galaxy S23, 8 GB RAM (Snapdragon 8 Gen 2, Adreno 740, UFS, USB 3.2 Gen 1, **no microSD**). Pack lives on internal storage or a USB NVMe drive.

## 2. Delivery

| Channel | Who | How |
|---|---|---|
| Android app (Flutter + Rust core) | Individuals, offline on own phone | Signed APK. Register as a verified Android developer so it installs without the 24 h "advanced flow" wait once 2027 global enforcement lands. Install on household phones **now**; keep APK on the drive for later sideloading. |
| Server / Wi-Fi hub | Community, any device incl. iPhone; also the GB10 on the home LAN | `sp-server` (Rust, axum) serves a plain server-rendered HTML search page (no framework, no JS needed) plus a JSON API over HTTP. As a hub: mini PC/SBC + NVMe with an open Wi-Fi network and captive portal. Same Rust core + same pack. |
| Kiwix fallback | Anyone with Kiwix installed | Original ZIM files kept on the drive when space allows. |
| iOS app | Later | App Store only (needs Apple dev account). Until then, iPhones use the hub. |

## 3. Architecture

```
            ┌──────────────── build machine (GB10) ────────────────┐
 ZIM files →│ ingest → clean HTML → chunk → embed → classify topics │→ pack/
            │ compose (budget, topics, langs, image tier) → index    │
            └────────────────────────────────────────────────────────┘

 pack/ ──► sp-core (Rust)
            ├─ pack reader (docs, images)
            ├─ BM25: Tantivy + Lindera (ja/zh tokenizers)
            ├─ vectors: IVF + binary codes + int8 rerank
            ├─ llama.cpp: query embedding + optional LLM
            └─ RAG pipeline
          ▲                         ▲
   flutter_rust_bridge (FFI)    HTTP (axum)
          │                         │
   Flutter app                sp-server → HTML page + JSON API
   (mobile only)              (server / hub, headless)
```

- **sp-core** is the single implementation of reading, search and RAG, shared by the app and the hub.
- **Two products:** a **server version** (`sp-server`, headless: the build box has no display) and a **mobile version** (Flutter, Android first, FFI to sp-core). Flutter is mobile-only; there is no Flutter desktop or web build. (Decided 2026-10-30.)
- The Flutter app may later add an `HttpBackend` to use a nearby `sp-server` when the phone has no pack.
- Articles render in a WebView (`webview_flutter`) from a local scheme handler that serves pack content.

## 4. Sources

ZIM files from Kiwix are the **input**, not the shipped format. Stored under `/mnt/bigbelly/survivalpedia/sources/`.

**Only the en column is used** (§1, 2026-10-30). The zh/ja ZIMs were downloaded before that decision and aren't needed.

| Source | en | zh | ja | Notes |
|---|---|---|---|---|
| Wikipedia (all, maxi) | 119 G | 25 G | 29 G | Heavily filtered by topic/importance |
| Wikipedia medicine (maxi) | 1.7 G | 0.4 G | 0.3 G | Prototype set |
| mdwiki (WikiProjectMed) | 2.1 G | | | **Dropped (2026-10-30):** largely duplicates Wikipedia medicine en |
| wikiHow (maxi) | 48 G (2023-03) | 9 G (2024-09) | 7 G (2024-11) | **Removed from the live Kiwix catalog; only in archive.download.kiwix.org.** Download soon. CC BY-NC-SA. |
| iFixit | 3.3 G | 2.9 G | 3.2 G | Repair |
| Appropedia | 0.6 G | | | Appropriate technology |
| Energypedia | 0.8 G | | | Off-grid energy |
| Wikivoyage | ✓ | ✓ | ✓ | Geography, local info |
| World Factbook | 0.4 G | | | |
| Stack Exchange (selected sites) | TBD | | | gardening, diy, cooking, outdoors, electronics, ham, chemistry, medical sciences, … |
| Survival guides (PDF/HTML) | 95 docs | | | FM 21-76, FEMA Are You Ready?, WHO-ICRC Basic Emergency Care (CC BY-NC-SA), UK Be Ready (`ingest/guides_extract.py`) |
| Project Gutenberg books | ~54k English | | | **Ebooks, not library documents** (2026-11-01): EPUBs that people download one by one or by category and read in the app's reader. Their titles and authors are searchable, but not their full text. See "Books" below. |

**Books (2026-11-01).** Books are read, not searched: documents (guides, wikiHow, Wikipedia) are for search and browse, and books are for download and reading. `ingest/books/epub_shelf.py` selects every English text that passes the rights rule below, about 54k books. It fetches Gutenberg's own text-only EPUB (`pg<id>.epub`) and its cover from mirrors carrying `cache/epub/` (gutenberg.nabasny.com, gutenberg.pglaf.org, aleph.pglaf.org; not every mirror has every file), most-read first, about 14 GB in all. The illustrated EPUB3 is also fetched for the ~1,600 curated books (the classics and practical lists below). `index` writes `shelf.json(.gz)`: Gutenberg's 72 "Category:" shelves under 8 groups, plus per book its title, author, categories, popularity rank and sizes. The hub serves it with `--books <dir>` at `/api/books` and `/download/books/…`, and the catalog advertises it. Book search is keyword search over title, author, subjects and Gutenberg's description (`shelf-text.jsonl`, 98% of books have one), never the text: `/api/books/search` (BM25 with field weights title 3, author 2, subjects 1.5, description 1; the last word also as a prefix; popularity as a tie-breaker) and `/api/books/{id}` for the description shown in the app. The app's Books tab shows the books on the phone. Its shelf browser offers groups, then categories (download the most-read 50 or 200, or all), title/author search, and per-book download or the illustrated edition. The reader is `assets/reader/reader.html` with epub.js (BSD) and JSZip (MIT), served by a small Dart server on 127.0.0.1. It turns pages by swipe or tap zones, and has size, typeface, line spacing, margins, light/sepia/dark themes, contents, a progress slider, search in the book, and the place remembered. "Open in another app" hands the EPUB to any reader through `BookProvider.kt`. **Import (2026-11-01):** people can add their own EPUBs (any language) from a folder, searched with its subfolders, or from picked files. The Android picker (`BookImport.kt`) copies each book to `books/imported/<key>.epub`, along with a cover image that sits next to it in the folder. `lib/book_import.dart` then reads the title, author and cover from the EPUB's OPF and adds a `book:imp-<key>` component. Duplicates (same file, or same title and author) are skipped. Imported books never leave the phone and are not part of any catalog. The EPUBs keep Gutenberg's licence and are served unchanged. The selection below (classics, practical) first went into searchable chapter packs; those packs are retired, and the lists now pick which books get illustrated editions.
- Classics: English texts, no periodicals, one edition per title, issued before 2025 (fresh uploads have bot-inflated download counts), top 1,000 by downloads.
- Practical: candidates by LCC class (S, SB, SF, SH, SK, T*, QK, VM, GV, HS) and Gutenberg shelves (How To, Cooking, Gardening, Farming, Engineering), no fiction, no medicine (R*: 19th-century treatments are often harmful; WikiMed covers medicine). Jev scores each from its catalogue entry (question `books-practical` in `essentials/questions.py`, 5,488 books, ~4.3M tokens), and books scoring ≥ 1.9 are kept (605).

`gutenberg_fetch.py` downloads `<id>-h.htm` (+ images for practical books) from two fast mirrors (csclub.uwaterloo.ca, xmission; Gutenberg asks automated downloads not to use www.gutenberg.org; their rsync was ~50 KB/s). `books_extract.py` cuts the Gutenberg header, footer and licence at the `*** START/END OF` markers (their licence allows this for public-domain texts; the trademark is then not used) and splits each book on its highest repeated heading level into one document per chapter, plus a contents document. Chapters link to each other through `wiki:pg<id>/<n>` redirects, so the ordinary article page is the reader.

**Rights rule (the app ships worldwide):** Gutenberg texts are only guaranteed public domain in the USA. Books Gutenberg marks "Copyrighted" are dropped. A named author who died after 1955 drops the book (life+70 everywhere). Authors who died in or before 1955, or who have no death year but were born before 1851, are fine. Anonymous, corporate or undated authors (and books with no author) are kept unless the text carries a copyright notice dated after 1955. A first strict pass that also dropped every undated author lost about half of the practical list (296 of 605), mostly old government bulletins and anonymous manuals. Translators and illustrators aren't in the catalogue and aren't checked.

All content must keep attribution and license (CC BY-SA, CC BY-NC-SA) in `manifest.json` and on every article page.

## 5. Pack format

**Browse (2026-11-01).** `sp browse <pack>` writes `<pack>/browse.json` (sections from the first `--depth` categories, a record's `browse` field, or a `--sections` title→section TSV; `--pack-order` for guides). The hub's `/browse/{c}` lists a section's subsections with counts and its articles, with an A–Z bar and pages for long lists. Collections without the file are listed A–Z. Sections: wikiHow by category (3 levels), guides by guide in reading order, Wikipedia essentials by its 15 ranking topics.

**Collections (2026-11-01).** Content is a library of collections, one pack each: e.g. Wikipedia Essentials, wikiHow, Survival guides (books are separate, see §4 Books). Any subset can be installed, including none (someone who only wants games or maps). `sp build --id --title --description --kind` writes `manifest.collection`; the core (`sp_search::library::Library`) opens 0..N collections and searches them together:
- BM25 hits are merged by raw score and dense hits by cosine (same embedding model everywhere), then fused with RRF. Raw BM25 scores aren't fully comparable across indexes (a 95-document collection has its own IDF), so semantic indexes matter more once there are several collections.
- URLs carry the collection: `/doc/{collection}/{n}`, `/link/{collection}/{source}/{path}`. `/api/search?c=` limits a search to one collection.
- The hub catalog (v2) groups components by collection: `col:<id>` (articles + titles + BM25), `col:<id>:images`, `col:<id>:vec:<name>` (needs `model`), one shared `model`. Nothing is required. Files land under `collections/<id>/…` and `models/…` on the phone.

```
pack/
  manifest.json        version, sources, licenses, languages, budget, BLAKE3 checksums per file
  docs-NNN.bin         text blocks: ~64 KB, zstd with trained per-language dictionaries
  docs.idx             doc_id → (shard, block, offset, len); title, lang, source, topic, score
  titles.fst           title / redirect lookup (prefix autocomplete)
  img-core-NNN.bin     essential images (WebP), content-hash deduplicated
  img-ext-NNN.bin      optional images; can be omitted
  assets/              CSS (mobile, dark mode, large text), fonts, icons
  index/bm25-{en,zh,ja}/   Tantivy
  index/vec/{source}.{core|rest}.d{256|128}/   optional vector components (see below)
  models/              embedding GGUF, optional LLM GGUF
```

- **Vector components are optional and downloaded separately** (decided 2026-10-30). Each source has up to four:
  - `core.d256`, `rest.d256`, `core.d128`, `rest.d128`
  - `core` is the source's most-used articles by usefulness score (pageviews for now, the §6 score later); `rest` is everything else
  - `d128` is the Matryoshka truncation of the same embeddings, so one embedding run and one model serve both
  - with no vector component, the install is full text plus BM25 only and can skip the embedding model (~0.6 GB)
  - the searcher opens whatever is installed; where both lengths of a component are present it uses d256. It searches each component and merges by cosine (all come from the same model).
  - `manifest.json` lists the components with files, size and checksums, so the app can offer them as a menu.
- **Shards ≤ 4 GB** so packs work on FAT32 and can be copied and verified piecewise.
- **Text = simplified, cleaned-up HTML**, allowed elements only: `p h2-h4 ul ol li table tr td th figure img figcaption a b i sup sub blockquote pre code`, math as SVG. Remove navboxes, edit links, styles, scripts, link farms; shorten reference lists. Every `<p>`/section gets a stable `id`, so search hits deep-link and highlight.
- **Images:** WebP (decodes everywhere; AVIF rejected for old-device decode support). Tiers: `none`, `320` px, `640` px max width, quality ~60.
- **Image policy per source:**
  - wikiHow, medical, iFixit, Appropedia, Energypedia: all images (core).
  - Wikipedia: lead image + diagrams in core; the rest in ext.
  - Flags, icons, decorative images: dropped.
- **Video/audio:** excluded by default.

## 6. Content selection (compose)

Every document gets a **topic** and a **usefulness score**; `compose` fills a byte budget by usefulness per byte.

**Topic list (v0):**
- medicine & first aid
- water & sanitation
- agriculture & food preservation
- foraging & plant/fungi ID
- animals, livestock & veterinary
- shelter & construction
- energy & electricity
- tools, repair & manufacturing
- radio & communications
- chemistry & materials
- navigation & geography
- education & core science/maths
- governance/law/economics basics
- other

**Labelling:**
1. Use Wikipedia's own signals: WikiProject tags, Wikidata P31 types, categories.
2. An LLM on the GB10 labels a ~50k sample.
3. A linear classifier on the (already computed) embeddings labels everything.

**Score** = topic weight × importance (pageviews, inbound links, WikiProject importance) × practicality (how-to > essay). Biographies, sports, entertainment and political/military history get weight ≈ 0 by default.

**CLI sketch:**
```
sp compose --budget 50GB \
  --topics medical:max,water:max,agri:high,construction:high,other:low \
  --images core:640,ext:none --video none
```
The same tool produces small packs, e.g. a 5 GB medical-only pack. Indexes are rebuilt for each composed subset (fast: vectors are precomputed).

**50 GB budget (~):**

| Part | Size |
|---|---|
| English text (all survival sources + ~2M selected Wikipedia articles) | 12 GB |
| Images (core) | 20 GB |
| BM25 index (English) | 4 GB |
| Vector index | 4 GB |
| Models | 3 GB |

### Tiers and the essentials list (started 2026-11-01)

Most people can't download 50 GB, so packs come in nested tiers, each a superset of the one below, so upgrading is just an extra download:

| Tier | Target | Content |
|---|---|---|
| Essentials | ~1–2 GB | practical sources' core + ~70k Wikipedia articles, diagrams only, BM25 (vectors optional) |
| Standard | ~8–10 GB | ~1.3M Wikipedia articles, lead images and diagrams, core vectors |
| Full | ~50 GB | as above |

The ranked list is also a **dataset in its own right** (a "vital articles for survival"): one row per article with rank, page id, title, topic, score, size, tier and *why* it was chosen (`seed:<topic>`, `wp:<project>:<importance>`, `vital<N>`, `cat:<category>` or `links`). People can check and correct it by hand.

Method (`ingest/essentials/`, outputs in `build/essentials/vN/`):
1. `docs_table.py`: per article page id, title, categories, chunk count and image count, from the extract.
2. `sql_graph.py`: the article link graph from the SQL dumps (713M links, redirects resolved), plus talk-page assessment categories (Vital Articles levels, WikiProject importance).
3. `rank.py`: per-topic seeds are combined:
   - ~650 hand-picked hub articles (`seeds.txt`)
   - Top/High/Mid ratings from ~55 relevant WikiProjects
   - Vital Articles, as a small "general" topic
   - category rules (British/European flora and fungi, edible and poisonous plants, WHO essential medicines, livestock and crop diseases, knots and tools)

   Personalised PageRank from each topic's seeds runs over the link graph (all topics at once, cached). The score is relevance ÷ global PageRank^β (so countries and years don't win just by being central), × pageviews^0.5, × penalties (biographies, entertainment, sport, places, organisations, disambiguation pages), ÷ chunks^γ (usefulness per byte). Hand seeds always go in. Articles are ordered by score and tiers are cut by estimated bytes (chunks × 417 B for text + BM25, measured).
4. `wikihow_select.py`: wikiHow by its own category breadcrumbs (first aid, disaster preparedness, gardening, farming, repair, ...).
5. `gen_questions.py`: test questions. An LLM wrote 931 realistic questions in 13 topics × 3 styles (how-to, symptom in everyday words, what-is), each with the Wikipedia article that answers it, without seeing the seeds. Coverage is the share of questions whose article is in the tier. 35 named a disambiguation page and are skipped.

v0 results (β 0.3, γ 0.2, pageviews^0.5):
- Wikipedia essentials: 69,644 articles, 600 MB, covering **96%** of the questions (topics range from 88% to 100%).
- Standard: 1.3M articles, 6 GB, covering 99.9%.
- wikiHow core: 11k articles, 48 MB.

Caveat: some of these question misses were used to tune the category rules. The ~170 questions written afterwards score about the same.

**v1 (2026-11-01): Jev re-rank** (`essentials/rerank.py`, output `~/sp-scratch/essentials/essentials-v1-wikipedia-en.tsv`). TypeSafe's Jev (question v5, region-neutral, common medicines count as useful) scored all 400k v0 candidates from their leads (235M input tokens in total, mostly packed 5 articles per request). The selection fills 600 MB in three stages:
1. v0 ranks ≤ 2,000 (hand seeds) unless Jev < 0.15. Jev underrates a few survival topics: Whetstone 0.25, Snare 0.22, Orienteering 0.22.
2. Every article with Jev ≥ 1.0 (49.6k), by Jev + 0.6 × rank prior.
3. The rest in v0 order among Jev ≥ 0.4. Below 1.0 Jev barely separates: a hand check of 100 found obscure chemicals and species just inside a pure-Jev cut and Pacific Ocean, Hinduism and major languages just outside, and v0 prefers broad, much-read topics.

Result: 128,716 articles, **98.0%** question coverage (v0: 96.4%); 41k articles added and 36k dropped compared with v0. Of 40 random articles in the selection, about 12 are clearly useful, 6 background and 22 filler (from stage 3). The filler is cheap, but the Essentials *budget* could shrink instead.

**v2 (2026-11-03): classifier over all of Wikipedia, exclusions, 450 MB.** Output `~/sp-scratch/essentials/essentials-v2-wikipedia-en.tsv`, built by `build-essentials-v2.sh`.
1. **Classifier.**
   - Each article's lead-chunk embedding (`shard_leads.py`) feeds a kNN + ridge model (`classify_all.py`) that predicts Jev's score for all 7.2M articles.
   - It was trained on 532k Jev labels: v1's 400k, plus 145k bought for it. The new labels covered every unscored article predicted ≥ 0.8, plus a random 20k from the tail.
   - In the random tail (predicted < 0.4, about 5M articles), 1 in 10,000 was useful. That tail is therefore not scored.
2. **Exclusions** (`exclude.py`):
   - Removed by exact category patterns: people, films, music, TV, video games, sport, battles and elections, 2.96M articles in all.
   - Never removed: hand seeds, question answers, a must-keep list, and anything Jev scored ≥ 1.3.
   - Of the removed articles Jev had scored, those rated useful were all people (inventors and practitioners); their methods have their own articles.
   - **Species rule** (`species.py`): species pages need ≥ 1.5. Jev rates any species with a recorded use as useful, and before this rule rare species filled half the budget. The common edible, medicinal and poisonous species all score ≥ 1.5.
3. **Clef** (local) scored the retrained classifier's new candidates (14.6k). Its scores were mapped to Jev's scale by isotonic regression on the 614-article audit set: Clef 0.9 ≈ Jev 0.88, Clef 1.0 ≈ Jev 1.05.
4. **Rerank** (`rerank.py --prior 0.6 --extra … --exclude …`) at a 450 MB budget.

Result: 105,101 articles, 98.4% question coverage (v1: 98.0% at 600 MB), and 62,978 articles with Jev ≥ 1.0 (v1: 49,608). 16k of the articles come from beyond v0's 400k candidates.

Coverage by budget:

| Budget | Coverage |
|---|---|
| 250 MB | 93.7% |
| 350 MB | 96.8% |
| 450 MB | 98.4% |

Browse topics for the new articles come from their 15 nearest labelled leads (`browse_topics.py`). The pack is 1.8 GB with lead images and both vector lengths. v1 is kept in `~/sp-scratch/library-old/`.

**v2d (2026-11-04): no byte cap, less waste.** Built by `~/sp-scratch/essentials/essentials-v2d.sh`.
- **Selection by score, not size:** useful articles must stay in. Every article scored ≥ 0.8 (Jev scale), plus 0.4–0.8 articles that are widely read and linked (v0 rank ≤ 100k): `rerank.py --floor 0.8 --fill-min 0.4 --fill-rank 100000`, no `--budget`. Taking whole bands below 0.8 would add tens of thousands of obscure articles (Bukkumi, 6-Fluorotryptamine) for the odd useful one.
- **Clef pass B** scored the 36,950 non-species, non-excluded articles the classifier predicts 0.6–0.8; 1,986 calibrated ≥ 1.0.
- **Waste removed** (`ingest/wp_scrub.py`, applied by `subset.py` and `zim_extract.py`): editor notes ("Please help improve Wikipedia… alt parameter", `[citation needed]` in all its forms, "you can help by adding…"), and the periodic-table navigation box in element infoboxes, which made them several screens wide on a phone. Animated images over 150 KB become their first frame (`subset.py --max-anim`): 639 animations were a quarter of the image bytes (234 MB → ~3 MB).
- **Links to articles no installed collection has** are shown as plain text when the page is rendered (`page.rs`), not stripped from the pack, so they come back when a bigger pack is installed.

Result: 131,113 articles (131,136 docs), 531 MB of text + BM25, 98.7% question coverage, 64,964 articles with Jev ≥ 1.0. Pack 1.9 GB: docs 395 MB, BM25 187 MB, images 827 MB, vectors 341 + 173 MB. Known miss: Svalbard Global Seed Vault (0.42, rank 153k). v2 (450 MB) is kept as `essentials-v2a-wikipedia-en.tsv`.

**Precision was the weak point (v0).** Checked by reading random samples, roughly 60% of the top 6k articles are useful, ~40% of ranks 6–20k, and 20–30% beyond. 70% of the essentials tier is there only through links (`why` = `links`), and those are most of the noise: obscure compounds, storm seasons, court cases. No setting of β, γ or the pageview weight fixed it. A content signal is needed next: the topic classifier on embeddings of each article's lead (§6 Labelling, step 3). Until then, the essentials budget may be better spent on the practical sources (wikiHow beyond its core, Appropedia, Energypedia, iFixit, Stack Exchange) than on the tail of the Wikipedia list.

## 7. Search

- **Chunking:** ~256 tokens on paragraph/section boundaries; chunk → (doc_id, para_id).
- **BM25:** Tantivy, English with stemming. (The prototype also had zh/ja indexes, built with Lindera using UniDic and CC-CEDICT. They were removed with the English-only decision, together with OpenCC. That took the app's native library from 234 MB to ~10 MB. The searcher ignores `bm25-zh/ja` in old packs.)
- **Dense:** Qwen3-Embedding-0.6B, Matryoshka-truncated to 256 dims (also shipped as 128 dims, see §5). Measured cost: ~293 B per chunk at 256 dims, about the same as the compressed text (MEASUREMENTS §2).
  - Stored as a binary code (for the IVF scan) plus int8 (for reranking).
  - IVF with ~64k centroids; probe a few dozen lists → each query reads a few MB. HNSW was rejected because its random access is slow on USB/UFS.
  - The query instruction prefix and embedding dimension must match the build exactly; both are recorded in `manifest.json`.
- **Fusion:** RRF over BM25 + dense, then an optional rerank of the top 50.
- **Non-English queries** aren't supported (§1). BM25 can't match them, so whatever comes back comes from dense search alone.

## 8. RAG (optional, high-end phones and the hub)

- **Runtime:** llama.cpp (already in sp-core for embeddings), CPU. Crate `sp-rag`; the server's `/ask` page and `/api/ask` (NDJSON stream) serve both the hub and the app, which runs the same server.
- **Model (2026-10-31): LFM2-1.2B-RAG, Q4_0 (696 MB, LFM Open License v1.0).** Compared on 8 medical/first-aid questions over wikimed, 4 passages each (GB10 CPU, 8 threads, while the GPU embedding job was running):
  - LFM2.5-2.6B (QAD Q4_0, 1.6 GB): a reasoning model. With its thinking switched off (empty `<think></think>`), it refuses most answerable questions. With thinking it needs ~90 s and runs out of budget. Not usable on phones.
  - LFM2.5-1.2B-Instruct: summarises each passage in turn instead of answering.
  - **LFM2-1.2B-RAG:** direct answers, doses quoted exactly, 9–14 s per answer. Weak points: it rarely writes [n] citations (sources are always listed under the answer anyway), and it answers off-topic questions from irrelevant passages instead of refusing.
  - The hub serves one model as the catalog component `llm` (`--llm DIR`). `sp-rag` handles any GGUF chat model: reasoning models get an empty think block, tool-call tokens are banned, and `think=true` is an experimental switch.
- **Follow-up chat:** the `/ask` page is a conversation, kept in the browser's sessionStorage (cleared by "New topic"). `POST /api/ask {q, history:[{q,a}]}` sends up to the last 4 turns. The model sees them as earlier chat turns: answers have their [n] citations stripped and are cut to 700 chars, and the oldest turns are dropped if the prompt won't fit in 4096 tokens. Retrieval: a follow-up is searched together with the last two questions, which fills all passage slots but one. The follow-up's own search takes the last slot, in case the topic changed.
- **Android build:** `GGML_CPU_ARM_ARCH=armv8.2-a+dotprod+i8mm+fp16` (int8 kernels). The app therefore needs Cortex-A710/X2 or newer (Snapdragon 8 Gen 1+, 2022 on).
- **S23 budget:**
  - App RAM ~3.5–4 GB in practice. LLM Q4 ~0.7 GB + KV cache, embedding model ~0.4–0.65 GB. The model is loaded on the first question, not at start.
  - Target total time to answer < 20 s. Since 2026-11-04 the context is ~1.7k tokens (8 passages of ≤1300 chars) in an 8k window, so prompt processing roughly doubles on a phone.
- **Pipeline (fixed, not a free-running agent):**
  1. Hybrid search, English only → top 8 articles, best chunk each (4 until 2026-11-04: often too little to go on).
  2. Answer **only from those passages**, citing them as [n] links to paragraph anchors. Up to 600 answer tokens (was 300), and the instructions ask for a full answer of 8–12 sentences or numbered steps (was "at most 6 sentences"). LFM2-1.2B-RAG still stops on its own at ~100–200 tokens (70–150 words on hypothermia, safe water, paracetamol dose and bleeding), so the cap is no longer what limits length; longer answers would need a bigger model.
  3. Later: optional query rewrite; up to 2 more rounds of tool calls (`search`, `open_section`) with output constrained by a GBNF grammar; a relevance threshold so off-topic questions get "not covered".
- **Safety rules:**
  - Sources are always shown next to the answer.
  - The model says "not found in library" rather than guess.
  - Dosages, ratios and electrical values are shown as quoted source text.
  - Search works fully without the LLM.
- **Hub:** Pi 5 is search-only. An LLM-capable hub (N100/Ryzen mini PC or Jetson Orin, 15–25 W) runs a 4–8B model for the community.

## 9. Repo layout

```
docs/       DESIGN.md; README-level guides: CONCEPTS, ARCHITECTURE, DATA-FORMATS (byte-level spec), MAKE-A-COLLECTION, HTTP-API
core/       Rust workspace: sp-pack, sp-search, sp-embed, sp-rag, sp-cli
ingest/     Python: ZIM → clean HTML + chunks (python-libzim), topic labelling, GPU embedding
app/        Flutter app; app/assets/games/ holds the games (§17)
hub/        hub server config, captive portal, HTML page, image build
```

Data (not in git): `/mnt/bigbelly/survivalpedia/{sources,build,packs}`. BigBelly is a USB **HDD**, so random-access build steps run on internal NVMe scratch and finished outputs are copied back.

## 10. Milestones

1. **Prototype (WikiMed en + Wikipedia medicine ja/zh):** ingest → pack → BM25 + dense → CLI search. Validate sizes and quality.
2. **Server version:** `sp-server` (axum) over sp-core: HTML search/article pages, JSON API, images, attribution. (Started 2026-10-30.)
3. Flutter mobile app over FFI, Android build on S23 (measure query latency and RAM).
4. Topic labelling + `compose` on full English Wikipedia + wikiHow (+ the other English sources); produce the 50 GB pack.
5. RAG on S23: model benchmark, pipeline, safety UI.
6. Hub packaging: `sp-server` on a mini PC/SBC, captive portal, power notes.
7. Verified developer registration, signed release APK, copy procedure for community drives.
8. **Maps (§16):** offline OSM vector tiles plus place search, first in `sp-server`, then mobile. Independent of the text work, so it can start any time after 2.

## 11. Source acquisition (2026-10-30)

- **Content: Kiwix ZIM** (`scripts/fetch_sources.sh`, lists in `scripts/sources_*.txt`), downloaded with aria2c, sha256-verified, `.ok` marker per file.
- **Raw Wikipedia HTML:** rejected. The public Enterprise HTML dumps stopped at 2025-03-20. The XML dumps are wikitext, and rendering them needs MediaWiki/Parsoid plus templates.
- **Metadata from raw dumps** (`scripts/fetch_wikimedia_meta.sh`, used for topic labelling and scoring):
  - en/zh/ja `page`, `page_props` (Wikidata QID), `redirect`, `categorylinks`, `linktarget`, `pagelinks` (inbound-link counts)
  - 12 months of `pageview_complete` user pageviews, used for popularity ranking *within* a topic
- dumps.wikimedia.org throttles to ~1.6 MB/s per connection, so metadata (~90 GB) takes most of a day.

## 12. Update pipeline (every 6 months: January, July)

1. **Fetch:** `scripts/fetch_*.sh` from a timer on the build machine. Old sources are kept; wikiHow is frozen (archive only).
2. **Ingest + diff:** content hash per doc and chunk; unchanged items are skipped.
3. **Embed changed chunks only:** cache keyed by (model id, dims, prefix, chunk hash). The embedding model is **pinned**; changing it is a major version with a full re-embed.
4. **Re-score + compose** using fresh pageviews and links.
5. **Check before release:**
   - golden queries in en/zh/ja must return their expected docs
   - size budget, checksums
   - test install on the reference phone
6. **Publish** the new version; keep the previous one until the new one passes checks.

Phones update by re-copying the whole pack (~5–10 min over USB 3); no delta updates.

## 13. Licensing

| Content | License | Obligation |
|---|---|---|
| Wikipedia, Wikibooks, Wikivoyage, Wikiversity, Wiktionary, Appropedia | CC BY-SA 3.0/4.0 | Attribution; modified text stays CC BY-SA |
| Stack Exchange | CC BY-SA 2.5/3.0/4.0 | Plus author name and link per post |
| wikiHow, iFixit | CC BY-NC-SA | Non-commercial distribution only |
| Commons images | per image | Store credit per image |
| enwiki non-free (fair-use) images | — | **Excluded** (identified via categorylinks) |
| World Factbook | public domain | — |
| OpenStreetMap map data (§16) | ODbL 1.0 | "© OpenStreetMap contributors" shown on the map; derived databases stay ODbL |

- Every doc stores source, original URL, revision date and license, and renders an attribution footer. `manifest.json` lists all sources and licenses.
- **Stack Exchange:** each doc has an `authors` list (name, profile URL, role, post date) for the asker and every kept answer, taken from the post signatures. The footer reads "Written by X (question); Y and Z (answers)".
  - The names are kept out of the text, so passages and vectors don't change when attribution does.
  - The licence follows the post dates: CC BY-SA 2.5 before 2011-04-08, 3.0 before 2018-05-02, 4.0 after. A thread mixing periods says, for example, "CC BY-SA 3.0 / 4.0".
- `compose --license nc-ok|commercial-ok`. NC sources ship as separate packs. The app is free forever (no charges, no ads), so NC sources are fine for it.
- Project Gutenberg: public-domain EPUBs served unchanged, with the Gutenberg licence inside (§4 Books for the worldwide rights rule).
- Index and embedding files are released under CC BY-SA. Format spec: CC0. Reference reader: MIT/Apache-2.0. App code: separate license (TBD).
- Donations are voluntary only: never a download gate, perk or paywall, and spending is reported.

## 14. Publishing

- **Open data:**
  - packs plus `docs/DATA-FORMATS.md` (versioned byte-level spec, CC0)
  - test vectors (later)
  - reference readers, so anyone can write their own: Rust (`core/crates/sp-pack`) and Python (`tools/sp_pack.py`, 2026-11-02)
  - people can build their own collections from Markdown/HTML/text (`ingest/docs_extract.py`, `docs/MAKE-A-COLLECTION.md`)
- **Name and domain (2026-11-05):** the app is **Bothy**; the store name is "Pocket Bothy: Offline Knowledge & First Aid".
  - **Why the change:** "Survivalpedia" is one letter from Survivopedia, a known prepper blog, and undersold an app that also covers repair, cooking, books, maps and chat. A bothy is a free shelter that's never locked.
  - **Domain:** `pocketbothy.com`, on its own Cloudflare account. The website explains the app and offers the Android APK (besides the Play Store) and the hub server for people who run a hub.
  - **Downloads:** `data.pocketbothy.com`, the custom domain of R2 bucket `bothy-data`, connected and serving.
    - **Layout (2026-11-05):** `/v1` is a static copy of a hub's download API: `/v1/api/catalog` (the catalog JSON, DATA-FORMATS §4) and `/v1/download/<path>`. So the app and `bothy-hub` can treat `https://data.pocketbothy.com/v1` exactly like a hub.
    - **Publishing:** `sp-server --library … --maps … --games … --llm … --export-catalog DIR` writes `DIR/api/catalog` plus `DIR/download/…` as symlinks to the real files. Upload DIR as it is, with the files first and `api/catalog` last (content-type application/json). Store `.html` files as `application/octet-stream`: Cloudflare's Email Obfuscation rewrites `text/html` that contains an address (it broke a puzzle game's hash). Uploaded 2026-11-05. WikiMed stays out (its old pack has zh/ja vectors; Essentials covers medicine). The current export is in `~/sp-scratch/publish/v1`: 79 components, 1,776 files, 13.2 GB.
- **Hub for everyone (2026-11-05):** `bothy-hub` (`core/crates/sp-server/src/bin/bothy-hub.rs`) is the hub for people who aren't developers.
  - **Commands:**
    - `get` downloads from the download server into a data folder, in the app's layout, with `installed.json`. It resumes interrupted files (Range), checks every file's BLAKE3, checks free space first, and removes components that are no longer chosen.
    - `check` confirms the download is complete (`--full` re-hashes) and lists what the hub offers.
    - `start` serves it with chat, HTTPS on 8443 and downloads for phones. With nothing downloaded, it runs `get` first.
  - **Presets:**
    - small (2.6 GB): all text, 256-dim vectors, the model, games and the world map;
    - standard (4.4 GB): adds images under 1 GB and the AI model;
    - everything (10 GB): adds the rest, including the 128-dim indexes for phones copying from the hub.
    - No preset has regional maps (2026-11-05): they're chosen one by one, and a preset run keeps the maps already chosen (Docker passes `BOTHY_PRESET` on every run).
    - `--add` and `--skip` take ids or id prefixes, and maps by id, country or name (`map:us` = every US state, `map:california`). Each option also has a `BOTHY_*` environment variable.
    - `maps [SEARCH]` lists regions by continent (✓ = installed); `maps add|remove NAME…` runs `get` on the current choice plus or minus those maps.
  - **Docker is the main way to install it.** `hub/Dockerfile` builds a 121 MB image that holds only the program; content goes into the `/data` volume on first start.
    - CPU baselines: armv8-a (Raspberry Pi) and x86-64-v2.
    - `.github/workflows/hub-image.yml` builds amd64 and arm64 on native runners and joins them into one Docker Hub tag. It needs the secrets `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN`.
    - **Image name: `andykcheng/bothy`** (2026-11-05). It's free on a personal account, and Docker Hub organisations now need a paid plan. Docker Hub can't rename an image, so this name stays.
    - The setup guide for Windows, Mac, Linux, Raspberry Pi and NAS is `site/hub.html` (www.pocketbothy.com/hub).
    - Native downloads for Windows and Mac can come later.
  - **What kept the old name:** only what people see changed. Wire formats (`survivalpedia-pack`, `survivalpedia-catalog`, the `survivalpedia` key in map metadata), crate names, method channels and the Android application ID `org.survivalpedia.app` are unchanged.
    - The application ID can still change before the first Play Store release. After that, it never can.
- **Primary host: Cloudflare R2** behind a custom domain (domain DNS on Cloudflare).
  - no egress fees, no rate limit on a custom domain
  - files over 512 MB are not edge-cached on the free plan; that's fine
  - add one rate-limit rule to cap request abuse
- **Mirrors:** BitTorrent with an R2 web seed (BEP 19), Internet Archive (long-term, free), Hugging Face Datasets (embeddings/chunks for ML users).
- **`releases.json`:** lists versions, mirrors and BLAKE3/sha256 checksums.

## 15. Budget

| Item | Cost |
|---|---|
| Google Play developer account (developer verification + Play Store) | $25 once. New personal accounts need a 12-tester, 14-day closed test before going public. |
| Cloudflare R2 (current + previous release, ~0.5–1 TB) | ~$8–15/month; requests stay within the free tier |
| Domain | already owned |
| Apple developer account | $99/year, deferred; iPhones use the hub |

## 16. Offline maps (optional component; prototype running 2026-10-30)

Goal: a map in your pocket with no network, as an optional download next to the text packs.

**Data and format**
- OpenStreetMap vector tiles in **PMTiles**: one file, a tile directory plus MVT tiles, readable by byte range.
  - Basemap schema from Protomaps (or OpenMapTiles). Build input: the Protomaps planet build, or our own run of planetiler on an OSM planet extract on the GB10.
  - Planet to z15 is on the order of 100+ GB. Exact sizes are to be measured.
- **Pieces people choose from:**
  - `world-low`: whole planet to ~z8 (countries, cities, main roads). Small; a good default.
  - `region-<name>`: country/state extracts at full zoom (`pmtiles extract --bbox`/region polygon). This is where the size goes. Dense countries are a few GB; small ones are tens to hundreds of MB.
  - optional `hillshade`/terrain raster per region, later. It is large, so off by default.
- **Assets shipped with the maps:** style JSON (light, dark, and a high-contrast "outdoor" style that shows water, tracks and huts), glyph PBFs for Latin + CJK, and a sprite sheet. CJK glyph ranges are large, so ship only what the styles use.

**Rendering**
- **Hub (`sp-server`):**
  - serves `/tiles/{z}/{x}/{y}.mvt` straight from the PMTiles file (Rust `pmtiles` reader, range reads, no unpacking)
  - serves MapLibre GL JS, the styles and the glyphs
  - map page at `/map`; any phone browser on the hub's Wi-Fi works
- **Mobile (Flutter):** MapLibre Native (`maplibre_gl`). Tiles come either from a PMTiles source or from a local tile handler in the Rust core. The handler avoids depending on the plugin's PMTiles support and reuses the server code.

**Search and links to the library**
- **Place search:** an offline gazetteer built from OSM `place=*` and named POIs. Names in all languages (`name`, `name:en`, `name:zh`, `name:ja`) go into a small FST + BM25 index per region; results zoom the map.
- **Survival POI layer:** drinking water, springs, hospitals, clinics, pharmacies, shelters, police, fuel, toilets, campsites. Extracted from OSM tags into its own layer, searchable by "nearest".
- **Map ↔ articles:** Wikipedia articles with coordinates (from the dumps' `page_props`/Wikidata) get map pins, and articles show a "view on map" link.

**Not in scope (for now):** turn-by-turn routing. It needs a routing graph (Valhalla/GraphHopper tiles) of a similar size to the map, so it could be a later optional component. Street-level imagery is far too large to ship.

**Updates:** rebuilt with the 6-monthly pipeline (§12). Regions are versioned independently.

**Built (2026-10-30):**
- **No planet download.** `sp map-extract` (crate `sp-maps`) cuts a region straight out of the remote daily Protomaps build with HTTP range requests. It fetches only the leaf directories and tile bytes inside the region, and writes a new PMTiles file with duplicate tiles stored once.
- **Regions (2026-11-05): countries, cut along borders.** About 500 regions, 148 GB for the world (build 20261105).
  - **Shapes:** Natural Earth 10m admin-0 (public domain). Countries over ~4 GB are split into admin-1 parts: US, Canada, Russia, China, Brazil and Germany by state; France by region (18). Largest part: Nunavut 4.7 GB (huge, mostly empty); the UK is 2.6 GB.
  - **Tile choice** (`sp-maps::region`): per zoom and tile row, a tile is kept if a border edge passes through it or the row's centre line is inside (even-odd); then one tile of margin all round, so the coarse borders never clip a coast or border town. `sp map-extract --polygon FILE.geojson`; `--estimate` sizes it without fetching tiles; `--dir-cache` keeps the build's 353 MB of leaf directories locally so ~500 regions don't refetch them.
  - **Ids:** ISO 3166-1 alpha-2 (`fr`), `uk` for the United Kingdom, ADM0_A3 where there's no code; parts are `<country>-<code>` (`us-ca`, `fr-ara`). Region metadata (`survivalpedia` in the PMTiles metadata) carries `continent`, and for parts `country` and `country_title`; the catalog passes them on with `name`.
  - **Pipeline:** `scripts/map_regions.py` (polygons + `maps/regions.json`, with size estimates), then `scripts/map_batch.py [ID…]` (extract + place index per region, resumable, 4 at a time; ~3 h for the world). Output: `~/sp-scratch/maps/regions/<id>.pmtiles` + `<id>.places/`.
  - **Choosing:** the app's Content tab has a picker (search, or continent → country → parts); hubs use `bothy-hub maps`. The catalog remembers map hashes in `<maps>/.hashes.json` (by size + mtime), so a hub with many maps starts quickly.
- **Serving:** `sp-server --maps DIR` serves every `*.pmtiles` in DIR as one tile layer. Each tile comes from the smallest region that has it, so the world overview fills in outside detailed regions. Regions whose bounds (plus a tile) miss the tile are skipped. Neighbouring regions overlap by their margin; overlapping tiles are identical (whole tiles from the same build), so either serves.
- **Web assets:** pinned and checksum-verified by `scripts/fetch_map_assets.sh`:
  - maplibre-gl 6.16.0 (ES modules)
  - @protomaps/basemaps 5.7.2 (style layers)
  - protomaps/basemaps-assets (Noto glyphs, sprites)
  - CJK labels use the device's own fonts (`localIdeographFontFamily`), so no CJK glyph files are needed
- **Map page:** `/map`. Light or dark follows the device; label language follows the browser, or `?lang=`.
- **Place search (built 2026-10-30):** `sp map-places <region.pmtiles>` builds `<region>.places/` straight from the tiles, with no extra download. `/api/places` serves it, and the map page has a search box (suggestions as you type, ↑/↓/Enter, `/map?q=`).
  - **What's indexed:**
    - places at every zoom, plus POIs and named streets at max zoom
    - names: default + en / zh-Hans / zh-Hant / ja
    - each street or POI gets the nearest settlement as context
  - **Lookup:**
    - an FST of normalised name suffixes from every word start (every character for CJK), so "museum" and 博物館 both find 大英博物館
    - ranking uses a cheap importance + distance score over up to 500k candidates, then full name scoring for the top 1,000
    - importance comes from place type and population, and POI prominence (`min_zoom`, wikidata); bus stops and car parks named after streets rank lower
- **Not yet:**
  - "St" ↔ "Saint" style abbreviations
  - category search ("pharmacy near me": the POI layer step)
  - country context for world-overview places, which needs the country polygons


## 17. Games (optional component; built 2026-10-30)

Small offline games for long stretches with no power or signal.
- **Format:** one directory per game, `index.html` + `game.json` (id, title, description, players, version, license, author). Games are self-contained HTML/JS, tens of KB each, no network. Saves and scores go in localStorage. Games pause on `visibilitychange` and on the app's `sp-pause` event (sent when the app goes to the background).
- **Sources live in `app/assets/games/`.** Ours are MIT OR Apache-2.0. Third-party open-source games are welcome under the same layout, with their licence and author in `game.json` and the licence text in the directory (DESIGN §13).
- **Hub:** `sp-server --games DIR` serves `/games` (list) and `/game/{id}/`. With `--downloads`, each game is a catalog component `game:<id>` (group `games`), downloaded from `/download/games/…`.
- **App:** a Games tab lists `<content>/games/*`. A small Dart HTTP server on 127.0.0.1:47801 serves them to a full-screen WebView. The port is fixed because localStorage is per origin. Games work without the library or a hub. All of our own games (~300 KB together) are bundled in the APK and copied out on start (when missing or older). The WebView shows JS `alert`/`confirm`/`prompt` as Flutter dialogs; Android's WebView otherwise drops them silently (`confirm()` returns false). The WebView background is dark, so after each page load the app gives any page whose `html` and `body` are both transparent a white background and dark text (Tatham's puzzles and their help pages were black-on-near-black).
- **Low power:** turn-based, DOM/SVG only, no animation loop while idle (Leap and Snake are the exceptions, and pause in the background).
- **Our set (2026-10-31):**
  - Solo: Snake; Sudoku (generated, unique solution, 4 levels); Leap (one-button runner: 10 seeded levels, each checked clearable by search, plus endless); Solitaire (Klondike, draw 1/3, tap-to-move, hints); FreeCell (auto-move home when safe); Sea Battle (vs computer, hunt/target AI).
  - **Greenhollow** (original casual farming RPG): crops by season (10-day seasons), daily watering for a double harvest, giant crops; monsters raid the fields at night; the Deep Woods (glades with forage, chests, springs, traps, monsters, a boss every 5th glade up to the Old Oak Wyrm at 20); levels with skill perks; gear; cooking with buffs; hens, cows and bees; scarecrows and sprinklers; village quests; four festivals; wandering merchant and other daily events; village restoration projects as long-term gold sinks; achievements. Balanced with a scripted player: year 1 ends near level 8 and glade 10.
  - **Emberdeep** (original fantasy dungeon RPG, D&D-style but no borrowed setting or characters): knight, rogue or wizard, each with two skills (the second at level 5). Six chapters of five floors (Old Mines, Drowned Crypts, Gilded Vaults, Temple of Embers, Wyrmhollow cave, the Sleeper's Heart), each with its own map style, palette, monsters and a guardian with a telegraphed or summoning attack. Then an endless dark below floor 30. Story: chapter intros, 30 lore pages (one per floor) and five prisoners who come back to town and open shops. Encounters include shrines, fountains, riddles, a peddler, wounded adventurers and cursed idols. One moral choice (spare or slay the dragon) changes the final fight and the epilogue. Town meta-progression: permanent upgrades, packed potions and scrolls, bounties, a hall of heroes, a journal with bestiary. Waystones let later runs start at a chapter whose guardian you have beaten. Harder replays (Ember-touched) unlock after you finish. Monster stats scale with an expected-hero curve per floor. Art: Kenney's Tiny Dungeon (CC0), recoloured on canvas for variants.
  - **Tinkerworks** (original physics contraption puzzles, in the genre of The Incredible Machine but with its own parts, stages and look): 22 stages. Place a limited set of parts (planks, bouncers, fans, conveyors, iron balls), press Start, and get the red target to the golden zone. Stage objects include dominoes, a seesaw, balloons, pegs, buttons that open trapdoors, and no-build zones. Physics: matter.js 0.20 (MIT, vendored). `sim.js` is shared by the page and `scripts/tinker_check.js`, the checker. The checker proves each stage fails with no parts and has a solution that still wins when every part is nudged ±4 px; the solution is stored as `sol` in `levels.js`, and the 💡 hint shows it one part at a time. Low power: the simulation runs (requestAnimationFrame) only between Start and a win or the 15 s limit; editing redraws only on input.
  - Family / 2-player (pass the phone, or computer opponents with 3 levels): Four in a Row (negamax), Draughts (English rules, alpha-beta), Reversi, Mancala (Kalah), Chess (full rules; perft-checked; alpha-beta + quiescence), Nine Men's Morris, Dots and Boxes (2–4 seats, any can be the computer), Yacht Dice (1–6 players), Estates (property trading, 2–4).
  - Party / word: Party Cards (act / draw / describe against the clock, 2–4 teams, kids or everyone cards); Balloon Words (hangman-style with balloons: category word lists, or a friend types a secret word).
- **Third-party** (built by `scripts/third_party_games.sh OUT`, kept out of git, served from the hub only):
  - **Puzzles:** Simon Tatham's Portable Puzzle Collection, 40 puzzles, the upstream WebAssembly build + help pages + an index page, with a white page background and a mobile viewport injected; MIT; source tarball included (18 MB).
  - **DopeWars**: DopeWars.js by Nicolas Hurtubise, a web version of the old Drug Wars trading game; MIT; static files plus one patch (a cancelled name prompt used to loop for ever). Adult theme, noted in its description.
  - **Farmhand** by Jeremy Kahn: GPL-2.0-or-later, so the download includes the complete source tarball and LICENSE; built unmodified except leaving out source maps, the Electron entry and the service worker (3.8 MB). Its online features (multiplayer, shared market) don't work offline.
- **Rules:** original designs only. Genres are fine, copies of specific commercial games aren't.

## 18. Notes (built 2026-11-02)

During a blackout people need somewhere to keep things: numbers, where the water is, what is in the grab bag. The Notes tab works with nothing downloaded, no core and no network.
- **Kinds:** a plain note, or a checklist (ticked items move under "Ticked"; "Untick everything" reuses a list). Either can be turned into the other, one line per item. Notes can be pinned to the top. Search appears once there are 6 or more notes. Delete has Undo.
- **Ready-made notes:** Emergency contacts (blanks to fill in, plus 112), a Power cut checklist, and a Grab bag list. They are offered when there are no notes, and always from New. They are worded for anywhere in the world, with no country-specific numbers.
- **Phone numbers:** any run of 7 or more digits, plus 112/911/999/000, shows as a chip above the note. Tapping a chip offers Call (Android `ACTION_DIAL` via the `survivalpedia/files` channel; it needs no permission, and the user presses call) or Copy.
- **Storage:** `<app support>/notes.json` is in private internal storage, apart from downloaded content, so removing content never touches notes. Saves happen 400 ms after a change, plus at once on leaving a note or when the app is backgrounded. Writes are synchronous (the file is a few KB), so a save is complete before the app can be killed. Each write goes to a temporary file that is then renamed over the old one, so a crash never leaves half a file. A file that cannot be read is renamed `notes.json.unreadable-<time>`, never overwritten. Empty notes are not kept.
- **Search** (`app/lib/notes_search.dart`, 2026-11-04):
  - **Keywords, always.** BM25 over the notes, with the title counted twice. Common little words are ignored ("who to call if someone is ill" is about calling and illness), and the word being typed also matches the start of longer words ("batt" finds batteries, "0770" a phone number). It needs nothing downloaded.
  - **Meaning, when the embedding model is installed.** The embedded server's `POST /api/embed` uses the same pinned model and query instruction as the library. Each note is embedded whole and line by line, with the title in front of each line ("Car: MOT due 14 June"), and scores as its best part. A meaning match must reach cosine ≥ 0.56 and be within 0.04 of the best note.
  - **Merging.** Keyword and meaning rankings are merged by reciprocal rank fusion (k = 60).
  - **Cache.** Vectors are cached in `notes-emb.json` beside `notes.json`, keyed by a hash of what was embedded. They are refreshed in the background 2 s after a change, so a search only waits for the query (~300 ms debounce). The cache is never needed to read notes.
  - **Measured** on 10 everyday notes and 20 queries with the real model: the right note comes first for every query that has one, including "blood pressure medicine" → Dad's tablets, "vegetables" → Garden, "vehicle test" → Car and "evacuation" → Grab bag. "earthquake" finds nothing. Whole-note vectors alone got 13 of 17. Nonsense like "cat food" can still bring up the closest note or two.
  - **Fallback.** Without the model, or while the library restarts, search is keywords only; the hint says "Search notes by words or meaning" once meaning search works.
- **Code:** `app/lib/notes.dart` and `notes_search.dart`, with tests in `app/test/notes_test.dart` and `notes_search_test.dart` and screenshots `notes-*` in `screens_test.dart`.

## 19. Sharing nearby, chat, and books in the browser (built 2026-11-02)

During a power cut, one phone on a power bank can serve everyone around it. The phone runs the same server as the hub, so people use the library, books, map, games and chat in their own browser. iPhones work too, with no app and no internet. A hub serves the same pages.

**Why not the BLE + SoftAP spec.** A proposed spec used Bluetooth discovery, a Wi-Fi handoff, UDP broadcast and peer-to-peer Noise sessions. We did not adopt it, for these reasons:
- iOS apps cannot set BLE manufacturer data, and they lose their local name in the background.
- UDP broadcast on iOS needs Apple's multicast entitlement.
- Wi-Fi broadcast is lossy, with no retries, and phones in power-save mode miss packets.
- `WifiNetworkSpecifier` shows a system dialog on every join, and only exists from Android 10.
- Android 11+ randomises the hotspot subnet.
- Some phone makers isolate hotspot clients from each other.
- We have no iOS app. The two QR codes below need none of these, and they work on every phone camera.

**On the phone** (`app/lib/share.dart`, `Share.kt`; Content → Share nearby):
- **Start a Wi-Fi network:** Android's local-only hotspot, with a name and password Android picks each time.
  - Permissions: Android 8–12 need location (fine plus coarse) with Location switched on; 13+ need `NEARBY_WIFI_DEVICES` (`neverForLocation`).
  - Failures are explained in the app. The common one is the phone's own hotspot being on (`ERROR_INCOMPATIBLE_MODE`), and the app offers its settings.
- **Use my hotspot or this Wi-Fi:** the phone shares on whatever network it is on, such as its own tethering hotspot or a shared router.
- **QR codes:** the screen shows the standard `WIFI:T:WPA;S:…;P:…;;` code (Android and iPhone cameras join from it) and the address `http://<ip>:8080/`.
  - The IP is the hotspot interface's IPv4, read from the phone's network interfaces. Mobile data and VPN are skipped.
- **Server:** the Rust core keeps its 127.0.0.1 port for the app and adds a listener on `0.0.0.0:8080` (or the next free port up to 8089) through `sp_share`.
  - The listener survives core restarts, for example after a download.
  - When sharing, the core runs even with no content, for chat and games.
- **Running in the background:** a `connectedDevice` foreground service holds a partial wake lock and a Wi-Fi lock, with a Stop button in its notification. The screen can be off.
  - A strip above the tabs shows the address while sharing.
- **What visitors get:**
  - the home page, search and articles, if collections are installed;
  - /books (the phone's own books, read from `installed.json` on each request);
  - /games (the phone's games directory);
  - /map, if maps are installed;
  - /ask, if the model is installed;
  - /chat.

**Chat** (`sp-server --chat DIR`, `chat.rs`, `chat.html`):
- One room for everyone, plus private messages, from any browser on the same Wi-Fi.
- **Identity:** a random id plus a secret token that the browser makes and keeps in localStorage. The first join claims the id, and the server stores only the token's BLAKE3.
  - The app's own user has an identity kept in preferences, which is passed in the URL fragment.
- **Delivery:** long polling (`/api/chat/poll?wait=1`, 25 s, woken by `tokio::Notify`). It needs no WebSocket and works in every browser.
- **Storage:** messages are appended to `messages.jsonl`, and people go in `people.json`. The last 3,000 messages are kept in memory, so people who join later catch up.
- **Limits:** text up to 2,000 characters, at most 10 messages per 10 s per person.
- **Notices:** "Ann joined" and renames are posted as notices.
- **Phone numbers** in messages become `tel:` links.
- **Photos, voice messages and places** (`/api/chat/upload`, `/api/chat/file/{name}`):
  - **Photos:** the page shrinks them before sending (longest side 1280 px, JPEG 0.82, usually 100–300 KB), with an optional caption. Tap to see one full size.
  - **Voice messages:** recorded in the page with MediaRecorder (Opus in WebM, or MP4 on Safari, 32 kbit/s, up to 60 s). Where the page cannot record, people send a file from their phone's recorder app instead.
  - **Places:** GPS ("My location"), or a place picked on the map (`/map?pick=1` hands it back through sessionStorage). A place shows as a card with "Show on map" (`/map?pin=lat,lon&label=`) and a link to the phone's maps app (`geo:` on Android, Apple Maps on iPhone).
  - **Files:** the server checks the type from the bytes (JPEG/PNG/WebP/GIF; WebM/Ogg/MP4/MP3/WAV), not from what the browser says. Limits are 4 MB per photo and 3 MB per voice message. Files take at most 256 MB in all; past that the oldest are removed (down to 90%), and their messages say "removed to save space".
  - A file is served only to someone who can see its message, so files in private messages stay private. Range requests are supported, which Safari needs to play audio.
- **HTTPS for the microphone and GPS** (`tls.rs`): browsers allow both only on a secure page, and there is no internet to get a real certificate from.
  - The server makes a self-signed certificate on first use and keeps it in the chat directory (`https-cert.der`, `https-key.der`), so returning visitors see the same one.
  - The hub uses `--https 0.0.0.0:8443`; a sharing phone opens 8443 (or the next free port) next to its http port.
  - The http address stays the main one, since it has no warning. Only when someone taps Voice message or My location does the page explain that the browser will warn once and how to get past it (Android, iPhone), then open the https chat with their identity in the URL fragment. Each address keeps its own storage, so the identity has to be carried over.
  - The app's own chat is on 127.0.0.1, which browsers already treat as secure. Its WebView asks Android for the microphone and location when the page needs them, and opens the gallery or camera for its file input (`ChatMedia.kt`).
- **Privacy:** private messages are shown only to the two people, but they are stored on the server like everything else, and the page says so.
  - End-to-end encryption would not protect against the host anyway, because the host serves the page's JavaScript to browsers. It is left for app-to-app use, if ever.

**Phone to phone** (`offer.rs`; app: Content → the Wi-Fi search button, `findNearby` in `share.dart`):
- **Serving:** a sharing phone offers what it has to other phones running the app, which download from it exactly as from a hub. The server gives:
  - `/api/catalog`, in the hub's format with `"source": "phone"`;
  - `/api/books`, the hub shelf's format without categories;
  - `/download/{path}`.
- **Checksums:** everything comes from the app's `installed.json`, so nothing is hashed again on the phone; the hashes were checked when the files arrived. The receiving phone checks every file once more, so a copy of a copy is still verified.
- **What is offered:**
  - only files listed in `installed.json`, and only components whose files are all present at their recorded size;
  - the person's own imported EPUBs are never offered, only Project Gutenberg books;
  - a book is served under both edition names, because a phone holds only the edition it chose.
- **Finding a phone:** the app tries every address of each /24 it is on, on ports 8080 and 8081 (48 at a time, a 0.7 s connect timeout, a few seconds in all), and keeps those that answer with a catalog. On the sharing phone's own hotspot, this finds that phone; on a shared router, any sharing phone or hub. The chosen address becomes the hub address, so the Books tab uses it too.
- **Getting the app:** `/app.apk` serves the app itself: the installed APK on a phone (`applicationInfo.sourceDir`), or `--apk` (default `<maps>/survivalpedia.apk`) on a hub. The home page shows "Get the app" to Android browsers only.
- **Speed:** a phone hotspot gives roughly 50–200 Mbit/s locally, so Essentials takes 1–3 minutes.

**Books in the browser** (`reading.rs`):
- **On a hub** (`--books`):
  - /books shows the 18 most-read books, the shelf's groups and categories as chips, and search through the existing BookIndex.
  - /books/c/{i} lists a category, 60 per page.
- **On a phone:** the books on the phone.
- **Reader:** books open in the app's own reader (`app/assets/reader`, compiled into sp-server) with `web=1`, which keeps settings, places and page locations in the browser's localStorage.
  - "Download this book" saves the EPUB, for example for Books on iPhone.
  - Curated books open in their illustrated edition (`/read/book/1342i.epub`).

## 20. First aid cards (built 2026-11-04)

Search ranks encyclopaedia articles, which is the wrong answer to "my child drank bleach". The fix is a small set of hand-written cards that say what to do now.

- **Content:** `app/assets/emergency/cards.json`, 15 cards: CPR, choking, severe bleeding, shock, burns, stroke (FAST), heart attack, poisoning, the recovery position, hypothermia, heat exhaustion and heatstroke, seizure, severe allergic reaction, asthma, and low blood sugar.
  - Each card has `when`, numbered `steps`, `children` (children and babies), `dont`, `call_if`, and `more` (library article titles).
  - The wording is our own, following Resuscitation Council UK / ERC 2021, NHS, British Red Cross and St John Ambulance guidance. It is plain and in the second person.
  - `"reviewed": false`: the cards have not yet been checked by a medical professional, and the disclaimer says so. Get that check before a wide release.
- **On the phone:** on the Library tab, the first tab. The library's home page and header link to `/emergency`, and its Call links open the dialler.
  - With no library, or if the core fails to start, the Library tab's message has a **First aid** button. It opens the same cards built into the APK (`app/lib/emergency.dart`), so they work with nothing downloaded.
  - A card shows large numbered steps, and on Android buttons for 112 / 999 / 911 / 000, which open the dialler with the number filled in (the `dial` method channel).
  - Not a floating button: the user found it intrusive, and switching to the first tab is quick.
- **On a hub or shared phone** (`core/crates/sp-server/src/emergency.rs`, the same JSON through `include_str!`):
  - `/emergency` shows every card, with `tel:` links, and links to the `more` articles that are installed.
  - "First aid" is in the header and on the home page.
  - A search, or a question in Ask, that sounds like an emergency gets the matching card in a red box above the results. Ask sends it as a `card` event first, so it shows before the model loads.
- **Matching:** whole words, case-insensitive, apostrophes dropped. A card matches on any of its `keywords`, or on a `weak` word together with a different `with` word (so "bleach" alone, as in water purification, doesn't match, but "drank bleach" or "child … bleach" does). Any `not` word blocks the card: bleeding brakes, two-stroke engines, poison ivy, food poisoning.
  - At most two cards are shown, and the second only on a keyword.
  - Tests list emergencies that must match and everyday searches that must not. On the 931 Essentials test questions, 36 get a card, all of them on topic. Run `card_audit` (an ignored test) to review a query file by eye.
