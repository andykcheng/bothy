"""Your own documents -> a Bothy extract (docs/MAKE-A-COLLECTION.md), ready for embed.py and `sp build`.

    uv run python docs_extract.py <input dir> <out dir> --source my-notes [--license "CC BY 4.0"]
        [--title "My notes"] [--author "…"] [--url "https://…"]

Reads every Markdown (.md, .markdown), HTML (.html, .htm) and plain-text (.txt) file under <input dir>, with its
subfolders. Each file becomes one article. Writes the same files as zim_extract.py: source.json, docs.jsonl,
chunks.jsonl, redirects.jsonl, images.jsonl and images/.

Per file:
- Title: Markdown front matter `title:`, else the first `# heading` (Markdown) or <title>/<h1> (HTML), else the
  first line of a text file, else the file name.
- Optional front matter at the top of a Markdown file, between `---` lines, `key: value` per line:
  title, license, url, author, categories (comma-separated), section (e.g. `Water › Boiling`).
- Images: local images the file links to (relative paths, PNG/JPEG/GIF/WebP/SVG) are converted to WebP at 320 and
  640 px wide and stored once each, however many articles use them. Remote images are left out (the library is
  offline).
- Links to your other files (`[see](../water/boiling.md)`) become links between articles. Web links stay links.
- Section for the browse pages: front matter `section:`, else the folder path (`water/boiling.md` -> "Water").

Headings: `#` is the title; use `##`-`####` inside an article. Allowed HTML is the pack's whitelist (DESIGN §5:
p, h2-h4, lists, tables, figures, images, links, b/i, sup/sub, blockquote, pre/code); anything else is unwrapped
or dropped (scripts, styles, forms).
"""
from __future__ import annotations

import argparse
import hashlib
import html as htmlmod
import json
import re
import shutil
from pathlib import Path

import markdown
from selectolax.lexbor import LexborHTMLParser as HTMLParser

import zim_extract
from zim_extract import Chunker, Cleaner, encode_image, norm_html, norm_space

TEXT_EXT = {".md": "md", ".markdown": "md", ".html": "html", ".htm": "html", ".txt": "txt"}
IMAGE_MIME = {".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".gif": "image/gif",
              ".webp": "image/webp", ".svg": "image/svg+xml"}
FRONT = re.compile(r"\A---\s*\n(.*?)\n---\s*\n", re.S)
LINK_EXT = re.compile(r'href="wiki:([^"#]*?)\.(?:md|markdown|html?|txt)(#[^"]*)?"')

# Your own documents keep every section: zim_extract drops "References", "Notes", "See also" and the like from
# Wikipedia articles, which would remove a section of yours with one of those names.
zim_extract.DROP_SECTIONS = set()


def front_matter(text: str) -> tuple[dict, str]:
    m = FRONT.match(text)
    if not m:
        return {}, text
    meta = {}
    for line in m.group(1).splitlines():
        k, sep, v = line.partition(":")
        if sep:
            meta[k.strip().lower()] = v.strip().strip('"').strip("'")
    return meta, text[m.end():]


def to_html(kind: str, text: str) -> str:
    if kind == "md":
        return markdown.markdown(text, extensions=["tables", "fenced_code", "sane_lists"])
    if kind == "txt":
        paras = [p.strip() for p in re.split(r"\n\s*\n", text) if p.strip()]
        return "".join(f"<p>{htmlmod.escape(p).replace(chr(10), '<br>')}</p>" for p in paras)
    return text


def pretty(name: str) -> str:
    return name.replace("_", " ").replace("-", " ").strip().capitalize()


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("input", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--source", required=True, help="short id, e.g. my-notes (letters, digits, '-')")
    ap.add_argument("--license", default="All rights reserved", help="default licence for every article")
    ap.add_argument("--title", default="", help="collection title for the pack manifest")
    ap.add_argument("--author", default="")
    ap.add_argument("--url", default="", help="default source URL for every article")
    a = ap.parse_args()
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]*", a.source):
        raise SystemExit("--source: lower-case letters, digits and '-' only")
    src = a.input.resolve()
    out = a.out
    if out.exists() and any(out.iterdir()):
        raise SystemExit(f"{out} is not empty")
    (out / "images").mkdir(parents=True, exist_ok=True)

    chunker = Chunker()
    docs, chunks, redirects, images = [], [], [], {}
    files = sorted(p for p in src.rglob("*") if p.is_file() and p.suffix.lower() in TEXT_EXT
                   and not any(part.startswith(".") for part in p.relative_to(src).parts))
    for f in files:
        rel = f.relative_to(src).as_posix()
        kind = TEXT_EXT[f.suffix.lower()]
        raw = f.read_text(encoding="utf-8", errors="replace")
        meta, body = front_matter(raw) if kind == "md" else ({}, raw)
        if kind == "txt":   # a short first line is the title, not the first paragraph
            first, _, rest = body.strip().partition("\n")
            if first.strip() and len(first) <= 120 and rest.strip():
                meta["title"], body = norm_space(first), rest
        tree = HTMLParser(to_html(kind, body))

        title = meta.get("title", "")
        h1 = tree.css_first("h1")
        if not title and kind == "html":
            t = tree.css_first("title")
            title = norm_space(t.text()) if t is not None else ""
        if not title and h1 is not None:
            title = norm_space(h1.text())
        title = title or pretty(f.stem)
        if h1 is not None and norm_space(h1.text()) == title:
            h1.decompose()   # the title is shown above the article already

        path = rel.rsplit(".", 1)[0]
        cl = Cleaner(rel)
        root = tree.css_first("body") or tree.root
        cl.walk(root)
        html = norm_html("".join(cl.out))
        if not cl.blocks:
            print(f"skipped (no text): {rel}")
            continue

        # images: local files only, stored once by content
        def image(m: re.Match) -> str:
            p = (src / htmlmod.unescape(m.group(1))).resolve()
            mime = IMAGE_MIME.get(p.suffix.lower())
            if not mime or not p.is_file() or src not in p.parents:
                return ""
            data = p.read_bytes()
            key = hashlib.sha256(data).hexdigest()[:24]
            if key not in images:
                info, variants = encode_image(data, mime)
                d = out / "images" / key[:2]
                d.mkdir(exist_ok=True)
                for v, b in variants.items():
                    (d / (f"{key}.svg" if v == "svg" else f"{key}.{v}.webp")).write_bytes(b)
                images[key] = {"key": key, "paths": [p.relative_to(src).as_posix()], "src_bytes": len(data), **info}
            return f'<img src="img:{key}"{m.group(2)}>'

        html = re.sub(r'<img src="sp-img:([^"]*)"([^>]*)>', image, html)
        html = re.sub(r"<figure>\s*</figure>", "", html)
        html = LINK_EXT.sub(lambda m: f'href="wiki:{m.group(1)}{m.group(2) or ""}"', html)

        doc_id = f"{a.source}/{path}"
        folders = rel.split("/")[:-1]
        section = meta.get("section", "")
        browse = [s.strip() for s in section.split("›")] if section else [pretty(x) for x in folders]
        cats = [c.strip() for c in meta.get("categories", "").split(",") if c.strip()] or [pretty(x) for x in folders]
        docs.append({
            "doc_id": doc_id, "source": a.source, "lang": "en", "title": title, "path": path,
            "url": meta.get("url", a.url) or None, "page_id": len(docs) + 1, "rev_id": None,
            "rev_date": None, "license": meta.get("license", a.license), "author": meta.get("author", a.author) or None,
            "categories": cats, "browse": [b for b in browse if b], "html": html, "blocks": cl.blocks,
            "images": [m for m in re.findall(r'img:([0-9a-f]{24})', html)],
        })
        chunks += chunker.chunk(doc_id, title, cl.blocks)
        # links to "folder/file.md" find the article through its path
        if path.lower() != title.lower():
            redirects.append({"from_path": path, "from_title": path, "to_path": path})
        print(f"{rel}: {title} ({len(cl.blocks)} blocks)")

    def jsonl(name, rows):
        with open(out / name, "w", encoding="utf-8") as fh:
            for r in rows:
                fh.write(json.dumps(r, ensure_ascii=False) + "\n")

    jsonl("docs.jsonl", docs)
    jsonl("chunks.jsonl", chunks)
    jsonl("redirects.jsonl", redirects)
    jsonl("images.jsonl", images.values())
    if not images:
        shutil.rmtree(out / "images")
    source = {
        "source": a.source, "lang": "en", "site": "docs", "title": a.title or pretty(a.source),
        "author": a.author or None, "url": a.url or None, "license": a.license,
        "docs": len(docs), "chunks": len(chunks), "redirects": len(redirects), "images_unique": len(images),
        "chunking": {"tokenizer": zim_extract.TOKENIZER, "target_tokens": zim_extract.CHUNK_TOKENS,
                     "max_tokens": zim_extract.CHUNK_MAX_TOKENS},
    }
    (out / "source.json").write_text(json.dumps(source, indent=1, ensure_ascii=False))
    print(f"{len(docs)} articles, {len(chunks)} chunks, {len(images)} images -> {out}")


if __name__ == "__main__":
    main()
