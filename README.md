# Bothy

*Formerly Survivalpedia. Website: [pocketbothy.com](https://pocketbothy.com). A bothy is a free shelter in the hills that is never locked; this app is a free shelter of knowledge you carry.*

**An offline library of practical knowledge for when the internet is gone.** It holds Wikipedia articles chosen for survival, wikiHow guides, survival and first-aid manuals, 54,000 public-domain books, offline maps and games. You can search all of it by keyword or by meaning on an ordinary Android phone, or from any browser on a small Wi-Fi hub, with no network at all.

It is free and always will be: no accounts, no ads, no tracking.

## What it does

- **Search that understands questions.** Keyword search (BM25) and semantic search (a small embedding model running on the phone) work together. "water tastes like metal from the tap" finds the articles on rusty water. Results open at the paragraph that answers.
- **First aid cards.** 15 step-by-step cards (CPR, choking, bleeding, burns, stroke, poisoning, …) built into the app, shown above search results when a query sounds like an emergency.
- **Wikipedia Essentials.** About 131,000 articles chosen from all 7.2 million English Wikipedia articles for usefulness in a crisis (medicine, water, food, shelter, energy, repair, plants and animals, navigation, …). A large language model scored 532,000 articles, and a classifier trained on its scores rated the rest. The selection answers 98.7% of 931 test questions.
- **Practical collections.** wikiHow guides (11,000), iFixit repair guides (56,000), Stack Exchange Q&A from 15 practical sites (227,000), Appropedia (13,900) and survival manuals (US Army FM 21-76, FEMA, WHO-ICRC Basic Emergency Care, UK Be Ready). The text of everything is about 1.2 GB; images are optional.
- **Answers from the library** (optional). A small local model writes a short answer from the best passages and always shows its sources. It runs on the phone's CPU.
- **Books.** Project Gutenberg's English public-domain books, with a built-in reader. Import your own EPUBs too.
- **Offline maps.** OpenStreetMap vector maps by region, with place search.
- **Notes and checklists** for a blackout: emergency numbers (tap to call), a power-cut checklist, a grab-bag list.
- **Games** for long evenings without power: chess, draughts, sudoku, solitaire, two RPGs, physics puzzles and more.
- **Share nearby.** A phone becomes a hub: it starts a Wi-Fi network, and people nearby scan a QR code and use the library, maps, books and chat in their browser (iPhones too). Phones with the app copy content from it, and then pass it on. Each copied file is checked against the original's hash.
- **Make your own collection** from Markdown, HTML or text files: a community handbook, local procedures, family notes. It gets the same search, browse pages and sharing as the built-in collections.

## Quick start

### Run a hub with Docker (the easy way)

Step-by-step guide for Windows, Mac, Linux, Raspberry Pi and NAS: [pocketbothy.com/hub](https://www.pocketbothy.com/hub).

```sh
docker run -d --name bothy --restart unless-stopped -p 8080:8080 -p 8443:8443 -v bothy-data:/data -e BOTHY_PRESET=standard andykcheng/bothy
```

The image (amd64 and arm64, so a Raspberry Pi works) holds only the `bothy-hub` program. On first start it downloads the library into the `bothy-data` volume: `small` 2.6 GB, `standard` 4.4 GB or `everything` 10 GB, resumable and checked file by file. Street maps are added country by country (`bothy-hub maps add uk`). `docker exec bothy bothy-hub check` confirms it's ready. See `hub/Dockerfile`.

`bothy-hub` also runs without Docker. Build it with `cargo build --release -p sp-server --bin bothy-hub` in `core/`, then run `bothy-hub get` and `bothy-hub start`. `--from http://<hub>:8080` copies from any running hub instead of the download server.

### Build and run the server yourself (Linux, macOS, Windows)

You need Rust (stable), CMake and Clang.

```sh
cd core
cargo build --release -p sp-server -p sp-cli
```

Try it on the small example collection. That needs Python 3.11+ and [uv](https://docs.astral.sh/uv/) for the extract step; see [docs/MAKE-A-COLLECTION.md](docs/MAKE-A-COLLECTION.md).

```sh
target/release/sp-server path/to/pack --listen 0.0.0.0:8080
```

Serve a whole library:

```sh
target/release/sp-server --library library/ --model library/models/Qwen3-Embedding-0.6B-Q8_0.gguf \
    --downloads --maps maps/ --games ../app/assets/games --books shelf/ --chat chat/ --https 0.0.0.0:8443
```

`library/` holds one directory per collection, plus `models/`. Open `http://<hub address>:8080/` from any device on the network. All the options are in [docs/HTTP-API.md](docs/HTTP-API.md).

### Android app

- **Build it:** the app is Flutter with the Rust core built for `arm64-v8a` (see `.github/workflows/android.yml`).
- **Install it:** download the APK from a hub's home page ("Get the app"). A phone that has it can also share it.
- **Get content:** in the app, open **Content** and point it at a hub or at a nearby sharing phone, then choose what to download.
- **Phone needed:** Android 8 or newer, with a 2022-or-newer CPU (Snapdragon 8 Gen 1 class) for the on-device models.

### Command line

```sh
sp search <pack> "how to purify water" [--model Qwen3-Embedding-0.6B-Q8_0.gguf]
sp get <pack> "Water purification"
sp title <pack> "water pur"
```

## Use the data in your own project

The formats are documented and open (CC0):
- **Extracts** are plain JSON Lines: cleaned HTML, text blocks, ~256-token passages and image lists for every article. They are the easiest way in.
- **Packs** are the compact shipped form. `tools/sp_pack.py` reads them in plain Python with one dependency:

  ```sh
  pip install zstandard
  python tools/sp_pack.py <pack> info
  python tools/sp_pack.py <pack> dump articles.jsonl   # every article: title, url, licence, html, …
  ```
- **Embeddings** are Qwen3-Embedding-0.6B vectors for every passage, as NumPy arrays.
- **The Essentials ranking** is a TSV list of Wikipedia articles ranked for survival usefulness, usable as a dataset on its own.

Details: [docs/DATA-FORMATS.md](docs/DATA-FORMATS.md). Keep each article's licence and attribution when you reuse content.

## Documentation

| Document | What's in it |
|---|---|
| [docs/CONCEPTS.md](docs/CONCEPTS.md) | why it exists, the vocabulary, how search and selection work |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | components, repository layout, build pipeline, the app |
| [docs/DATA-FORMATS.md](docs/DATA-FORMATS.md) | extract, embeddings, pack (byte level), catalog, book shelf, games, rankings |
| [docs/MAKE-A-COLLECTION.md](docs/MAKE-A-COLLECTION.md) | turn your own documents into a collection |
| [docs/HTTP-API.md](docs/HTTP-API.md) | server options, pages and JSON API |
| [docs/DESIGN.md](docs/DESIGN.md) | every design decision, with reasons and measurements |
| [docs/MEASUREMENTS.md](docs/MEASUREMENTS.md) | sizes, speeds and search quality measurements |

## Repository

```
core/      Rust: pack format, search, embedding, local answers, maps, server, app bindings, CLI
ingest/    Python: sources → extracts → embeddings → article selection
app/       Flutter Android app
tools/     sp_pack.py, a reference pack reader
scripts/   source downloads, map assets, checks
examples/  my-notes, a tiny collection made from Markdown and text
eval/      search test questions
docs/      documentation
```

## Licence

The code is licensed under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0). If you run a modified hub or server for others, you must offer them its source.

Bundled third-party parts keep their own licences, included with them: the Inter font (SIL OFL 1.1), the ebook reader (epub.js, JSZip), the map library and third-party games.

## Content licences

Every article carries its own licence and attribution, and shows them on its page.

| Content | Licence |
|---|---|
| Wikipedia | CC BY-SA 3.0/4.0 |
| wikiHow | CC BY-NC-SA 3.0 (non-commercial) |
| iFixit | CC BY-NC-SA 3.0 (non-commercial) |
| Stack Exchange | CC BY-SA 2.5/3.0/4.0 by post date, authors named on each page |
| Appropedia | CC BY-SA 4.0 |
| WHO-ICRC Basic Emergency Care | CC BY-NC-SA 3.0 IGO |
| US Army FM 21-76, FEMA | US government work, public domain |
| UK Be Ready | Open Government Licence v3.0 |
| Project Gutenberg books | public domain, with the Project Gutenberg licence inside each book |
| OpenStreetMap map data | ODbL 1.0, © OpenStreetMap contributors |
| Qwen3-Embedding-0.6B | Apache-2.0 |
| LFM2-1.2B-RAG (optional) | LFM Open License v1.0 |

Third-party games keep their own licences (MIT, GPL-2.0-or-later), which are included with them.
