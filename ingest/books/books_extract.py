"""Project Gutenberg books -> the extract format of zim_extract.py (source.json, docs.jsonl, chunks.jsonl,
redirects.jsonl, images.jsonl, images/), so `sp build` takes them like any source.

  uv run python books/books_extract.py <books dir> <catalog.jsonl> <ids.txt> <out dir> --source gutenberg-practical \
      --title "Practical books" [--images]

<books dir>/<id>/ holds <id>-h.htm (and images/ for --images), from books/gutenberg_fetch.py.
Each book becomes a contents document (title, author, summary, chapter list) plus one document per chapter, split
on the highest heading level that occurs at least twice. Chapters link to each other and to the contents page with
in-library links (`wiki:pg<id>/<n>`, resolved through redirects), so the server's article page doubles as a reader.

Rights: Gutenberg texts are public domain in the USA; the app is used worldwide. A book is dropped if any author
died after DEATH_CUTOFF (still in copyright under life+70 rules) or Gutenberg marks it "Copyrighted" (distributed with
permission). Books by anonymous, corporate or government authors, or by authors whose dates are unknown, are kept
unless their text carries a copyright notice dated after DEATH_CUTOFF. The Project Gutenberg header, footer and licence (and so the trademark) are removed, as
its licence allows for public-domain texts; the source is still credited per document.
"""
import argparse, hashlib, json, re, sys
from pathlib import Path

from selectolax.lexbor import LexborHTMLParser as HTMLParser

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import zim_extract  # noqa: E402
from zim_extract import Chunker, Cleaner, encode_image, esc, esc_attr, norm_html, norm_space  # noqa: E402

zim_extract.DROP_SECTIONS = set()   # "Notes", "Bibliography"… are part of a book

DEATH_CUTOFF = 1955
SKIP_PART = re.compile(r"^(table of )?contents\.?$|^list of (illustrations|plates|figures)|^illustrations\.?$|^transcriber|"
                       r"^index\.?$|^the end\.?$|^footnotes?\.?$", re.I)
MAX_PART = 150_000      # characters of text; longer parts are cut at block boundaries
MIN_PART = 800          # shorter parts (title pages, epigraphs) are merged into the next


NOT_A_PERSON = re.compile(r"\b(anonymous|various|unknown|company|co\.|inc\.|corporation|department|bureau|society|association|"
                          r"institute|board|commission|council|office|government|ministry|club|league|school|college|"
                          r"university|station|service)\b", re.I)


def rights_ok(b):
    """"ok", "check" (keep only without a copyright notice after DEATH_CUTOFF) or False."""
    if "copyrighted" in (b.get("rights") or "").lower():
        return False   # distributed by Gutenberg with the holder's permission, which doesn't extend to us
    verdict = "ok"
    for a in b["authors"]:
        if NOT_A_PERSON.search(a["name"] or ""):
            verdict = "check"   # anonymous, collective or corporate/government author
            continue
        d, born = a.get("death"), a.get("birth")
        if d is not None and str(d).lstrip("-").isdigit():
            if int(d) > DEATH_CUTOFF:
                return False
        elif not (born is not None and str(born).lstrip("-").isdigit() and int(born) <= 1850):
            verdict = "check"   # dates unknown: the catalogue often has none for obscure ~1900 authors
    return "check" if not b["authors"] else verdict


COPYRIGHT = re.compile(r"(?:copyright|copr\.|©)\D{0,30}((?:19|20)\d\d)", re.I)


def late_copyright(raw: str) -> bool:
    """An anonymous book whose front matter carries a copyright notice after DEATH_CUTOFF (mid-century leaflets are
    public domain in the USA for lack of renewal, but not under life/publication+70 rules)."""
    text = re.sub(r"<[^>]+>", " ", raw[:60000])
    return any(int(y) > DEATH_CUTOFF for y in COPYRIGHT.findall(text))


def author_name(b):
    names = []
    for a in b["authors"]:
        n = a["name"] or ""
        n = re.sub(r"\s*\(.*?\)", "", n)
        if ", " in n:   # "Austen, Jane" -> "Jane Austen"; "Tolstoy, Leo, graf" -> "Leo Tolstoy"
            last, first = n.split(", ", 2)[:2]
            n = f"{first} {last}"
        names.append(n.strip())
    return " and ".join(names) if len(names) <= 2 else f"{names[0]} et al."


def clean_title(t):
    t = t.split("\n")[0].replace(" : $b", ": ").replace("$b ", "")
    return re.sub(r"\s+", " ", t).strip()


def short_title(t):
    t = re.split(r"[:;]\s", t)[0]   # "Moby Dick; Or, The Whale"; never at ". " ("Dr. Jekyll", "the Rev. Hugh")
    return t if len(t) <= 70 else t[:67].rsplit(" ", 1)[0] + "…"


def nice_heading(h):
    h = norm_space(h).strip(" .")
    if h.isupper() and len(h) > 3:
        h = h.title().replace("'S ", "'s ")
    h = re.sub(r"\b(CHAPTER|BOOK|PART|VOLUME|ACT|SCENE|SECTION|STAVE|CANTO)\b", lambda m: m.group(1).title(), h)
    h = re.sub(r"\b(Act|Scene|Chapter|Book|Part|Volume|Section|Stave|Canto) ([IVXLCDM][ivxlcdm]*)\b",
               lambda m: f"{m.group(1)} {m.group(2).upper()}", h)
    return h[:120]


def flatten(node, level):
    """Block nodes in document order; containers holding a split-level heading are opened up."""
    for c in node.iter(include_text=False):
        if c.tag in ("div", "section", "article", "main") and c.css_first(level) is not None:
            yield from flatten(c, level)
        else:
            yield c


def text_len(nodes):
    return sum(len(norm_space(n.text(deep=True))) for n in nodes)


def split_book(body):
    for level in ("h2", "h3", "h4"):
        if len(body.css(level)) >= 2:
            break
    else:
        level = None
    parts, cur = [], {"title": None, "nodes": []}
    for n in (flatten(body, level) if level else body.iter(include_text=False)):
        if level and n.tag == level:
            if cur["nodes"] or cur["title"]:
                parts.append(cur)
            cur = {"title": nice_heading(n.text(deep=True, separator=" ")), "nodes": []}
        else:
            cur["nodes"].append(n)
    parts.append(cur)
    # merge short parts into the next; drop contents/index pages
    out, carry, carry_title = [], [], None
    for p in parts:
        if p["title"] and SKIP_PART.search(p["title"]):
            continue
        if p["title"] and re.match(r"(by|with|translated|edited|illustrated)\b", p["title"], re.I):
            carry += p["nodes"]   # title-page lines ("by William Shakespeare")
            continue
        p["nodes"] = carry + p["nodes"]
        carry = []
        if text_len(p["nodes"]) < MIN_PART and p is not parts[-1]:
            carry = p["nodes"]
            if p["title"] and carry_title is None:
                carry_title = p["title"]   # e.g. a play's name before its "Dramatis Personae" and "Act I"
            continue
        if carry_title and p["title"] and carry_title.lower() != p["title"].lower():
            p["title"] = f"{carry_title}: {p['title']}"
        elif carry_title and not p["title"]:
            p["title"] = carry_title
        carry_title = None
        out.append(p)
    # cut very long parts
    final = []
    for p in out:
        if text_len(p["nodes"]) <= MAX_PART:
            final.append(p)
            continue
        piece, size, k = [], 0, 1
        for n in p["nodes"]:
            piece.append(n); size += len(n.text(deep=True))
            if size >= MAX_PART * 0.6:
                final.append({"title": f"{p['title'] or 'Text'} ({k})", "nodes": piece}); piece, size, k = [], 0, k + 1
        if piece:
            final.append({"title": f"{p['title'] or 'Text'} ({k})" if k > 1 else p["title"], "nodes": piece})
    for i, p in enumerate(final):
        p["title"] = p["title"] or ("Beginning" if i == 0 else f"Part {i + 1}")
    return final


START = re.compile(r"\*\*\*\s*START OF (THE|THIS) PROJECT GUTENBERG[^*]*\*\*\*", re.I)
END = re.compile(r"\*\*\*\s*END OF (THE|THIS) PROJECT GUTENBERG[^*]*\*\*\*", re.I)


def strip_boilerplate(raw: str) -> str:
    """The text between the '*** START OF … ***' and '*** END OF … ***' markers (licence and header removed)."""
    m = START.search(raw)
    if m:
        close = raw.find("</", m.end())
        raw = raw[raw.find(">", close) + 1 if close >= 0 else m.end():]
    m = END.search(raw)
    if m:
        raw = raw[: raw.rfind("<", 0, m.start())]
    return f"<html><body>{raw}</body></html>"


INTERNAL_A = re.compile(r'<a href="#[^"]*">(.*?)</a>', re.S)
IMG = re.compile(r'<img src="sp-img:([^"]*)"([^>]*)>')


def nav(bid, i, n):
    prev = f'<a href="wiki:pg{bid}/{i - 1}">‹ Previous</a>' if i > 1 else "<span></span>"
    nxt = f'<a href="wiki:pg{bid}/{i + 1}">Next ›</a>' if i < n else "<span></span>"
    return f'<nav class="book">{prev}<a href="wiki:pg{bid}">Contents</a>{nxt}</nav>'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("books"); ap.add_argument("catalog"); ap.add_argument("ids"); ap.add_argument("out")
    ap.add_argument("--source", required=True, help="source id, e.g. gutenberg-practical")
    ap.add_argument("--title", required=True)
    ap.add_argument("--images", action="store_true", help="keep illustrations (diagrams in practical books)")
    ap.add_argument("--max-image-bytes", type=int, default=400_000, help="skip larger originals (photo plates)")
    a = ap.parse_args()

    books, out = Path(a.books), Path(a.out)
    cat = {}
    for l in open(a.catalog):
        b = json.loads(l); cat[b["id"]] = b
    ids = [int(x) for x in open(a.ids).read().split()]
    out.mkdir(parents=True, exist_ok=True); (out / "images").mkdir(exist_ok=True)
    chunker = Chunker()
    docs_f, chunks_f = open(out / "docs.jsonl", "w"), open(out / "chunks.jsonl", "w")
    red_f, img_f = open(out / "redirects.jsonl", "w"), open(out / "images.jsonl", "w")
    seen_img, stats = set(), {"books": 0, "docs": 0, "chunks": 0, "rights": 0, "missing": 0, "empty": 0}
    page_id = 0

    def write(rec):
        nonlocal page_id
        page_id += 1
        rec["page_id"] = page_id
        docs_f.write(json.dumps(rec, ensure_ascii=False) + "\n")
        for c in chunker.chunk(rec["doc_id"], rec["title"], rec["blocks"]):
            chunks_f.write(json.dumps(c, ensure_ascii=False) + "\n"); stats["chunks"] += 1
        stats["docs"] += 1

    for bid in ids:
        b = cat.get(bid)
        if b is None:
            continue
        verdict = rights_ok(b)
        if not verdict:
            stats["rights"] += 1; continue
        hp = books / str(bid) / f"{bid}-h.htm"
        if not hp.exists():
            stats["missing"] += 1; continue
        raw = strip_boilerplate(hp.read_bytes().decode("utf-8", "replace"))
        if verdict == "check" and late_copyright(raw):
            stats["rights"] += 1; continue
        tree = HTMLParser(raw)
        body = tree.body
        for n in body.css("#pg-header, #pg-footer, .pg-boilerplate, .pagenum, span.pagenum, .pgheader, .pgfooter, "
                          "#pg-machine-header, #pg-start-separator, #pg-end-separator"):
            n.decompose()
        for h in body.css("h1"):
            h.decompose()   # the book title; the contents page shows it
        parts = split_book(body)
        if not parts or text_len(parts[0]["nodes"]) == 0 and len(parts) == 1:
            stats["empty"] += 1; continue
        title, author = clean_title(b["title"]), author_name(b)
        st = short_title(title)
        url = f"https://www.gutenberg.org/ebooks/{bid}"
        common = {"source": a.source, "lang": "en", "url": url, "rev_id": None, "rev_date": (b["issued"] or None),
                  "license": "Public domain", "categories": b["subjects"][:8] + [f"Gutenberg #{bid}"]}
        written = []
        for i, p in enumerate(parts, 1):
            frag = "".join(n.html or "" for n in p["nodes"])
            sub = HTMLParser(f"<body>{frag}</body>").body
            cl = Cleaner(f"{bid}/x")
            cl.walk(sub)
            html = INTERNAL_A.sub(r"\1", norm_html("".join(cl.out)))
            images = []

            def img(m):
                path, rest = m.group(1), m.group(2)
                if not a.images:
                    return ""
                f = books / path
                if not f.exists() or f.stat().st_size > a.max_image_bytes:
                    return ""
                data = f.read_bytes()
                key = hashlib.sha256(data).hexdigest()[:24]
                if key not in seen_img:
                    try:
                        info, files = encode_image(data, "image/" + f.suffix.lstrip(".").lower().replace("jpg", "jpeg"))
                    except Exception:
                        return ""
                    d = out / "images" / key[:2]; d.mkdir(exist_ok=True)
                    for v, blob in files.items():
                        (d / (f"{key}.svg" if v == "svg" else f"{key}.{v}.webp")).write_bytes(blob)
                    img_f.write(json.dumps({"key": key, "paths": [path], "src_bytes": len(data), **info}) + "\n")
                    seen_img.add(key)
                images.append(key)
                return f'<img src="img:{key}"{rest}>'

            html = IMG.sub(img, html)
            html = re.sub(r"<figure>\s*(<figcaption>\s*</figcaption>)?\s*</figure>", "", html)
            if sum(len(x["text"]) for x in cl.blocks) < 200:
                continue
            written.append((p["title"], html, cl.blocks, images))
        if not written:
            stats["empty"] += 1; continue
        n = len(written)
        # contents page
        lis = "".join(f'<li><a href="wiki:pg{bid}/{i}">{esc(t)}</a></li>' for i, (t, *_rest) in enumerate(written, 1))
        summary = norm_space(b.get("summary") or "")
        blocks = [{"id": "p0", "t": "p", "text": f"{title}. By {author}." if author else f"{title}."}]
        chtml = f'<p id="p0"><b>{esc(title)}</b>{(" · " + esc(author)) if author else ""}</p>'
        if summary:
            blocks.append({"id": "p1", "t": "p", "text": summary})
            chtml += f'<p id="p1">{esc(summary)}</p>'
        if b["subjects"]:
            chtml += f'<p class="meta">{esc("; ".join(b["subjects"][:6]))}</p>'
        chtml += f'<h2 id="h-contents">Contents</h2><ul class="toc">{lis}</ul>'
        write({**common, "doc_id": f"{a.source}/{bid}", "title": f"{st} ({author})" if author else st, "path": str(bid),
               "html": chtml, "blocks": blocks, "images": [], "book": {"id": bid, "part": 0, "parts": n}})
        red_f.write(json.dumps({"from_title": f"pg{bid}", "to_path": str(bid)}) + "\n")
        for i, (t, html, blocks, images) in enumerate(written, 1):
            path = f"{bid}/{i}"
            write({**common, "doc_id": f"{a.source}/{path}", "title": f"{st}: {t}", "path": path,
                   "html": nav(bid, i, n) + html + nav(bid, i, n), "blocks": blocks, "images": images,
                   "book": {"id": bid, "part": i, "parts": n}})
            red_f.write(json.dumps({"from_title": f"pg{bid}/{i}", "to_path": path}) + "\n")
        stats["books"] += 1
        if stats["books"] % 50 == 0:
            print(f"{stats['books']} books, {stats['docs']} documents, {stats['chunks']} chunks", flush=True)
    for f in (docs_f, chunks_f, red_f, img_f):
        f.close()
    (out / "source.json").write_text(json.dumps({
        "source": a.source, "lang": "en", "site": "gutenberg", "docs": stats["docs"], "chunks": stats["chunks"],
        "redirects": stats["docs"], "images_unique": len(seen_img),
        "zim_metadata": {"Title": a.title, "Creator": "Project Gutenberg volunteers (public-domain texts)",
                         "Publisher": "Survivalpedia", "Source": "www.gutenberg.org"},
        "stats": stats, "rights_rule": f"no author died after {DEATH_CUTOFF}; unknown dates / anonymous / corporate only without a copyright notice after {DEATH_CUTOFF}; Gutenberg 'Copyrighted' books excluded",
    }, indent=1))
    print("done:", json.dumps(stats), f"images {len(seen_img)}")


if __name__ == "__main__":
    main()
