# Architecture

Bothy has three parts:
- a **build pipeline** that turns sources into packs (Python, then Rust);
- a **core** that reads and searches packs (Rust);
- two **front ends** over the same core: a server for any browser, and an Android app.

```
            build machine (GPU helps for embedding)
 ┌──────────────────────────────────────────────────────────────────────┐
 │ ZIM files, PDFs, your documents                                      │
 │   → ingest/*_extract.py   clean HTML, text blocks, chunks, images    │
 │   → ingest/embed.py       Qwen3-Embedding-0.6B vector per chunk      │
 │   → ingest/essentials/    choose which articles go in (ranking)      │
 │   → sp build              pack: compressed docs, BM25, vector index  │
 └───────────────────────────────┬──────────────────────────────────────┘
                                 │ packs (one per collection)
                                 ▼
          ┌────────────────────── Rust core ───────────────────────┐
          │ sp-pack   read docs, titles, images                     │
          │ sp-search BM25 (Tantivy) + IVF vectors, fused by RRF    │
          │ sp-embed  query embedding (llama.cpp, CPU)              │
          │ sp-rag    answers from passages (llama.cpp, CPU)        │
          │ sp-maps   PMTiles maps, place search                    │
          │ sp-server HTML pages + JSON API (axum)                  │
          └──────┬──────────────────────────────┬──────────────────┘
                 │                              │
           sp-server binary                sp-ffi (C ABI)
        hub / laptop / Pi:              Android app (Flutter):
        any browser on the Wi-Fi        WebView on 127.0.0.1, native tabs,
                                        and the same server shared on Wi-Fi
```

The phone and the hub run **the same server code**. The app starts it on 127.0.0.1 and shows its pages in a WebView. When a phone shares, it also opens that server to the Wi-Fi, so a phone can act as a hub during a power cut.

## Repository layout

```
core/                 Rust workspace
  crates/sp-pack      pack format: writer and reader (docs.idx, blocks, titles.fst, images, chunks)
  crates/sp-search    BM25 index (Tantivy), IVF vector index, hybrid search, multi-collection Library
  crates/sp-embed     query embedding with llama.cpp (must match ingest/embed.py exactly)
  crates/sp-rag       local answering: prompt, streaming, citation handling
  crates/sp-maps      PMTiles reader, region extraction over HTTP ranges, place index (FST)
  crates/sp-server    axum server: pages, JSON API, catalog/downloads, books, games, maps, chat, HTTPS
  crates/sp-ffi       C ABI for the app: sp_start, sp_stop, sp_share, sp_blake3
  crates/sp-cli       `sp`: build, browse, search, get, title, map-extract, map-places, …
ingest/               Python (uv): sources → extracts → embeddings → selection
  zim_extract.py      Kiwix ZIM → extract (HTML cleaning, blocks, chunking, images)
  guides_extract.py   PDF/HTML survival guides → extract
  docs_extract.py     your own Markdown/HTML/text → extract
  embed.py            chunk embeddings (torch, or an OpenAI-compatible server)
  pageviews.py        Wikipedia pageview totals (for core/rest splits)
  essentials/         ranking Wikipedia for the Essentials pack (DESIGN §6)
  books/              Project Gutenberg EPUB shelf
app/                  Flutter app (Android)
  lib/main.dart       tabs: Library, Books, Map, Notes, Games, Content
  lib/core.dart       loads libsp_ffi.so, starts the server
  lib/content.dart    catalog, downloads, installed.json
  lib/books.dart      shelf, book downloads, reader
  lib/share.dart      share nearby, find nearby phones
  lib/notes.dart      notes and checklists
  android/…/*.kt      hotspot, foreground service, file picking, chat media, book provider
  assets/games/       our games (bundled)
  assets/reader/      EPUB reader page (epub.js)
tools/sp_pack.py      reference pack reader in plain Python
scripts/              source downloads, map assets, third-party games, checks
maps/regions.json     map regions people can download
eval/                 search test questions (golden, synthetic, essentials coverage)
examples/my-notes/    a tiny collection made from Markdown/text files
docs/                 this documentation
```

## Build pipeline

1. **Fetch.** `scripts/fetch_sources.sh` downloads Kiwix ZIM files (Wikipedia, wikiHow, …) with checksums. `scripts/fetch_wikimedia_meta.sh` fetches the Wikipedia link graph, categories and pageviews, which the ranking uses.
2. **Extract.** `zim_extract.py` reads a ZIM file in cluster order (fast on slow disks), cleans each article's HTML down to an allow-list, gives every block an id, splits the text into ~256-token chunks, and re-encodes images as WebP. Output: an **extract** (DATA-FORMATS §1). `guides_extract.py` and `docs_extract.py` produce the same format from other inputs.
3. **Embed.** `embed.py` embeds every chunk with Qwen3-Embedding-0.6B and stores f16, int8 and sign-bit versions. Vectors are keyed by a hash of the text, so later runs re-embed only what changed. Full English Wikipedia (~50M chunks) runs for days on one GPU and is written in resumable shards.
4. **Select.** For Essentials, `ingest/essentials/` ranks Wikipedia articles by usefulness in a crisis (CONCEPTS: "Essentials"). It produces a TSV. `subset.py` cuts the full extract down to the chosen articles, and `embed.py --reuse` takes their vectors from the full run instead of embedding them again.
5. **Pack.** `sp build` writes a pack:
   - docs compressed in 64 KB zstd blocks with a trained dictionary;
   - a title FST;
   - an image store;
   - a Tantivy BM25 index;
   - IVF vector indexes at 256 and 128 dims, split into `core` (most-read articles) and `rest`;
   - a manifest with BLAKE3 hashes.

   `sp browse` adds browse sections.
6. **Serve.** Put packs in a library directory and run `sp-server --library DIR --downloads`. Phones copy collections from the hub, and then from each other.

## Search

A query is answered in a few milliseconds, even on a phone:

1. **BM25** over each collection's Tantivy index (English stemming), on chunk text plus title.
2. **Dense:**
   - The query is embedded on the CPU by llama.cpp (Q8_0 GGUF, ~60 ms), with the pinned instruction prefix, and cut to 256 or 128 dims.
   - Each installed vector index finds its nearest IVF lists, scans their sign-bit codes, and reranks the best with int8.
   - The index files are memory-mapped and read in small ranges, so search stays fast from slow phone storage or a USB drive.
3. **Fusion:**
   - Results from all collections are merged: BM25 by score and dense by cosine. Every pack uses the same model, so cosines compare across packs.
   - The two lists are then combined by reciprocal rank fusion.
   - Each article is represented by its best chunk.
4. The hit links to the article at the chunk's first paragraph (`#p12`), which is highlighted.

Without the embedding model or vector indexes, search is BM25 only. Everything still works.

## Answering (RAG)

`/ask` is a fixed pipeline, not a free-running agent:
1. Search, and take the best passage of each of the top 4 articles.
2. A small local model (Qwen3.5-2B, Q4_K_M, ~1.3 GB, CPU) answers only from those passages.

Sources are always listed under the answer. Follow-up questions carry the last few turns.

## The app

- **Tabs:**
  - **Library:** search and articles, the server's pages in a WebView.
  - **Books:** native shelf, with an epub.js reader in a WebView.
  - **Map:** the offline map.
  - **Notes:** native, works with nothing downloaded.
  - **Games:** HTML games served on a fixed local port, so their saves persist.
  - **Content:** downloads, sharing nearby, finding nearby phones.
- **Content** comes from a hub or a nearby phone's `/api/catalog`:
  - Files are downloaded by a foreground service, resumed with HTTP ranges, and checked against BLAKE3 hashes.
  - They are recorded in `installed.json` under the app's storage, at the same relative paths as the catalog.
  - The core is restarted to pick them up.
- **Native code:** the Rust core is built for `arm64-v8a` with `cargo-ndk`. The llama.cpp kernels use dotprod and i8mm, so the app needs a 2022-or-newer CPU (Cortex-A710/X2 class).
- **Builds:** GitHub Actions builds the APK (`.github/workflows/android.yml`).

## Data placement

Build data lives outside the repository. Sources, extracts, embeddings and packs for full English Wikipedia come to several terabytes in total. Random-access steps (embedding, pack building) want an SSD; finished outputs can live on a large HDD.
