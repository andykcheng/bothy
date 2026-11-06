# Concepts

The ideas behind Bothy, and the words the rest of the documentation uses.

## Why it exists

When the internet is gone, after a disaster or during a long power cut, people still need to know how to purify water, treat a wound, keep food safe, fix a pump or find the nearest hospital. Bothy is a library of that knowledge that works with **no network at all**, on the phones people already have.

The design follows from that goal:
- **Offline after setup.** Everything a feature needs (text, images, indexes, models, maps) is downloaded beforehand. Nothing calls the internet.
- **Curated, not complete.** All of Wikipedia is about 100 GB with images; most people won't download that. The main pack, **Essentials**, is about 650 MB and was chosen for usefulness in a crisis.
- **Phones first.** It has to work on an ordinary Android phone: from slow storage, with little memory, and on a battery. A hub serves everyone else, including iPhones, through the browser.
- **Share once, spread locally.** One phone or hub with the content can pass it to every phone nearby over Wi-Fi, with no internet.
- **Free.** No accounts, no ads, no charges, no tracking.
- **Open data.** The formats are documented (DATA-FORMATS.md) so the data can outlive this project.

## Words

**Source.** One origin of content, in one language: Wikipedia, wikiHow, the survival guides, or your own documents. Each source has a licence, and every article keeps its own licence and attribution.

**Extract.** A source after cleaning, as plain JSON Lines files: articles with simplified HTML, their text split into blocks and chunks, and images as WebP. It is the easiest format for other projects to use, and the input to the pack builder.

**Article (doc).** One page: a Wikipedia article, a wikiHow guide, a chapter of a manual, one of your Markdown files.

**Block.** One paragraph, heading, list item or table of an article, with a stable id (`p12`). Search results point to blocks, so a hit opens at the right paragraph.

**Chunk (passage).** A run of blocks of about 256 tokens: the unit search works on. Each chunk starts with its article title and section, so it makes sense on its own.

**Pack.** One collection in its shipped form: one directory with compressed articles, a title index, images, a keyword index and optional vector indexes. Every file is checksummed in `manifest.json`. Packs are memory-mapped and read in small pieces, so a phone never loads one whole.

**Collection.** A pack as people see it: "Wikipedia Essentials", "wikiHow essentials", "Survival guides", or your own. A library is any set of collections, including none. All installed collections are searched together.

**Component.** One download in the catalog. A collection is split into components so people can choose:

| Component | Content | Needs |
|---|---|---|
| `col:<id>` | articles, titles and keyword search | nothing |
| `col:<id>:images` | the collection's images | `col:<id>` |
| `col:<id>:vec:<name>` | semantic search over part of the collection | `col:<id>` and `model` |
| `model` | the embedding model | nothing |

Maps, games, books and the answering model are components too.

**Catalog.** The list of components a hub or a sharing phone offers (`/api/catalog`), with every file's size and hash. Phones download from it and check every file.

**Hub.** Any computer running `sp-server`: a mini PC, a Raspberry Pi or a laptop, usually with its own Wi-Fi. It serves the library, maps, books, games and chat to any browser, and offers its content to phones.

**Sharing nearby.** A phone running the app becomes a small hub. It starts a Wi-Fi network (or uses its hotspot) and shows two QR codes: one to join the network, one to open the page. People nearby use the library in their browser. Phones that have the app can copy content from it, and then pass it on themselves.

## Search

**Keyword search (BM25).** Finds passages that contain the query's words (with stemming: "boiling" matches "boil"). It is exact, fast and needs no model. It misses passages that say the same thing in other words.

**Semantic search (dense).** Finds passages with the same meaning. A small embedding model, Qwen3-Embedding-0.6B, turns each passage into a vector at build time and each query into a vector at search time. The nearest passage vectors are the results. For example, "water tastes like metal from the tap" finds articles on rusty water and testing water quality. Semantic search needs the model (~640 MB) on the device.

**Hybrid search.** Both methods run, and their rankings are merged with **reciprocal rank fusion** (RRF): a passage ranked high by either method rises. Each article appears once, represented by its best passage.

**Pinned model.** Query vectors and passage vectors are only comparable if they come from the same model, made the same way. The model, its revision and the query prefix are therefore fixed and recorded in every pack. Changing any of them means embedding every passage again.

**Matryoshka dims, IVF, binary codes.** These keep semantic search small and fast:
- **Matryoshka dims.** The model's vectors can be cut to their first 256 or 128 numbers and still work. That makes them 4–8 times smaller than full vectors.
- **IVF.** Vectors are grouped into clusters; a query looks only in the nearest few clusters.
- **Binary codes, then int8.** Within those clusters, it compares 1-bit codes first, then rescores the best candidates with 8-bit vectors.

**Core and rest.** A collection's vectors are split into the most-read articles (`core`) and everything else (`rest`), so a phone short of space can install semantic search for just the important part.

## Choosing what goes in: Essentials

Essentials is the main pack: about 130,000 Wikipedia articles (600 MB) plus about 11,000 wikiHow guides and the survival manuals. Picking the Wikipedia articles is a ranking problem over 6.9 million candidates. It was done in stages (DESIGN §6):

1. **Link graph (v0).**
   - About 650 hand-picked hub articles, plus WikiProject ratings, Vital Articles and category rules, form seeds for 13 survival topics.
   - Personalised PageRank spreads relevance from the seeds over Wikipedia's 700 million links.
   - The score is adjusted for popularity and size, with penalties for biographies, sport and entertainment.
2. **LLM judge (v1).**
   - An LLM read the lead of each of the top 400,000 candidates and scored how useful it would be with no internet, from 0 to 3.
   - That score was combined with the v0 rank.
3. **Classifier (v2).**
   - A classifier on the lead's embedding predicts the judge's score for all 6.9 million articles.
   - The judge then checks the most promising articles beyond the first 400,000.

The test is **coverage**: 931 realistic questions (how-to, symptoms in everyday words, what-is) were written without looking at the selection. Coverage is the share of questions whose answering article is in the pack. v1 covers 98.0% of them.

The resulting list is a dataset in its own right: a ranked "vital articles for survival" list that other projects can use or correct.

## Answering questions (RAG)

On capable phones and hubs, `/ask` writes a short answer from the library:
1. It searches.
2. It hands the best eight passages to a small local model, with the instruction to answer only from them.

The passages are always shown as sources. Doses, ratios and electrical values should be read from the source, not from the model. Search works without this feature, and the model is an optional download.

## Licences and attribution

Every article records its source, original URL, revision date and licence, and every article page shows them.
- **Wikipedia** is CC BY-SA.
- **wikiHow** is CC BY-NC-SA. This is fine for a free app, but not for commercial use.
- **Project Gutenberg** books are public domain in the USA. They are filtered by author death year, so the selection is also public domain in life+70 countries.
- **OpenStreetMap** data is ODbL.

If you reuse the data, keep the licence and attribution with it.
