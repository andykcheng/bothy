# Data formats

This page describes every file Survivalpedia reads or writes, so you can use our data in your own project or produce data our tools accept. The formats are open (CC0); the content inside them keeps its own licence (see "Licences" at the end).

The pipeline has four kinds of data:

| Stage | What | Made by | Read by |
|---|---|---|---|
| **Extract** | Clean HTML, text blocks, chunks, images, as JSON Lines | `ingest/zim_extract.py`, `guides_extract.py`, `docs_extract.py` | `embed.py`, `sp build` |
| **Embeddings** | One vector per chunk, as NumPy arrays | `ingest/embed.py` | `sp build` |
| **Pack** | The shipped, compressed, indexed collection | `sp build` | `sp-server`, the app, `tools/sp_pack.py` |
| **Catalog** | What a hub or phone offers for download | `sp-server --downloads` | the app |

The extract is the easiest format to work with: plain JSON Lines. The pack is the compact one: zstd-compressed and memory-mappable, for phones. To get the text back out of a pack, use `tools/sp_pack.py dump` (see "Reading a pack" below).

Conventions:
- All binary integers are **little-endian**.
- JSON files are UTF-8. JSON Lines files (`.jsonl`) hold one object per line.
- Doc numbers and chunk ordinals in a pack are dense `u32`s, assigned at build time in input order.

---

## 1. Extract (an ingest output directory)

One directory per source, for example `build/wikipedia-en/` or `my-notes-extract/`:

```
source.json        what the source is
docs.jsonl         one article per line
chunks.jsonl       search passages, ~256 tokens each
redirects.jsonl    other names for articles
images.jsonl       one line per unique image
images/<k2>/<key>.320.webp, <key>.640.webp, <key>.svg
emb/               embeddings (section 2), added by embed.py
```

### source.json

```json
{
 "source": "my-notes",            // short id: lower-case letters, digits, '-'
 "lang": "en",
 "site": "docs",                  // wikipedia | wikihow | guides | docs | …
 "title": "Household emergency notes",
 "author": "…", "url": "https://…", "license": "CC BY 4.0",
 "docs": 3, "chunks": 4, "redirects": 3, "images_unique": 1,
 "chunking": {"tokenizer": "Qwen/Qwen3-Embedding-0.6B", "target_tokens": 256, "max_tokens": 384},
 "zim_metadata": {"Title": "…", "Date": "2023-03-21", "Source": "en.wikipedia.org", "Creator": "…", "Publisher": "…"}
}
```

`zim_metadata` exists only for sources read from a Kiwix ZIM file. Where it is missing, `sp build` takes the source's title, date, origin URL, creator and publisher from the top-level `title`, `date`, `url`, `author` and `publisher` fields.

### docs.jsonl

```json
{
 "doc_id": "my-notes/water/boiling-water",   // unique across all sources: "<source>/<path>"
 "source": "my-notes",
 "lang": "en",
 "title": "Boiling water to make it safe",
 "path": "water/boiling-water",              // the article's path within its source; links use it
 "url": "https://…",                         // original URL, or null
 "page_id": 2, "rev_id": null, "rev_date": null,
 "license": "CC BY 4.0",                     // required: every article carries its licence
 "author": "…",                              // optional
 "categories": ["Water", "Hygiene"],
 "browse": ["Water"],                        // optional: section path for the browse pages
 "html": "<p id=\"p0\">Boiling is …</p> <p id=\"p1\"><img src=\"img:b485b8816a31b8d74b6c0350\" alt=\"…\"></p> …",
 "blocks": [{"id": "p0", "t": "p", "text": "Boiling is …"}, {"id": "p2", "t": "h2", "text": "How to do it"}, …],
 "images": ["b485b8816a31b8d74b6c0350"]
}
```

**The HTML** is cleaned and simplified. Only these elements are allowed:

`p h2 h3 h4 ul ol li table tr td th figure img figcaption a b i sup sub blockquote pre code`

Scripts, styles, forms, navigation boxes and edit links are removed. In addition:
- Every block-level element (paragraph, heading, list item, table) has a stable `id` (`p0`, `p1`, …). Search hits link to these ids, so a hit opens at the right paragraph.
- Images are `<img src="img:KEY">`, where `KEY` is the image key (see images.jsonl).
- Links to other articles are `<a href="wiki:PATH">` or `wiki:PATH#anchor`. They resolve by path or title within the same collection, through the article's own path, its title, or a redirect. Web links stay ordinary `https:` links.

**blocks** is the plain text of the article, one entry per `id` in the HTML. `t` is the element (`p`, `h2`–`h4`, `li`, `td`, `pre`, …). Search indexes and chunks are built from the blocks, not from the HTML.

### chunks.jsonl

```json
{"chunk_id": "my-notes/water/boiling-water#0", "doc_id": "my-notes/water/boiling-water",
 "block_ids": ["p0", "p3", "p4", "p5", "p6"], "n_tokens": 101,
 "text": "Boiling water to make it safe\nBoiling is the most reliable way …\nIf the water is cloudy, …"}
```

A chunk is a run of whole blocks of up to about 256 tokens, counted with the Qwen3 embedding tokenizer:
- A new `h2` section starts a new chunk once the current chunk has at least 64 tokens.
- A single block longer than 384 tokens is split on its own.
- Headings are not part of the chunk body. Instead, each chunk's `text` starts with one line naming the article and section (`Title` or `Title > Section > Subsection`), so the passage makes sense on its own.
- Chunk `#0` of an article is its lead.

### redirects.jsonl

```json
{"from_path": "Boiled_water", "from_title": "Boiled water", "to_path": "water/boiling-water"}
```

Other names that lead to an article. Title lookups and `wiki:` links use them.

### images.jsonl and images/

```json
{"key": "b485b8816a31b8d74b6c0350", "w": 640, "h": 480, "variants": ["320", "640"],
 "paths": ["water/images/boil.png"], "src_bytes": 18233}
```

- `key` is the first 24 hex characters of the SHA-256 of the original image bytes, so an image used by many articles is stored once.
- Bitmaps are WebP (quality ~60) at 320 and 640 px wide, never enlarged. Images 320 px wide or less have only the 320 variant. SVGs are kept as SVG.
- Files: `images/<first 2 hex chars>/<key>.320.webp`, `<key>.640.webp` or `<key>.svg`.

---

## 2. Embeddings (`<extract>/emb/`)

Written by `ingest/embed.py`. One row per chunk, in the order of `chunk_ids.txt`:

| File | Shape, type | Content |
|---|---|---|
| `manifest.json` | | model, revision, dims, query instruction, pooling, quantisation |
| `chunk_ids.txt` | N lines | chunk ids, one per row |
| `full_f16.npy` | (N, 1024) float16 | unit-length full vectors |
| `emb_f16.npy` | (N, D) float16 | the first D dims (default 256), renormalised to unit length |
| `emb_i8.npy` | (N, D) int8 | `round(x / i8_scale * 127)` |
| `i8_scale.npy` | (D,) float32 | per-dimension scale for int8 |
| `emb_bin.npy` | (N, D/8) uint8 | sign bits: `np.packbits(x > 0, axis=1)` |
| `hashes.npy` | (N, 16) uint8 | `sha256(model + text)[:16]`, so unchanged chunks are reused by later runs (`--reuse`) |

**The model and query prefix are pinned.** Vectors from different models can't be compared, so any query vector must be made the same way:
- **Model:** [Qwen/Qwen3-Embedding-0.6B](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B). Last-token pooling, L2-normalised.
- **Documents** (chunk texts) are embedded with no prefix.
- **Queries** are embedded with this exact prefix:
  ```
  Instruct: Given a question, retrieve passages that answer it\nQuery: 
  ```
  followed by the question.
- **Shorter vectors:** the model is Matryoshka-trained, so you can take the first 256 or 128 dims and renormalise.

On a phone or hub, queries are embedded by llama.cpp from `Qwen3-Embedding-0.6B-Q8_0.gguf` ([Qwen/Qwen3-Embedding-0.6B-GGUF](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF)).

Large sources are embedded in shards: `emb/shards/NNNNN/` each hold `meta.json` (`start_offset`, `end_offset`: the byte range of `chunks.jsonl` it covers), `chunk_ids.txt` and `full_f16.npy`.

---

## 3. Pack (a collection)

A pack is one directory. `sp build` writes it from one or more extracts. Every file is listed with its size and BLAKE3 hash in `manifest.json`.

```
manifest.json
docs.idx           SPDI header + DocEntry[docs]
docs-000.bin …     zstd-compressed blocks of doc records, each shard ≤ 4 GB
dict-en.zst        zstd dictionary for those blocks
titles.bin         all titles, UTF-8, back to back
titles.fst         title and redirect lookup
images.idx         SPII header + ImageEntry[images]
img-core-000.bin … image bytes, each shard ≤ 4 GB
chunks.bin         SPCI header + ChunkEntry[chunks]
index/bm25-en/     Tantivy full-text index
index/vec/<name>/  vector indexes (optional), e.g. my-notes-en.core.d256
browse.json        sections for the browse pages (optional; `sp browse`)
models/*.gguf      embedding model (optional; usually shared by the library instead)
```

Shards stay under 4 GB so packs fit on FAT32 drives and can be copied and checked piece by piece.

### Binary header

`docs.idx`, `images.idx` and `chunks.bin` start with a 16-byte header:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic: `SPDI`, `SPII` or `SPCI` |
| 4 | 4 | format version, u32 (currently 1) |
| 8 | 8 | entry count, u64 |

The fixed-size entries follow immediately.

### docs.idx: DocEntry (32 bytes)

| Offset | Type | Field |
|---|---|---|
| 0 | u16 | `shard`: the record is in `docs-<shard, 3 digits>.bin` |
| 2 | u8 | `lang`: 0 = en (1 = zh, 2 = ja: old packs only) |
| 3 | u8 | `source`: index into `manifest.sources` |
| 4 | u32 | `block_clen`: compressed length of the block |
| 8 | u64 | `block_off`: byte offset of the block in the shard |
| 16 | u32 | `rec_off`: offset of the record in the *decompressed* block |
| 20 | u32 | `rec_len`: record length in bytes |
| 24 | u32 | `title_off`: offset in `titles.bin` |
| 28 | u32 | `title_len` |

To read doc `n`:
1. Read the 32 bytes at `16 + 32 × n`.
2. Read `block_clen` bytes at `block_off` in the shard.
3. Decompress them with zstd, using `dict-<lang>.zst` as the dictionary.
4. Take `rec_len` bytes at `rec_off`. That is the doc record, as JSON.

Blocks hold about 64 KB of records back to back, compressed at zstd level 19. Neighbouring docs share a block, so cache the last block you decompressed. The dictionary is normally a trained zstd dictionary (112 KB). A very small pack may get a "raw content" dictionary instead; zstd libraries accept both.

**The doc record** is the extract's docs.jsonl line without `blocks`, `images` and `path`: `doc_id, source, lang, title, url, page_id, rev_id, rev_date, license, categories, html`, plus `author` and `browse` when present.

### titles.fst

An [fst](https://docs.rs/fst) map (BurntSushi's format) from `normalise(title) + 0x1F + source index` to the doc number. It holds every title and every redirect.

Normalising a title:
1. NFKC.
2. Lower-case.
3. `_` → space.
4. Runs of whitespace collapse to one space, and leading and trailing whitespace is trimmed.

A prefix search over it gives title autocomplete.

### images.idx: ImageEntry (28 bytes), sorted by (key, variant)

| Offset | Type | Field |
|---|---|---|
| 0 | 12 bytes | key (the 24-hex-char key as bytes) |
| 12 | u8 | variant: 0 = 320 px WebP, 1 = 640 px WebP, 2 = SVG |
| 13 | u8 | padding |
| 14 | u16 | shard: `img-core-<shard, 3 digits>.bin` |
| 16 | u32 | length |
| 20 | u64 | offset |

Image bytes are stored as-is (WebP or SVG), not compressed again. Find an image by binary search on the key.

### chunks.bin: ChunkEntry (12 bytes)

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | doc number |
| 4 | u32 | first block (index into the doc's blocks) |
| 8 | u32 | last block |

The chunk ordinal is the id used by both search indexes. Each entry maps it back to a doc and to the paragraph range a hit should highlight.

### index/bm25-en/

A [Tantivy](https://github.com/quickwit-oss/tantivy) index with one document per chunk:
- `title` (text), the article title;
- `body` (text), the chunk text;
- `chunk` (u64, stored, fast), the chunk ordinal.

Text uses English stemming.

### index/vec/<name>/

An IVF index. Vectors are grouped into `lists` clusters by k-means. A query scans the nearest lists using 1-bit codes, then reranks the best candidates with int8 vectors. Names are `<source>.<core|rest>.d<256|128>`:
- `core` is the source's most-viewed articles (with no view counts, everything is core);
- `rest` is everything else;
- `d128` is the same vectors cut to 128 dims.

Every component is optional, and a reader uses whichever ones are present.

| File | Content |
|---|---|
| `vec.json` | `{"n", "dims", "lists", "list_size_min", "list_size_max"}` |
| `centroids.f32` | lists × dims float32, unit length |
| `offsets.u32` | lists + 1 prefix offsets into the arrays below |
| `ids.u32` | n chunk ordinals, in list order |
| `codes.bin` | n × dims/8 bytes, sign bits, most significant bit first (`np.packbits` order), list order |
| `i8.bin` | n × dims int8, list order; value ≈ `i8 / 127 × scale[dim]` |
| `scale.f32` | dims float32 |

To search:
1. Embed the query (section 2) and truncate it to `dims`.
2. Take the dot product with every centroid and keep the best few dozen lists.
3. Score every vector in those lists by Hamming similarity of the sign bits.
4. Rerank the top few hundred with int8 dot products.

Scores from different components and packs are comparable (same model), so you can merge their results by score.

### browse.json

```json
{"order": "title", "sections": [{"path": ["Water"], "docs": [1, 2]}, {"path": ["Power"], "docs": [0]}]}
```

`path` is a section path (`["Health", "First aid"]`) and `docs` are doc numbers. `order` is `title` (A–Z) or `pack` (keep the pack's order, for guides read front to back).

### manifest.json

```json
{
 "format": "survivalpedia-pack", "format_version": 1,
 "collection": {"id": "my-notes", "title": "Household emergency notes", "description": "…", "kind": "guides"},
 "created_unix": 1793660751,
 "languages": ["en"],
 "sources": [{"id": "my-notes-en", "lang": "en", "title": "…", "date": null, "origin": "https://…",
              "creator": "…", "publisher": null, "docs": 3, "redirects": 3,
              "licenses": {"CC BY 4.0": 3}}],
 "counts": {"docs": 3, "chunks": 4, "images": 1},
 "embedding": {"model": "Qwen/Qwen3-Embedding-0.6B", "model_revision": "…", "dims": 256, "full_dims": 1024,
               "query_instruction": "Instruct: Given a question, retrieve passages that answer it\nQuery: ",
               "pooling": "last_token", "normalize": "l2", …},
 "vec_index": {…},
 "sizes": {…}, "stats": {…},
 "files": [{"path": "docs-000.bin", "size": 18, "blake3": "2c04…"}, …],
 "license_note": "…"
}
```

- `collection.id` is stable. It appears in URLs (`/doc/<id>/<n>`) and in download paths, so don't change it between versions.
- `kind` is a hint for the app: `articles`, `guides` or `books`.
- Check `embedding.model` and `embedding.query_instruction` before you embed queries for this pack.

### Reading a pack

`tools/sp_pack.py` is a small reference reader in plain Python. It reads everything above except the two search indexes, and its only dependency is `zstandard`:

```sh
pip install zstandard
python tools/sp_pack.py <pack> info
python tools/sp_pack.py <pack> list 20
python tools/sp_pack.py <pack> get 1                  # one record as JSON
python tools/sp_pack.py <pack> image <key> out.webp
python tools/sp_pack.py <pack> dump all.jsonl         # every record, one per line
```

```python
from sp_pack import Pack
p = Pack("essentials-wikipedia")
for n in range(p.doc_count):
    r = p.record(n)            # dict with title, url, license, html, …
```

The Rust reader is `core/crates/sp-pack` (`sp_pack::Pack`), and search is in `core/crates/sp-search`.

---

## 4. Hub catalog and the phone's installed.json

A hub started with `--downloads` (or a phone that is sharing) serves `GET /api/catalog`:

```json
{
 "format": "survivalpedia-catalog", "version": 2, "source": "hub",       // or "phone"
 "groups": [{"id": "col:guides", "title": "Survival guides", "description": "…", "kind": "guides",
             "docs": 95, "created_unix": 1793532733}],
 "components": [
  {"id": "col:guides", "title": "Articles + keyword search (95 articles)", "group": "col:guides", "size": 41234567,
   "files": [{"path": "collections/guides/docs.idx", "size": 3056, "blake3": "…"}, …]},
  {"id": "col:guides:images", "requires": ["col:guides"], …},
  {"id": "col:guides:vec:guides-en.core.d256", "requires": ["col:guides", "model"], …},
  {"id": "model", …}, {"id": "map-assets", …}, {"id": "map:uk", "requires": ["map-assets"], "bounds": […], "max_zoom": 15, …},
  {"id": "game:chess", "description": "…", "players": "1–2", "license": "…", "cover": "games/chess/cover.png", …},
  {"id": "llm", "model": "…", …}
 ],
 "books": {"index": "/api/books", "download": "/download/books"}
}
```

- Each file is fetched from `/download/<path>` and saved at the same relative path under the app's content directory. The hash is checked when the file arrives.
- A component is usable when its `requires` are installed.

On the phone, `<content>/installed.json` records what is installed, keyed by component id:

```json
{"col:guides": {"id": "col:guides", "title": "…", "group": "col:guides", "description": "…", "cover": null,
                "size": 41234567, "requires": [], "files": [{"path": "collections/guides/docs.idx", "size": 3056, "blake3": "…"}]}}
```

A sharing phone builds its own catalog from this file, so copies from phone to phone are checked against the original hashes.

---

## 5. Book shelf (`shelf.json`)

Written by `ingest/books/epub_shelf.py index`. It indexes Project Gutenberg EPUBs, which a hub serves with `--books DIR`:

```
shelf.json, shelf.json.gz
shelf-text.jsonl         {"id", "d": description, "sub": [subjects]} per book, for search
epub/<id>.epub           text-only EPUB
epub-images/<id>.epub    illustrated EPUB3 (some books)
covers/<id>.jpg
```

```json
{
 "version": 1, "source": "Project Gutenberg (gutenberg.org); …",
 "categories": [{"name": "Adventure", "count": 812}, …],
 "groups": [{"name": "Fiction", "categories": [0, 1, 2]}, …],
 "books": [{"id": 1342, "t": "Pride and Prejudice", "a": "Jane Austen", "c": [4, 0, 9, 10], "r": 1,
            "s": 563992, "si": 24830846, "cs": 31675}]
}
```

Each book has these fields:

| Field | Meaning |
|---|---|
| `t` | title |
| `a` | author |
| `c` | category indexes |
| `r` | popularity rank by Gutenberg downloads |
| `s` | EPUB size in bytes |
| `si` | illustrated EPUB size, when there is one |
| `cs` | cover size, when there is one |

---

## 6. Games

One directory per game, with `index.html` and `game.json`:

```json
{"id": "chess", "title": "Chess", "description": "…", "players": "1–2", "version": 2,
 "license": "MIT OR Apache-2.0", "author": "…", "cover": "cover.jpg"}
```

- A game must work offline as plain HTML/JS, and keeps its saves in `localStorage`.
- It should pause when the page is hidden (`visibilitychange`) and on the app's `sp-pause` event.

---

## 7. Essentials ranking (dataset)

`essentials-v1-wikipedia-en.tsv` and later versions list the Wikipedia articles chosen for the Essentials pack, best first:

| Column | Meaning |
|---|---|
| `rank` | position in the final selection |
| `page_id`, `title` | the English Wikipedia article |
| `topic` | one of the survival topics (DESIGN §6) |
| `jev` | usefulness score from an LLM judge, 0–3; 1.0 and above is useful |
| `key` | the sort key used for selection |
| `v0_rank` | rank from the link-graph method (v0) |
| `chunks` | number of search chunks |
| `est_bytes` | estimated pack bytes: chunks × 417 |

The method is described in DESIGN §6. The list can be used as a "vital articles for survival" list, independently of our packs.

---

## Licences

- **Formats** described here: CC0. Use them freely.
- **Content** keeps its own licence, recorded per article (`license`) and per source (`manifest.sources[].licenses`). If you redistribute it, show that licence and the attribution:
  - Wikipedia: CC BY-SA;
  - wikiHow: CC BY-NC-SA, so non-commercial use only;
  - Project Gutenberg: public domain in the USA, plus the Gutenberg licence.
- **Search indexes and embeddings** derived from the content: CC BY-SA.
