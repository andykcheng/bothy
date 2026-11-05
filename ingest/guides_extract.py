"""Survival guides (PDF) -> the same extract format as zim_extract.py (source.json, docs.jsonl, chunks.jsonl,
redirects.jsonl, images.jsonl, images/), one document per chapter, so `sp build` can take them like any source.

  uv run python guides_extract.py <guides dir> <out dir>

Each guide has a config below: which PDF, licence, which font sizes/regexes start a document (chapter) or mark
headings, and what to drop (running headers, page numbers, contents pages). The PDFs have no bookmarks, so
structure comes from fonts (PyMuPDF spans). Diagrams are kept; tiny images and ones repeated on many pages
(logos, page furniture) are dropped.
"""
import hashlib, json, re, sys
from collections import Counter
from pathlib import Path

import pymupdf

from selectolax.lexbor import LexborHTMLParser as HTMLParser

from zim_extract import Chunker, Cleaner, encode_image, esc, norm_html, norm_space

# web pages saved as HTML (one document each); main content only, images dropped
SITES = [
    {"id": "gov-uk-be-ready", "dir": "gov-beready", "short": "UK Government: Be Ready",
     "title": "Be ready for emergencies (beready.campaign.gov.uk)", "publisher": "UK Government (Cabinet Office), 2026",
     "url": "https://beready.campaign.gov.uk/", "license": "Open Government Licence v3.0"},
]

GUIDES = [
    {"id": "fema-are-you-ready", "file": "fema/areyouready_full.pdf", "short": "FEMA: Are You Ready?",
     "title": "Are You Ready? An In-depth Guide to Citizen Preparedness", "publisher": "FEMA (US Federal Emergency Management Agency), 2004",
     "url": "https://www.ready.gov/are-you-ready-guide", "license": "Public domain (US Government work)",
     "start_page": 5,
     "doc": lambda t, size, bold: size >= 44 and not re.fullmatch(r"[\d.\s]+", t),
     "h2": lambda t, size, bold: 23.5 <= size < 30,
     "h3": lambda t, size, bold: 17.5 <= size < 19 or (bold and size >= 10.5 and len(t) < 70 and not t.endswith(".")),
     "drop": lambda t, size, bold: size in (30.0, 21.0, 39.0) or 14 < size < 15 or re.fullmatch(r"[\d.\s]+", t) is not None
             or t.startswith("Are You Ready? An In-depth Guide")},
    {"id": "fm-21-76", "file": "fm21-76/FM21-76_text.pdf", "short": "US Army Survival Manual FM 21-76",
     "title": "US Army Survival Manual (FM 21-76)", "publisher": "US Department of the Army, 1992",
     "url": "https://archive.org/details/USArmyFM2176", "license": "Public domain (US Government work)",
     "start_page": 5,
     "doc": lambda t, size, bold: bold and 13.5 <= size < 15 and re.match(r"(CHAPTER|APPENDIX)\b", t) is not None,
     "h2": lambda t, size, bold: bold and 11.5 <= size < 13,
     "h3": lambda t, size, bold: bold and 10.5 <= size < 11.5 and len(t) < 70 and t not in ("WARNING", "CAUTION", "NOTE"),
     "drop": lambda t, size, bold: size < 9.5 or "Reprinted as" in t or "...." in t or t.startswith("FM 21-76")},
    {"id": "who-icrc-basic-emergency-care", "file": "who-bec/who-bec.pdf", "short": "WHO-ICRC Basic Emergency Care",
     "title": "Basic Emergency Care: approach to the acutely ill and injured (participant workbook)",
     "publisher": "World Health Organization and International Committee of the Red Cross, 2018",
     "url": "https://www.who.int/publications/i/item/9789241513081", "license": "CC BY-NC-SA 3.0 IGO",
     "start_page": 8,
     "doc": lambda t, size, bold: 24 <= size < 26 and not bold and re.match(r"(Module \d|INTRODUCTION|Skills|SKILL|Annex|Glossary|Quick Cards)", t, re.I) is not None,
     "h2": lambda t, size, bold: 17.5 <= size < 19 or (24 <= size < 26),
     "h3": lambda t, size, bold: bold and 10.5 <= size < 17 and len(t) < 80 and not t.endswith("."),
     "drop": lambda t, size, bold: "....." in t or re.fullmatch(r"[\d\s]+", t) is not None or t.startswith("BEC |")},
]


LIGATURES = {"ﬁ ": "fi", "ﬂ ": "fl", "ﬁ": "fi", "ﬂ": "fl", "ﬀ": "ff", "ﬃ": "ffi", "ﬄ": "ffl"}
WHO_MODULES = {"1": "The ABCDE and SAMPLE history approach", "2": "Approach to trauma", "3": "Approach to difficulty in breathing",
               "4": "Approach to shock", "5": "Approach to altered mental status"}


def fix_text(t):
    for a, b in LIGATURES.items():
        t = t.replace(a, b)
    return t


def fix_title(t):
    t = re.sub(r"^(CHAPTER|Chapter) (\d+) ?- ?", lambda m: f"Chapter {m.group(2)}: ", t)
    if m := re.match(r"Module (\d):", t):
        t = f"Module {m.group(1)}: {WHO_MODULES.get(m.group(1), '')}".strip()
    head, sep, rest = t.rpartition(": ") if ": " in t else ("", "", t)
    if rest.isupper():
        rest = rest.title().replace(" And ", " and ").replace(" Of ", " of ").replace(" In ", " in ").replace(" With ", " with ")
    t = head + sep + rest
    return t.rstrip(", ")


def slug(s):
    return re.sub(r"[^A-Za-z0-9]+", "_", s).strip("_")[:80]


def is_bold(span):
    return "Bold" in span["font"] or "Black" in span["font"] or bool(span["flags"] & 16)


def main(src: Path, out: Path):
    out.mkdir(parents=True, exist_ok=True)
    (out / "images").mkdir(exist_ok=True)
    chunker = Chunker()
    docs_f, chunks_f, img_f = open(out / "docs.jsonl", "w"), open(out / "chunks.jsonl", "w"), open(out / "images.jsonl", "w")
    open(out / "redirects.jsonl", "w").close()
    seen_img, n_docs, n_chunks, page_id, counts = set(), 0, 0, 0, {}
    for g in GUIDES:
        pdf = pymupdf.open(src / g["file"])
        # images repeated on several pages are page furniture (logos, banners)
        reps = Counter()
        for p in pdf:
            for b in p.get_text("dict")["blocks"]:
                if b["type"] == 1:
                    reps[hashlib.sha256(b["image"]).hexdigest()] += 1
        docs, cur = [], None

        def new_doc(title, page):
            nonlocal cur
            cur = {"title": title, "page": page, "html": [], "blocks": [], "images": []}
            docs.append(cur)

        def add(kind, text, page):
            if cur is None:
                new_doc("Introduction", page)
            bid = f"p{len(cur['blocks'])}"
            if kind == "img":
                cur["html"].append(f'<figure><img src="img:{text}"></figure>')
                cur["images"].append(text)
                return
            if kind == "p" and cur["blocks"] and cur["blocks"][-1]["t"] == "p" and cur["_open"]:
                # a paragraph that continues from the previous page / column
                last = cur["blocks"][-1]
                last["text"] = (last["text"][:-1] if last["text"].endswith("-") else last["text"] + " ") + text
                cur["html"][-1] = f'<p id="{last["id"]}">{esc(last["text"])}</p>'
                cur["_open"] = not re.search(r"[.!?:)]$", last["text"])
                return
            cur["blocks"].append({"id": bid, "t": kind, "text": text})
            cur["html"].append(f'<{kind} id="{bid}">{esc(text)}</{kind}>')
            cur["_open"] = kind == "p" and not re.search(r"[.!?:)]$", text)

        for pno in range(g["start_page"] - 1, pdf.page_count):
            page = pdf[pno]
            for b in page.get_text("dict")["blocks"]:
                if b["type"] == 1:
                    h = hashlib.sha256(b["image"]).hexdigest()
                    if reps[h] > 2 or b["width"] < 80 or b["height"] < 60:
                        continue
                    key = h[:24]
                    if key not in seen_img:
                        try:
                            info, files = encode_image(b["image"], "image/" + b.get("ext", "png"))
                        except Exception:
                            continue
                        d = out / "images" / key[:2]; d.mkdir(exist_ok=True)
                        for v, data in files.items():
                            (d / (f"{key}.svg" if v == "svg" else f"{key}.{v}.webp")).write_bytes(data)
                        img_f.write(json.dumps({"key": key, "paths": [f"{g['file']}#page={pno + 1}"], "src_bytes": len(b["image"]), **info}) + "\n")
                        seen_img.add(key)
                    add("img", key, pno + 1)
                    continue
                # text block: group lines by style; a heading-styled line becomes its own block
                pend_kind, pend = None, []

                def flush():
                    nonlocal pend_kind, pend
                    if pend:
                        t = norm_space(" ".join(pend).replace("- ", "-") if pend_kind != "p" else re.sub(r"(\w)- (\w)", r"\1\2", " ".join(pend)))
                        if pend_kind == "doc":
                            if cur is not None and not cur["blocks"] and cur["title"] != "Introduction" and cur.get("_merge"):
                                cur["title"] += " " + t   # a chapter title that wraps onto the next line/block
                            else:
                                new_doc(t, pno + 1); cur["_merge"] = True
                        elif t:
                            add(pend_kind, t, pno + 1)
                            if cur is not None:
                                cur["_merge"] = False
                    pend_kind, pend = None, []

                for l in b.get("lines", []):
                    spans = [s for s in l["spans"] if s["text"].strip()]
                    if not spans:
                        continue
                    t = fix_text(norm_space("".join(s["text"] for s in l["spans"])))
                    size, bold = round(spans[0]["size"], 1), is_bold(spans[0])
                    if g["drop"](t, size, bold):
                        continue
                    kind = "doc" if g["doc"](t, size, bold) else "h2" if g["h2"](t, size, bold) else "h3" if g["h3"](t, size, bold) else "p"
                    if kind == "p" and re.match(r"^[•●▪■◦\-–]\s", t):
                        flush(); kind = "li"; t = t[2:]
                    if kind != pend_kind or kind == "li":
                        flush(); pend_kind = kind
                    pend.append(t)
                flush()
        # write the guide's documents
        n_before = n_docs
        for d in docs:
            text_blocks = [x for x in d["blocks"] if x["t"] in ("p", "li")]
            if sum(len(x["text"]) for x in text_blocks) < 200:
                continue   # cover pages, empty chapter dividers
            page_id += 1
            title = f"{fix_title(d['title'])} ({g['short']})"
            doc_id = f"guides-en/{g['id']}/{slug(d['title'])}"
            html = "".join(d["html"])
            html = html.replace("<li ", "<ul><li ").replace("</li>", "</li></ul>").replace("</ul><ul>", "")
            rec = {"doc_id": doc_id, "source": "guides-en", "lang": "en", "title": title, "path": f"{g['id']}/{slug(d['title'])}",
                   "url": f"{g['url']}", "page_id": page_id, "rev_id": None, "rev_date": None, "license": g["license"],
                   "categories": [g["title"], g["publisher"]], "html": html,
                   "blocks": [{"id": x["id"], "t": x["t"] if x["t"] != "li" else "li", "text": x["text"]} for x in d["blocks"]],
                   "images": d["images"]}
            docs_f.write(json.dumps(rec, ensure_ascii=False) + "\n")
            for c in chunker.chunk(doc_id, title, rec["blocks"]):
                chunks_f.write(json.dumps(c, ensure_ascii=False) + "\n"); n_chunks += 1
            n_docs += 1
        counts[g["id"]] = n_docs - n_before
        print(f"{g['id']}: {n_docs - n_before} documents", flush=True)
    for site in SITES:
        n_before = n_docs
        for hp in sorted((src / site["dir"]).glob("*.html")):
            tree = HTMLParser(hp.read_text(errors="replace"))
            root = tree.css_first("main") or tree.body
            h1 = root.css_first("h1")
            name = norm_space(h1.text()) if h1 is not None else hp.stem
            for n in root.css("h1, nav, form, .govuk-breadcrumbs, .share, script, style"):
                n.decompose()
            cl = Cleaner(hp.stem)
            cl.walk(root)
            html = re.sub(r"<figure>\s*(<figcaption>\s*</figcaption>)?\s*</figure>\s*", "", re.sub(r"<img [^>]*>", "", norm_html("".join(cl.out))))
            if sum(len(x["text"]) for x in cl.blocks) < 300:
                continue
            page_id += 1
            path = hp.stem.replace("__", "/")
            title = f"{'Overview' if path == 'home' else name} ({site['short']})"
            doc_id = f"guides-en/{site['id']}/{path}"
            rec = {"doc_id": doc_id, "source": "guides-en", "lang": "en", "title": title, "path": f"{site['id']}/{path}",
                   "url": site["url"] + ("" if path == "home" else path + "/"), "page_id": page_id, "rev_id": None, "rev_date": None,
                   "license": site["license"], "categories": [site["title"], site["publisher"]], "html": html,
                   "blocks": cl.blocks, "images": []}
            docs_f.write(json.dumps(rec, ensure_ascii=False) + "\n")
            for c in chunker.chunk(doc_id, title, rec["blocks"]):
                chunks_f.write(json.dumps(c, ensure_ascii=False) + "\n"); n_chunks += 1
            n_docs += 1
        counts[site["id"]] = n_docs - n_before
        print(f"{site['id']}: {n_docs - n_before} documents", flush=True)
    for f in (docs_f, chunks_f, img_f):
        f.close()
    (out / "source.json").write_text(json.dumps({
        "source": "guides-en", "lang": "en", "site": "guides", "docs": n_docs, "chunks": n_chunks, "redirects": 0,
        "images_unique": len(seen_img), "guides": [{k: g[k] for k in ("id", "title", "publisher", "url", "license")} | {"docs": counts[g["id"]]} for g in GUIDES + SITES],
        "notes": "FM 21-76 is a 1992 text reprint (archive.org USArmyFM2176) without the appendices.",
    }, indent=1))
    print(f"done: {n_docs} documents, {n_chunks} chunks, {len(seen_img)} images")


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]))
