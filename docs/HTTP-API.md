# HTTP API

`sp-server` is the hub. The phone app runs the same server on 127.0.0.1, and on the Wi-Fi when sharing. It serves plain HTML pages that work without JavaScript, plus a JSON API. Everything is unauthenticated and meant for a local network. There is no internet access and no tracking.

```sh
sp-server [PACK ...] [--library DIR] [--listen 0.0.0.0:8080] [--model EMBED.gguf]
          [--maps DIR] [--games DIR] [--books DIR] [--llm GGUF|DIR] [--chat DIR] [--https 0.0.0.0:8443]
          [--downloads] [--apk FILE]
```

| Option | What it adds |
|---|---|
| `PACK ...` / `--library DIR` | Collections: single packs, or every subdirectory of DIR that has a `manifest.json`. `DIR/models/*.gguf` is the embedding model. |
| `--model` | Query embedding for semantic search. Without it, search is keyword only. |
| `--maps DIR` | `/map`, tiles and place search, from `DIR/*.pmtiles` plus `DIR/assets/` |
| `--games DIR` | `/games` |
| `--books DIR` | `/books`, from a book shelf (DATA-FORMATS §5) |
| `--llm` | `/ask`: answers written by a local model from library passages |
| `--chat DIR` | `/chat`, with its messages stored in DIR |
| `--https ADDR` | A second listener with a self-signed certificate, which chat's microphone and GPS need. Requires `--chat`. |
| `--downloads` | `/api/catalog` and `/download/…`, so phones can copy content from the hub |
| `--apk FILE` | `/app.apk`, so visitors can install the Android app |
| `--export-catalog DIR` | Write a static copy of the download API (`DIR/api/catalog`, `DIR/download/…` as symlinks) for a plain file host, then exit |

Collection ids appear in URLs as `{c}`. Doc numbers `{n}` come from search results. They are stable for a given pack, but can change when a pack is rebuilt.

## Pages (HTML)

| Path | Page |
|---|---|
| `/` | home: search box, collections, links to the other features |
| `/search?q=&c=&mode=` | search results |
| `/doc/{c}/{n}` | an article, with its attribution and licence footer; `#p12` jumps to a paragraph |
| `/link/{c}/{source}/{path}` | follows a `wiki:` link; redirects (303) to the article, or shows "not found" |
| `/browse/{c}` | a collection's sections, or its articles A–Z |
| `/img/{key}?v=640` | an image (WebP or SVG); the 320 px version unless `v=640` |
| `/ask` | ask a question, answered by the local model (needs `--llm`) |
| `/emergency`, `/emergency?q=…` | first aid cards (always served); `q` jumps to the matching card |
| `/map`, `/map?q=…`, `/map?pin=lat,lon&label=…` | offline map |
| `/games`, `/game/{id}/` | games |
| `/books`, `/books/c/{i}`, `/read/…` | books and the reader |
| `/chat` | chat for people on the same Wi-Fi |
| `/app.apk` | the Android app |

## Search

### `GET /api/search`

| Parameter | |
|---|---|
| `q` | the query |
| `mode` | `hybrid` (default), `bm25` or `dense`; falls back to `bm25` when the server has no embedding model |
| `c` | limit to one collection id |
| `k` | number of hits, 1–100 (default 20) |

```json
{
 "query": "freezer food blackout", "mode": "hybrid", "embed_ms": 61.2, "search_ms": 3.4,
 "hits": [
  {"rank": 1, "collection": "my-notes", "doc": 0, "title": "Power cut checklist", "source": "my-notes-en",
   "lang": "en", "block": "p0", "snippet": "Power cut checklist Switch off appliances …",
   "bm25_rank": 1, "dense_rank": 2}
 ]
}
```

Hits are passages: their best articles first, one hit per article. `block` is the paragraph to open (`/doc/{collection}/{doc}#{block}`). `bm25_rank` and `dense_rank` show where each method ranked the hit (`null` when it didn't). The two rankings are fused with reciprocal rank fusion.

### `GET /api/complete?q=&k=`

Title autocomplete (prefix match), at most 50 results:

```json
[{"key": "storing water", "collection": "my-notes", "doc": 2, "title": "Storing water", "source": "my-notes-en"}]
```

### `GET /api/doc/{c}/{n}`

One article as JSON. `record` is the doc record (DATA-FORMATS §3): `html`, `license`, `url`, `categories` and the rest.

```json
{"collection": "my-notes", "doc": 2, "title": "Storing water", "lang": "en", "source": "my-notes-en", "record": {…}}
```

### `GET /api/info`

Lists the collections (id, title, description, kind, counts, sources, vector indexes) and what else the server offers:

```json
{"collections": [...], "docs": 240000, "dense": true, "llm": "LFM2-1.2B-RAG-Q4_0", "maps": true, "games": true, ...}
```

### `POST /api/embed`

Vectors from the library's embedding model, for short texts. The app's Notes use it for search by meaning.

Body: `{"texts": ["…", …], "query": false}`, at most 32 texts. `query: true` puts the pinned query instruction in front, as library search does; documents get no prefix.

Returns `{"dims": 256, "vectors": [[…], …]}`: the first 256 dimensions, L2-normalised, as the packs store them. Returns 503 when no model is loaded (none installed, or no collection has vectors).

## Ask (local RAG)

`GET /api/ask?q=&k=8`, or `POST /api/ask` with `{"q": "...", "k": 8, "history": [{"q": "...", "a": "..."}]}` for a follow-up question.

The reply is a stream of NDJSON, one event per line:

```
{"type":"card","id":"poisoning","title":"Poisoning","first":"…"}   a first aid card that fits the question (only then; comes first)
{"type":"status","text":"Loading the model…"}
{"type":"sources","sources":[{"n":1,"doc":…,"title":…,"block":…,"snippet":…}]}
{"type":"text","text":"Boil it for "}       pieces of the answer, in order
{"type":"reset"}                            discard the text so far
{"type":"done","prompt_tokens":…,"gen_tokens":…,"gen_ms":…}
{"type":"error","error":"…"}
```

The answer is written only from the passages it lists. Always show those sources next to the answer.

## Maps

| Request | Returns |
|---|---|
| `GET /api/maps` | `[{"id": "uk", "title": "United Kingdom", "bounds": [w, s, e, n], "min_zoom", "max_zoom", "center", "bytes", "places"}]` |
| `GET /api/places?q=&lon=&lat=&k=` | place search; `lon`/`lat` favour nearby results: `[{"name", "names", "class", "kind", "context", "lat", "lon", "score", "region"}]` |
| `GET /tiles/{z}/{x}/{y}` | Mapbox Vector Tile ([Protomaps basemap](https://docs.protomaps.com/basemaps/layers) schema); each tile comes from the smallest region that has it |
| `GET /maps/assets/…` | MapLibre GL JS, style, glyphs and sprites |

## Books

| Request | Returns |
|---|---|
| `GET /api/books` | the shelf (DATA-FORMATS §5) |
| `GET /api/books/search?q=&limit=` | `{"results": [{"id", "score"}]}`; searches title, author, subjects and description |
| `GET /api/books/{id}` | `{"id", "summary", "subjects"}` |
| `GET /books/cover/{id}` | the cover JPEG |
| `GET /read/book/{id}.epub` | the EPUB; `{id}i.epub` is the illustrated edition |

## Downloads (`--downloads`, or a sharing phone)

| Request | Returns |
|---|---|
| `GET /api/catalog` | everything on offer, as components with files, sizes and BLAKE3 hashes (DATA-FORMATS §4) |
| `GET /download/{path}` | a file from the catalog; supports HTTP range requests, so interrupted downloads can resume |

A client should check each file's size and BLAKE3 hash against the catalog after downloading it.

## Chat (`--chat`)

There are no accounts. A browser makes a random `id` and a secret `token`, keeps both in localStorage, and sends them with every request. The first `join` claims the id, and the server stores only the token's hash.

| Request | Body / parameters | Returns |
|---|---|---|
| `POST /api/chat/join` | `{"id", "token", "name"}` (also used to rename) | `{"people": [...]}` |
| `POST /api/chat/send` | `{"id", "token", "text", "to"?, "lat"?, "lon"?}` | `{"seq": N}` |
| `POST /api/chat/upload?id=&token=&kind=image\|audio&to=&text=` | the file as the request body | `{"seq": N}` |
| `GET /api/chat/poll?id=&token=&since=&wait=1` | | `{"seq", "messages": [...], "people": [...]}` |
| `GET /api/chat/file/{name}?id=&token=` | | a photo or voice message |

- **Messages:** `{seq, ts, from, name, to, text, kind, file, lat, lon}`.
- **Private messages:** set `to` to a person's id.
- **Places:** set `lat`/`lon`.
- **Polling:** with `wait=1`, a poll waits up to 25 s for a new message (long polling), so no WebSocket is needed.
- **Limits:**
  - text up to 2,000 characters, and at most 10 messages per 10 seconds;
  - photos up to 4 MB and voice messages up to 3 MB, with the type checked from the file's bytes;
  - files take at most 256 MB in all; past that, the oldest are removed.

Private messages are stored on the server like everything else, so whoever runs the server can read them.
