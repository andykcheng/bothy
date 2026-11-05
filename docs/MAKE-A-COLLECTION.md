# Make your own collection

You can turn your own documents into a Survivalpedia collection: notes, a community handbook, local first-aid instructions, a farm's procedures. You get the same search (keywords plus meaning), article pages, images, links between articles and browse pages as the built-in collections. It works on a hub and in the app.

A complete example is in [`examples/my-notes/`](../examples/my-notes): three short articles, one image and two folders.

## What you need

- **Python 3.11+** and [uv](https://docs.astral.sh/uv/) for the `ingest/` tools.
- **Rust** (stable) to build `sp` and `sp-server` from `core/`:
  ```sh
  cd core && cargo build --release -p sp-cli -p sp-server
  ```
- **For semantic search** (optional but recommended): the embedding model.
  - `embed.py` downloads `Qwen/Qwen3-Embedding-0.6B` from Hugging Face on first use. It runs on CPU; a GPU is faster.
  - The server needs the GGUF version for queries: [`Qwen3-Embedding-0.6B-Q8_0.gguf`](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF) (640 MB).

Without the model you still get keyword (BM25) search.

## 1. Write your documents

Put them in a folder, with subfolders if you like:

```
my-notes/
  water/
    boiling-water.md
    storing-water.md
    images/boil.png
  power/
    power-cut-checklist.txt
```

- **Supported files:** Markdown (`.md`, `.markdown`), HTML (`.html`, `.htm`) and plain text (`.txt`).
- **One file is one article.** Files and folders starting with `.` are skipped.

**Titles.** The first match wins:
1. `title:` in the front matter;
2. the first `# heading` (Markdown), or `<title>` / `<h1>` (HTML);
3. the first line of a text file;
4. the file name.

**Front matter** (Markdown only, optional) goes at the very top, between `---` lines:

```markdown
---
title: Boiling water to make it safe
license: CC BY 4.0
author: Jane Smith
url: https://example.org/water/boiling
categories: Water, Hygiene
section: Water › Treatment
---
# Boiling water to make it safe

Boiling is the most reliable way to kill germs …
```

| Key | Meaning | Default |
|---|---|---|
| `title` | article title | see above |
| `license` | licence shown on the article | `--license` |
| `author` | shown in the attribution | `--author` |
| `url` | the original online version, if any | `--url` |
| `categories` | comma-separated | the folder names |
| `section` | where it sits on the browse pages, levels separated by `›` | the folder path (`water/` → "Water") |

**Headings.** `#` is the title. Inside an article, use `##`, `###` and `####`. Search results deep-link to the paragraph, and a new `##` section starts a new search passage.

**Images.**
- Link local images with ordinary Markdown or HTML: `![A pot at a rolling boil](images/boil.png)`.
- PNG, JPEG, GIF, WebP and SVG are accepted.
- They are resized to 320 and 640 px wide WebP and stored once, however many articles use them.
- Write alt text: it is shown when images aren't installed.
- Images on the web (`https://…`) are dropped, because the library is offline.

**Links between your articles.** Link to the other file by its relative path, as you would in a Git repository: `[storing water](storing-water.md)` or `[checklist](../power/power-cut-checklist.txt)`. They become links inside the library. Web links stay web links (they only work when there is internet).

**Allowed formatting:**
- Paragraphs, lists, tables, bold and italics, superscript and subscript, quotes, code blocks, figures with captions, links and images.
- Anything else in HTML is unwrapped or removed: scripts, styles, forms, iframes and colours.
- Keep pages simple; they must read well on a small phone, in dark mode, at large text sizes.

**Licence.** Every article needs one. Use `--license` for the default and `license:` in front matter for exceptions.
- If the text is yours, consider an open licence (CC BY 4.0, CC BY-SA 4.0 or CC0) so others can share it.
- If it is someone else's, check you may redistribute it, and keep their licence and attribution.

## 2. Extract

```sh
cd ingest
uv run python docs_extract.py ../examples/my-notes ../build/my-notes \
    --source my-notes --title "Household emergency notes" \
    --license "CC BY 4.0" --author "Jane Smith"
```

```
power/power-cut-checklist.txt: Power cut checklist (4 blocks)
water/boiling-water.md: Boiling water to make it safe (10 blocks)
water/storing-water.md: Storing water (3 blocks)
3 articles, 4 chunks, 1 images -> ../build/my-notes
```

`--source` is a short id: lower-case letters, digits and `-`. The output directory is an **extract**: plain JSON Lines that you can inspect or edit (format in [DATA-FORMATS.md](DATA-FORMATS.md#1-extract-an-ingest-output-directory)). If you already have content in another form (a database, a wiki export), you can write this format yourself and skip this step.

## 3. Embed (for semantic search)

```sh
uv run python embed.py ../build/my-notes
```

This writes `../build/my-notes/emb/`. A few hundred articles take a minute or two on a CPU. When you change some documents and run the whole process again, pass `--reuse <old extract>/emb` and unchanged passages are not embedded again.

Skip this step if you only want keyword search, and pass `--no-vec` to `sp build`.

## 4. Build the pack

```sh
cd ../core
target/release/sp build ../build/my-notes --out ../packs/my-notes \
    --id my-notes --title "Household emergency notes" \
    --description "Water, power cuts" --kind guides
target/release/sp browse ../packs/my-notes
```

| Option | Meaning |
|---|---|
| `--id` | stable collection id, used in URLs and download paths; keep it the same across versions |
| `--title`, `--description` | shown in the app and on the hub |
| `--kind` | `articles` (an encyclopedia, sorted A–Z) or `guides` (read in order) |

`sp browse` writes the browse sections from each article's `section` or folders. Add `--pack-order` to keep files in the order they were read (alphabetical by path) instead of A–Z by title. Numbering files (`01-start.md`, `02-water.md`) gives you a reading order.

Several extracts can go into one pack: `sp build extract-a extract-b --out …`.

Try it from the command line:

```sh
target/release/sp search ../packs/my-notes "how long do I boil water" --model Qwen3-Embedding-0.6B-Q8_0.gguf
target/release/sp get ../packs/my-notes "Storing water"
```

## 5. Use it

**On a computer or hub.** Serve it on its own:

```sh
target/release/sp-server ../packs/my-notes --model Qwen3-Embedding-0.6B-Q8_0.gguf --listen 0.0.0.0:8080
```

Open `http://<this computer>:8080/` from any browser on the network.

To serve it as part of a library, put it in the library directory next to the other collections:

```
library/
  models/Qwen3-Embedding-0.6B-Q8_0.gguf
  essentials-wikipedia/
  my-notes/            <- your pack
```

```sh
sp-server --library library --downloads
```

All collections are searched together. `/api/search?c=my-notes` searches only yours.

**On phones.** Start the hub with `--downloads` and your collection appears in the app's Content tab, next to the hub's other collections, once the app is pointed at the hub. Phones that download it can then share it on to other phones (see the README's "Sharing nearby").

## 6. Update it

Edit your documents, then repeat steps 2 to 4 into new directories, keeping the same `--source` and `--id`. Article URLs are `/doc/<id>/<number>`, and the numbers can change between builds. Links between your articles use paths, so they keep working.

## Troubleshooting

| Message | Cause |
|---|---|
| `skipped (no text): x.md` | The file had no text after cleaning, e.g. only an image or only HTML that isn't allowed. |
| `<out> is not empty` | Extract and pack directories must be new or empty. |
| `no embeddings (run ingest/embed.py, or pass --no-vec)` | `sp build` found no `emb/` in the extract. |
| A link opens a "not found" page | The target file wasn't found. Check the relative path, and that the target is one of the input files. |
| An image is missing | Only local files inside the input folder are used. Check the path and the file type. |
