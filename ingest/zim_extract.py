"""ZIM -> cleaned docs, chunks, redirects and WebP images (DESIGN §5, §7, §13).

    uv run python zim_extract.py <zim> <out_dir> --source <id> --lang <en|zh|ja>

Entries are read in ZIM cluster order (near-sequential disk reads). Interrupted runs resume:
rerun the same command; --fresh starts over.

Outputs in <out_dir>:
    source.json      ZIM metadata, counts, chunking parameters
    docs.jsonl       doc_id, source, lang, title, path, url, page_id, rev_id, rev_date,
                     license, categories, html, blocks[{id, t, text}], images[keys]
    chunks.jsonl     chunk_id, doc_id, block_ids, n_tokens, text (title + section prepended)
    redirects.jsonl  from_path, from_title, to_path
    images.jsonl     key, w, h, variants, paths (original ZIM paths)
    images/<k2>/<key>.{320,640}.webp | <key>.svg

Works for mwoffliner ZIMs (Parsoid <section> layout and the older flat mw-headline layout).
"""

from __future__ import annotations

import argparse
import hashlib
import html as htmlmod
import io
import multiprocessing as mp
import os
import re
import shutil
import sys
import time
from pathlib import Path
from urllib.parse import unquote

import orjson
from libzim.reader import Archive
from PIL import Image
from selectolax.lexbor import LexborHTMLParser as HTMLParser
from tqdm import tqdm

sys.path.insert(0, str(Path(__file__).resolve().parent))
from wp_scrub import scrub_doc  # noqa: E402

TOKENIZER = "Qwen/Qwen3-Embedding-0.6B"
CHUNK_TOKENS = 256
CHUNK_MAX_TOKENS = 384  # a single block above this is split
CHUNK_MIN_TOKENS = 64  # don't start a new chunk at a section boundary below this
IMG_WIDTHS = (320, 640)
IMG_QUALITY = 60

# Output element whitelist (DESIGN §5) plus br/caption, which tables and infoboxes need.
ALLOWED = {
    "p", "h2", "h3", "h4", "ul", "ol", "li", "table", "tr", "td", "th", "caption",
    "figure", "img", "figcaption", "a", "b", "i", "sup", "sub", "blockquote", "pre",
    "code", "br",
}
RENAME = {"strong": "b", "em": "i", "h1": "h4", "h5": "h4", "h6": "h4", "dt": "p", "dd": "p",
          "kbd": "code", "tt": "code", "var": "i", "cite": "i"}
BLOCKS = {"p", "h2", "h3", "h4", "li", "table", "figcaption", "blockquote", "pre"}
HEADINGS = {"h2": 2, "h3": 3, "h4": 4}
DROP_TAGS = {"style", "script", "link", "meta", "noscript", "math", "input", "button",
             "svg", "audio", "video", "source", "track", "iframe", "object", "form", "map",
             "template", "head", "title"}
DROP_CLASSES = {
    "navbox", "navbox-styles", "vertical-navbox", "navbar", "hatnote", "dablink", "mw-editsection",
    "reference", "mw-ref", "mw-references-wrap", "reflist", "references", "mw-cite-backlink",
    "noprint", "metadata", "ambox", "mbox-small", "ombox", "tmbox", "cmbox", "sistersitebox",
    "side-box", "portalbox", "portal", "catlinks", "zim-footer", "mw-empty-elt",
    "authority-control", "toc", "mw-jump-link", "shortdescription", "Z3988", "printfooter",
    "mw-indicators", "mwe-math-mathml-a11y", "mwe-math-mathml-inline",
    "mwe-math-mathml-display", "noexcerpt", "box-Unreferenced", "magnify", "mw-kartographer-maplink",
    # wikiHow (wikihow2zim): byline, table of contents, voting widgets, related articles
    "article_byline", "en_toc", "wh_vote_container", "wh_thumb_response", "relatedwikihows",
    # iFixit (ifixit2zim): author, stats, comments, widgets
    "guide-author", "stats-container", "guide-comments-container", "js-comment-container", "actionbar-portal",
    "difficulty-indicator", "statuses", "author-container", "credit-author-team-container",
    # Stack Exchange (sotoki): votes, user cards, tags, comments, menus
    "votecell", "post-signature", "s-user-card", "post-taglist", "js-post-menu", "comments", "js-comments-container",
    "s-topbar", "site-header", "left-sidebar", "answers-subheader", "bounty-indicator",
}
DROP_IDS = {"toc", "catlinks", "siteSub", "contentSub", "jump-to-nav", "coordinates", "byline_hover",
            "author-container", "sidebar", "answers-header"}
# Whole sections dropped by heading text (lower-cased, en/zh/ja).
DROP_SECTIONS = {
    "references", "external links", "further reading", "notes", "sources", "bibliography",
    "citations", "see also", "footnotes", "notes and references", "references and notes",
    "general references", "works cited", "explanatory notes", "external link",
    "参考文献", "外部リンク", "脚注", "出典", "関連項目", "注釈", "参考", "外部連結", "外部链接",
    "参考资料", "參考資料", "参考来源", "參考來源", "注释", "註釋", "延伸阅读", "延伸閱讀",
    "参见", "參見", "相关条目", "相關條目", "参考书目", "參考書目", "引用", "脚注・出典",
    "you might also like", "about this article",  # wikiHow
}
DECORATIVE_IMG = re.compile(
    r"(Flag_of_|Icon|icon|Symbol_|Commons-logo|Wiktionary|Wikisource|Wikiquote|Wikibooks|"
    r"Wikiversity|Wikivoyage|Wikinews|Wikispecies|Wikidata|Question_book|Ambox|Edit-clear|"
    r"Padlock|Lock-|Crystal_Clear|Nuvola|Folder_Hexagonal|Portal-puzzle|Star_of_life|OOjs_UI|"
    r"Disambig|Text_document_with|Gnome-|Sound-icon|Red_pog|Blue_pog|Green_pog)"
)
FOOTER_RE = re.compile(
    r'class="zim-footer".*?title="Last edited on (\d{4}-\d{2}-\d{2})"\s+href="([^"]+)".*?'
    r'href="([^"]+)"', re.S)
LICENSE_URL_RE = re.compile(r"creativecommons\.org/licenses/([a-z-]+)/(\d\.\d)")
REFRESH_RE = re.compile(r'http-equiv="refresh"\s+content="\d+;\s*URL=\'?([^\'"]+)', re.I)
SENT_RE = re.compile(r"(?<=[.!?。！？．])\s*")

_SPACE = re.compile(r"\s+")


def norm_space(s: str) -> str:
    return _SPACE.sub(" ", s).strip()


def esc(s: str) -> str:
    return htmlmod.escape(s, quote=False)


def esc_attr(s: str) -> str:
    return htmlmod.escape(s, quote=True)


# ---------------------------------------------------------------------------
# HTML cleaning


class Cleaner:
    """Serialises the article body to whitelisted HTML and collects block texts in one pass."""

    def __init__(self, doc_path: str):
        self.doc_dir = doc_path.rsplit("/", 1)[0] + "/" if "/" in doc_path else ""
        self.out: list[str] = []
        self.blocks: list[dict] = []
        self.buf: list[str] | None = None  # text buffer of the open outermost block
        self.skip_level = 0  # >0 while inside a dropped section
        self.images: list[str] = []  # ZIM paths, in order

    def resolve(self, href: str) -> str:
        """A relative href as a ZIM path: "../../images/x" from "Guide/Name/123" is "images/x" (".." above the
        root is dropped)."""
        href = unquote(href)
        if href.startswith("/"):
            href = href.lstrip("/")
        elif not href.startswith("_assets_"):
            href = self.doc_dir + href
        parts: list[str] = []
        for p in href.split("/"):
            if p == "..":
                if parts:
                    parts.pop()
            elif p != ".":
                parts.append(p)
        return "/".join(parts)

    def text(self, s: str):
        if self.skip_level:
            return
        self.out.append(esc(s))
        if self.buf is not None:
            self.buf.append(s)

    def walk(self, node):
        for child in node.iter(include_text=True):
            tag = child.tag
            if tag == "-text":
                self.text(child.text_content or "")
                continue
            if tag.startswith(("-comment", "_comment", "!")) or tag in DROP_TAGS:
                continue
            attrs = child.attributes
            cls = attrs.get("class") or ""
            if cls and not DROP_CLASSES.isdisjoint(cls.split()):
                continue
            if attrs.get("id") in DROP_IDS or attrs.get("role") == "navigation":
                continue
            if "display:none" in (attrs.get("style") or "").replace(" ", ""):
                continue
            tag = RENAME.get(tag, tag)

            if tag in HEADINGS:
                level = HEADINGS[tag]
                htext = norm_space(child.text(deep=True, separator=""))
                if self.skip_level and level > self.skip_level:
                    continue
                self.skip_level = 0
                if htext.lower().rstrip(":：") in DROP_SECTIONS:
                    self.skip_level = level
                    continue
            elif self.skip_level:
                if tag not in ALLOWED or tag == "a":
                    self.walk(child)  # containers may hold the next heading
                continue

            if tag == "img":
                self.img(child, attrs)
            elif tag == "br":
                self.out.append("<br>")
                if self.buf is not None:
                    self.buf.append("\n")
            elif tag == "a":
                self.anchor(child, attrs)
            elif tag in ALLOWED:
                self.element(child, tag, attrs)
            else:
                self.walk(child)  # unwrap div/span/section/...

    def anchor(self, node, attrs):
        href = attrs.get("href") or ""
        if not href or href.startswith(("http:", "https:", "//", "mailto:", "#cite")) or \
                "action=edit" in href or "redlink=1" in href:
            self.walk(node)
            return
        if href.startswith("#"):
            target = href
        else:
            path, _, frag = href.partition("#")
            target = "wiki:" + self.resolve(path) + ("#" + frag if frag else "")
        self.out.append(f'<a href="{esc_attr(target)}">')
        self.walk(node)
        self.out.append("</a>")

    def img(self, node, attrs):
        src = attrs.get("src") or ""
        if not src or src.startswith(("data:", "http:", "https:", "//")):
            return
        path = self.resolve(src)
        cls = attrs.get("class") or ""
        is_math = "mwe-math" in cls
        try:
            w = int(attrs.get("width") or 0)
            h = int(attrs.get("height") or 0)
        except ValueError:
            w = h = 0
        if not is_math and ((0 < w < 30) or (0 < h < 30) or DECORATIVE_IMG.search(path)):
            return
        alt = attrs.get("alt") or ""
        parts = [f'<img src="sp-img:{esc_attr(path)}"']
        if alt:
            parts.append(f' alt="{esc_attr(alt)}"')
        if w and h:
            parts.append(f' width="{w}" height="{h}"')
        if is_math:
            parts.append(' class="math"')
            if self.buf is not None and alt:
                self.buf.append(alt)
        self.out.append("".join(parts) + ">")
        self.images.append(path)

    def element(self, node, tag, attrs):
        start = len(self.out)
        is_block = tag in BLOCKS and self.buf is None
        extra = ""
        if tag in ("td", "th"):
            for a in ("colspan", "rowspan"):
                v = attrs.get(a)
                if v and v.isdigit() and v != "1":
                    extra += f' {a}="{v}"'
        self.out.append(f"<{tag}{extra}>")
        if is_block:
            self.buf = []
        self.walk(node)
        if self.buf is not None:
            if tag in ("td", "th"):
                self.buf.append(" | ")
            elif tag in ("tr", "li", "p", "caption"):
                self.buf.append("\n")
        if not is_block:
            self.out.append(f"</{tag}>")
            return
        # keep line structure (table rows, nested list items, <br>), collapse other whitespace
        lines = (norm_space(r).strip(" |") for r in "".join(self.buf).split("\n"))
        text = "\n".join(r for r in lines if r)
        self.buf = None
        has_img = any(s.startswith("<img") for s in self.out[start:])
        if not text and not has_img:
            del self.out[start:]
            return
        bid = f"p{len(self.blocks)}"
        self.out[start] = f'<{tag} id="{bid}"{extra}>'
        self.out.append(f"</{tag}>")
        if text:
            self.blocks.append({"id": bid, "t": tag, "text": text})
        else:
            self.blocks.append({"id": bid, "t": tag, "text": ""})


ROOTS = {"ifixit": (".articleContainer", "#content"), "stackexchange": ("#mainbar",)}


def content_root(tree: HTMLParser, site: str = "wikipedia"):
    for sel in (*ROOTS.get(site, ()), "#mw-content-text", "main", "body"):
        n = tree.css_first(sel)
        if n is not None:
            return n
    return tree.root


RL_INT = {k: re.compile(rf'"{k}":(-?\d+)') for k in ("wgArticleId", "wgCurRevisionId", "wgNamespaceNumber")}


WIKIHOW_TITLE = re.compile(r"(?::\s*\d+\s+(?:Steps|Ways|Tips)\b.*|\s*\(with Pictures\).*|\s*-\s*wikiHow.*)$")


def parse_doc(raw: str, path: str, title: str, default_license: str, site: str = "wikipedia", base_url: str = ""):
    meta = {k: (int(m.group(1)) if (m := r.search(raw, 0, 20000)) else None) for k, r in RL_INT.items()}
    if meta["wgNamespaceNumber"] not in (None, 0):
        return None
    rev_date = url = license_ = None
    authors: list[dict] = []
    if fm := FOOTER_RE.search(raw):
        rev_date, url, lic_href = fm.group(1), htmlmod.unescape(fm.group(2)), fm.group(3)
        if lm := LICENSE_URL_RE.search(lic_href):
            license_ = f"CC {lm.group(1).upper()} {lm.group(2)}"
    tree = HTMLParser(raw)
    cats = [norm_space(li.text()) for li in tree.css("#mw-normal-catlinks li")]
    if site == "wikihow":
        h1 = tree.css_first("h1")
        title = norm_space(h1.text()) if h1 is not None else WIKIHOW_TITLE.sub("", title)
        url = url or f"https://www.wikihow.com/{path}"
        cats = cats or [norm_space(a.text()) for a in tree.css("#breadcrumb li a")][1:]  # skip "Categories"
    if site == "ifixit":
        h1 = tree.css_first("h1")
        title = norm_space(h1.text()) if h1 is not None else title
        for n in tree.css("h1"):  # shown as the page title already
            n.decompose()
        # a step with several photos repeats the first as a 110 px thumbnail: keep the thumbnails, full size,
        # and drop the big copy
        for main in tree.css('img[id$="-mainImage"]'):
            step = (main.attributes.get("id") or "").split("-")[0]
            thumbs = tree.css(f'img[id^="{step}-stepImage"]')
            if thumbs:
                main.decompose()
                for t in thumbs:
                    for a in ("width", "height"):
                        if a in t.attributes:
                            del t.attrs[a]
    elif site == "stackexchange":
        h1 = tree.css_first("h1")
        title = norm_space(h1.text()) if h1 is not None else re.sub(r" - [^-]+ Stack Exchange$", "", title)
        q = tree.css_first("#question")
        qscore = int(q.attributes.get("data-score") or 0) if q is not None else 0
        answers = tree.css("div.answer")
        scores = [int(a.attributes.get("data-score") or 0) for a in answers]
        # questions nobody found useful, with no useful answer: noise
        if qscore < 1 and not any(sc >= 1 for sc in scores):
            return None
        cats = [norm_space(a.text()) for a in tree.css(".post-taglist a.post-tag")]
        for n in tree.css("h1"):
            n.decompose()
        # CC BY-SA asks for the authors by name: the asker and each kept answer's author, with profile links,
        # shown in the footer (not in the text, so passages and their vectors stay the same)
        authors = []
        if q is not None and (au := se_author(q, "question", base_url)):
            authors.append(au)
        for a, sc in zip(answers, scores):
            if sc >= 0 and (au := se_author(a, "answer", base_url)):
                authors.append(au)
        if authors:
            license_ = se_license(authors)
        for a, sc in zip(answers, scores):
            if sc < 0:
                a.decompose()
                continue
            accepted = "accepted-answer" in (a.attributes.get("class") or "")
            label = ("Accepted answer" if accepted else "Answer") + f" ({sc} vote{'' if abs(sc) == 1 else 's'})"
            a.insert_before(HTMLParser(f"<h2>{label}</h2>").css_first("h2"))
    if site in ("ifixit", "stackexchange") and base_url:
        url = url or base_url.rstrip("/") + "/" + path
    cl = Cleaner(path)
    cl.walk(content_root(tree, site))
    html = norm_html("".join(cl.out))
    return scrub_doc({
        "title": title, "path": path, "url": url, "page_id": meta["wgArticleId"],
        "rev_id": meta["wgCurRevisionId"], "rev_date": rev_date,
        "license": license_ or default_license, "license_parsed": license_ is not None,
        "categories": cats, "html": html, "blocks": cl.blocks, "images": cl.images,
        **({"authors": authors} if authors else {}),
    })


_SE_DATE = re.compile(r"(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) (\d+)(?:, (\d{4})| '(\d\d))?")
_MONTHS = "Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec".split()


def se_author(post, role: str, base_url: str) -> dict | None:
    """The post's author from its signature ("asked …" / "answered …", not "edited …"): name, profile URL, date."""
    for sig in post.css(".post-signature"):
        t = sig.css_first("time")
        when = (t.attributes.get("datetime") or t.text()) if t is not None else ""
        if not when.startswith(("asked", "answered")):
            continue
        link = sig.css_first(".s-user-card--link")
        info = sig.css_first(".s-user-card--info")
        name = norm_space((link or info).text()) if (link or info) is not None else ""
        if not name:
            return None
        href = link.attributes.get("href") if link is not None else None
        url = None
        if href and base_url and "/users/" in href:
            url = base_url.rstrip("/") + "/users/" + href.split("/users/", 1)[1]
        out = {"name": name, "role": role}
        if url:
            out["url"] = url
        if m := _SE_DATE.search(when):
            year = m.group(3) or (("20" + m.group(4)) if m.group(4) else None)
            if year:
                out["date"] = f"{year}-{_MONTHS.index(m.group(1)) + 1:02d}-{int(m.group(2)):02d}"
        return out
    return None


def se_license(authors: list[dict]) -> str:
    """Stack Exchange posts are CC BY-SA 2.5 before 2011-04-08, 3.0 before 2018-05-02, 4.0 after (by post date)."""
    vs = []
    for a in authors:
        d = a.get("date") or "9999"
        v = "2.5" if d < "2011-04-08" else "3.0" if d < "2018-05-02" else "4.0"
        if v not in vs:
            vs.append(v)
    return "CC BY-SA " + " / ".join(sorted(vs))


_EMPTY = re.compile(r"<(b|i|sup|sub|a[^>]*|code|ul|ol|tr|caption)>\s*</\w+>")


def norm_html(h: str) -> str:
    h = re.sub(r"[ \t\r\n]+", " ", h)
    for _ in range(3):
        h2 = _EMPTY.sub("", h)
        if h2 == h:
            break
        h = h2
    return h.strip()


# ---------------------------------------------------------------------------
# Chunking


class Chunker:
    def __init__(self):
        from transformers import AutoTokenizer

        self.tok = AutoTokenizer.from_pretrained(TOKENIZER, local_files_only=True)

    def ntok(self, texts: list[str]) -> list[int]:
        if not texts:
            return []
        return [len(x) for x in self.tok(texts, add_special_tokens=False)["input_ids"]]

    def split_long(self, text: str) -> list[str]:
        """Split an oversized block into pieces of <= CHUNK_TOKENS, on sentences where possible."""
        sents = [s for s in SENT_RE.split(text) if s.strip()]
        out, cur, cur_n = [], [], 0
        for s, n in zip(sents, self.ntok(sents)):
            if n > CHUNK_TOKENS:
                if cur:
                    out.append("".join(cur)); cur, cur_n = [], 0
                ids = self.tok(s, add_special_tokens=False)["input_ids"]
                for i in range(0, len(ids), CHUNK_TOKENS):
                    out.append(self.tok.decode(ids[i:i + CHUNK_TOKENS]))
                continue
            if cur and cur_n + n > CHUNK_TOKENS:
                out.append("".join(cur)); cur, cur_n = [], 0
            cur.append(s if not cur or s[:1].isspace() or not s[:1].isascii() else " " + s)
            cur_n += n
        if cur:
            out.append("".join(cur))
        return out

    def chunk(self, doc_id: str, title: str, blocks: list[dict]) -> list[dict]:
        heads: dict[int, str] = {}
        items = []  # (block_id, section_path, text)
        for b in blocks:
            if b["t"] in HEADINGS:
                lvl = HEADINGS[b["t"]]
                heads = {k: v for k, v in heads.items() if k < lvl}
                heads[lvl] = b["text"]
                items.append((b["id"], None, None, lvl))
            elif b["text"]:
                items.append((b["id"], " > ".join(heads[k] for k in sorted(heads)), b["text"], 0))
        texts = [t for _, _, t, _ in items if t is not None]
        counts = iter(self.ntok(texts))
        chunks: list[dict] = []
        cur_ids: list[str] = []
        cur_txt: list[str] = []
        cur_n = 0
        cur_sec = ""

        def flush():
            nonlocal cur_ids, cur_txt, cur_n
            if cur_txt:
                chunks.append(self._mk(doc_id, title, cur_sec, cur_ids, "\n".join(cur_txt), cur_n))
            cur_ids, cur_txt, cur_n = [], [], 0

        for bid, sec, text, lvl in items:
            if text is None:
                if lvl == 2 and cur_n >= CHUNK_MIN_TOKENS:
                    flush()
                continue
            n = next(counts)
            if n > CHUNK_MAX_TOKENS:
                flush()
                for piece in self.split_long(text):
                    cur_sec = sec
                    chunks.append(self._mk(doc_id, title, sec, [bid], piece, None))
                continue
            if cur_txt and cur_n + n > CHUNK_TOKENS:
                flush()
            if not cur_txt:
                cur_sec = sec
            cur_ids.append(bid)
            cur_txt.append(text)
            cur_n += n
        flush()
        missing = [i for i, c in enumerate(chunks) if c["n_tokens"] is None]
        for i, n in zip(missing, self.ntok([chunks[i]["text"] for i in missing])):
            chunks[i]["n_tokens"] = n
        for i, c in enumerate(chunks):
            c["chunk_id"] = f"{doc_id}#{i}"
        return chunks

    @staticmethod
    def _mk(doc_id, title, sec, ids, body, n):
        head = title if not sec else f"{title} > {sec}"
        return {"chunk_id": None, "doc_id": doc_id, "block_ids": list(ids),
                "n_tokens": n, "text": f"{head}\n{body}"}


# ---------------------------------------------------------------------------
# ZIM directory: read entries in cluster order, so a big ZIM on a slow disk is read about
# sequentially instead of seeking for every article (entry ids are in path order).

def zim_order(zim_path: str):
    """Return (HTML entry ids sorted by cluster, redirect ids, {image path: cluster})."""
    import mmap
    import struct

    import numpy as np

    with open(zim_path, "rb") as f:
        mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
    magic, major, _minor = struct.unpack_from("<IHH", mm, 0)
    if magic != 0x044D495A:
        raise ValueError(f"not a ZIM file: {zim_path}")
    n, _clusters, path_ptr, _title_ptr, _cluster_ptr, mime_pos = struct.unpack_from("<IIQQQQ", mm, 24)
    mimes, pos = [], mime_pos
    while mm[pos] != 0:
        end = mm.find(b"\0", pos)
        mimes.append(mm[pos:end].decode())
        pos = end + 1
    is_img = np.array([m.startswith("image/") for m in mimes] + [False], dtype=bool)
    is_html = np.array([m.startswith("text/html") for m in mimes] + [False], dtype=bool)

    ptrs = np.frombuffer(mm, dtype="<u8", count=n, offset=path_ptr)
    order = np.argsort(ptrs, kind="stable")  # walk dirents in file order (sequential reads)
    mime = np.empty(n, dtype=np.uint16)
    cluster = np.empty(n, dtype=np.uint32)
    blob = np.empty(n, dtype=np.uint32)
    images: dict[str, int] = {}
    unpack = struct.Struct("<HBcIII").unpack_from
    for i in order:
        off = int(ptrs[i])
        m, _plen, ns, _rev, c, b = unpack(mm, off)
        mime[i] = m
        if m == 0xFFFF:  # redirect: no cluster
            continue
        cluster[i], blob[i] = c, b
        if m < len(mimes) and is_img[m]:
            start = off + 16
            p = mm[start:mm.find(b"\0", start)].decode("utf-8", "replace")
            if major < 6 or ns == b"C":
                images[p] = c
    redirect = mime == 0xFFFF
    content = np.flatnonzero(~redirect & is_html[np.minimum(mime, len(mimes))])
    content = content[np.lexsort((blob[content], cluster[content]))]
    return content.tolist(), np.flatnonzero(redirect).tolist(), images


# ---------------------------------------------------------------------------
# Workers

_W: dict = {}


def _init_worker(zim_path: str, chunk: bool):
    _W["zim"] = Archive(zim_path)
    _W["chunker"] = Chunker() if chunk else None


PART_NAMES = ("docs", "chunks", "redirects", "images")


def part_done(part_dir: str, j: int) -> bool:
    return os.path.exists(f"{part_dir}/stats-{j:07d}.json")


def doc_worker(job):
    """Process one job (a list of entry ids) and write its part files; return (counts, image paths).

    Part files are written as .tmp and renamed; stats-<j>.json is written last and marks the job
    done, so an interrupted run resumes where it stopped."""
    j, ids, part_dir, source, lang, default_license, site, only, skip, base_url = job
    only_re, skip_re = (re.compile(only) if only else None), (re.compile(skip) if skip else None)
    z: Archive = _W["zim"]
    chunker: Chunker = _W["chunker"]
    images: set[str] = set()
    stats = {"docs": 0, "chunks": 0, "redirects": 0, "skipped": 0, "no_license": 0, "empty": 0}
    tmp = {k: f"{part_dir}/{k}-{j:07d}.jsonl.tmp" for k in PART_NAMES}
    with open(tmp["docs"], "wb") as fd, open(tmp["chunks"], "wb") as fc, open(tmp["redirects"], "wb") as fr:
        for i in ids:
            e = z.get_entry_by_id(i)
            if e.is_redirect:
                try:
                    to = e.get_redirect_entry().path
                except Exception:
                    continue
                fr.write(orjson.dumps({"from_path": e.path, "from_title": e.title, "to_path": to}) + b"\n")
                stats["redirects"] += 1
                continue
            item = e.get_item()
            if not item.mimetype.startswith("text/html") or e.path.startswith(("_", "-/")) \
                    or e.path in ("mainPage", "index", "favicon"):
                continue
            if (only_re and not only_re.search(e.path)) or (skip_re and skip_re.search(e.path)):
                stats["skipped"] += 1
                continue
            raw = bytes(item.content).decode("utf-8", "replace")
            if len(raw) < 2000 and (m := REFRESH_RE.search(raw)):  # soft redirect, often to a section
                to, _, frag = unquote(m.group(1)).removeprefix("./").partition("#")
                fr.write(orjson.dumps({"from_path": e.path, "from_title": e.title, "to_path": to,
                                       "fragment": frag or None}) + b"\n")
                stats["redirects"] += 1
                continue
            d = parse_doc(raw, e.path, e.title, default_license, site, base_url)
            if d is None:
                stats["skipped"] += 1
                continue
            if not any(bl["text"] for bl in d["blocks"]):
                stats["empty"] += 1
                continue
            doc_id = f"{source}/{e.path}"
            stats["no_license"] += not d.pop("license_parsed")
            d = {"doc_id": doc_id, "source": source, "lang": lang, **d}
            images.update(d["images"])
            fd.write(orjson.dumps(d) + b"\n")
            stats["docs"] += 1
            if chunker:
                for c in chunker.chunk(doc_id, d["title"], d["blocks"]):
                    fc.write(orjson.dumps(c) + b"\n")
                    stats["chunks"] += 1
    with open(tmp["images"], "wb") as fi:
        fi.write(orjson.dumps(sorted(images)))
    for k, t in tmp.items():
        os.rename(t, t.removesuffix(".tmp"))
    sf = f"{part_dir}/stats-{j:07d}.json"
    with open(sf + ".tmp", "wb") as f:
        f.write(orjson.dumps(stats))
    os.rename(sf + ".tmp", sf)
    return stats, images


def load_part(part_dir: str, j: int):
    with open(f"{part_dir}/stats-{j:07d}.json", "rb") as f:
        stats = orjson.loads(f.read())
    with open(f"{part_dir}/images-{j:07d}.jsonl", "rb") as f:
        return stats, set(orjson.loads(f.read()))


def encode_image(data: bytes, mimetype: str) -> tuple[dict, dict[str, bytes]]:
    """Return (info, {suffix: bytes})."""
    if "svg" in mimetype:
        return {"w": None, "h": None, "variants": ["svg"]}, {"svg": data}
    im = Image.open(io.BytesIO(data))
    animated = getattr(im, "is_animated", False)
    w, h = im.size
    out = {}
    for tw in IMG_WIDTHS:
        if tw == IMG_WIDTHS[-1] and w <= IMG_WIDTHS[0] and out:
            break  # small image: the 320 variant already holds it at full size
        if animated:
            frames = []
            for f in range(im.n_frames):
                im.seek(f)
                fr = im.convert("RGBA")
                if w > tw:
                    fr = fr.resize((tw, max(1, round(h * tw / w))), Image.LANCZOS)
                frames.append(fr)
            b = io.BytesIO()
            frames[0].save(b, "WEBP", save_all=True, append_images=frames[1:], quality=IMG_QUALITY,
                           duration=im.info.get("duration", 100), loop=0)
        else:
            src = im if im.mode in ("RGB", "RGBA", "L", "LA") else im.convert("RGBA" if "A" in im.mode or "transparency" in im.info else "RGB")
            if w > tw:
                src = src.resize((tw, max(1, round(h * tw / w))), Image.LANCZOS)
            b = io.BytesIO()
            src.save(b, "WEBP", quality=IMG_QUALITY, method=4)
        out[str(tw)] = b.getvalue()
    return {"w": w, "h": h, "variants": list(out)}, out


def image_worker(job):
    paths, img_dir = job
    z: Archive = _W["zim"]
    res = []
    for p in paths:
        try:
            item = z.get_entry_by_path(p).get_item()
            data = bytes(item.content)
            key = hashlib.sha256(data).hexdigest()[:24]
            sub = Path(img_dir) / key[:2]
            info, files = None, None
            if not any(sub.glob(f"{key}.*")):
                info, files = encode_image(data, item.mimetype)
                sub.mkdir(exist_ok=True)
                for suf, blob in files.items():
                    name = f"{key}.svg" if suf == "svg" else f"{key}.{suf}.webp"
                    tmp = sub / f".{name}.{os.getpid()}"
                    tmp.write_bytes(blob)
                    tmp.rename(sub / name)
            res.append((p, key, info, len(data)))
        except Exception as ex:  # missing or undecodable image
            res.append((p, None, {"error": f"{type(ex).__name__}: {ex}"}, 0))
    return res


# ---------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("zim")
    ap.add_argument("out_dir")
    ap.add_argument("--source", required=True)
    ap.add_argument("--lang", required=True, choices=["en", "zh", "ja"])
    ap.add_argument("--license", default="CC BY-SA 4.0", help="used when the ZIM footer has no CC licence link")
    ap.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    ap.add_argument("--limit", type=int, default=0, help="only the first N content entries and N redirects (testing)")
    ap.add_argument("--no-chunks", action="store_true")
    ap.add_argument("--no-images", action="store_true")
    ap.add_argument("--only-path", default="", help="regex: keep only HTML entries whose path matches")
    ap.add_argument("--skip-path", default="", help="regex: drop HTML entries whose path matches")
    ap.add_argument("--base-url", default="", help="ifixit/stackexchange: page URL = base + path (attribution)")
    ap.add_argument("--site", default="wikipedia", choices=["wikipedia", "wikihow", "ifixit", "stackexchange"],
                    help="page layout: MediaWiki/mwoffliner (default), wikiHow (wikihow2zim), iFixit (ifixit2zim), Stack Exchange (sotoki)")
    ap.add_argument("--fresh", action="store_true", help="discard finished jobs of an interrupted run")
    args = ap.parse_args()

    t0 = time.time()
    if not args.no_chunks:
        Chunker()  # fail fast: a failing Pool initializer respawns workers forever
    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    part_dir = out / "_parts"
    if args.fresh:
        shutil.rmtree(part_dir, ignore_errors=True)
    part_dir.mkdir(exist_ok=True)

    z = Archive(args.zim)
    meta = {}
    for k in z.metadata_keys:
        if not k.startswith("Illustration"):
            try:
                meta[k] = z.get_metadata(k).decode("utf-8", "replace")
            except Exception:
                pass

    t1 = time.time()
    content, redirects, img_cluster = zim_order(args.zim)
    if args.limit:  # the first N entries in read order
        content, redirects = content[:args.limit], redirects[:args.limit]
    print(f"directory: {len(content):,} HTML entries, {len(redirects):,} redirects, "
          f"{len(img_cluster):,} images ({time.time() - t1:.0f}s)", flush=True)
    # the job list is a pure function of the ZIM, so a resumed run gets the same jobs
    step = 2000
    id_lists = [content[a:a + step] for a in range(0, len(content), step)]
    id_lists += [redirects[a:a + 20000] for a in range(0, len(redirects), 20000)]
    jobs = [(j, ids, str(part_dir), args.source, args.lang, args.license, args.site, args.only_path, args.skip_path,
             args.base_url)
            for j, ids in enumerate(id_lists)]
    stats: dict[str, int] = {}
    images: set[str] = set()

    def add(st, im):
        for k, v in st.items():
            stats[k] = stats.get(k, 0) + v
        images.update(im)

    todo = []
    for job in jobs:
        if part_done(str(part_dir), job[0]):
            add(*load_part(str(part_dir), job[0]))
        else:
            todo.append(job)
    if len(todo) < len(jobs):
        print(f"resuming: {len(jobs) - len(todo):,} of {len(jobs):,} jobs already done", flush=True)
    ctx = mp.get_context("fork")
    with ctx.Pool(args.workers, _init_worker, (args.zim, not args.no_chunks)) as pool:
        # imap (ordered) hands jobs out in cluster order, so the workers read the disk front to back
        for st, im in tqdm(pool.imap(doc_worker, todo), total=len(todo), desc="docs", unit="job",
                           mininterval=30):
            add(st, im)
    t_docs = time.time() - t0

    # Images: encode each referenced image once, dedupe by content hash.
    keymap: dict[str, str] = {}
    img_records: dict[str, dict] = {}
    img_errors = 0
    if not args.no_images and images:
        img_dir = out / "images"
        img_dir.mkdir(exist_ok=True)
        plist = sorted(images, key=lambda p: (img_cluster.get(p, 1 << 32), p))
        ijobs = [(plist[i:i + 200], str(img_dir)) for i in range(0, len(plist), 200)]
        with ctx.Pool(args.workers, _init_worker, (args.zim, False)) as pool:
            for res in tqdm(pool.imap(image_worker, ijobs), total=len(ijobs), desc="images", unit="job",
                            mininterval=30):
                for p, key, info, size in res:
                    if key is None:
                        img_errors += 1
                        continue
                    keymap[p] = key
                    rec = img_records.setdefault(key, {"key": key, "paths": [], "src_bytes": size})
                    rec["paths"].append(p)
                    if info:
                        rec.update(info)
        with open(out / "images.jsonl", "wb") as f:
            for rec in img_records.values():
                f.write(orjson.dumps(rec) + b"\n")

    # Merge parts; rewrite image placeholders to content keys (or drop unresolved images).
    img_re = re.compile(r'<img src="sp-img:([^"]*)"([^>]*)>')

    def fix_img(m):
        key = keymap.get(htmlmod.unescape(m.group(1)))
        return f'<img src="img:{key}"{m.group(2)}>' if key else ""

    parts = [part_dir / f"docs-{j:07d}.jsonl" for j in range(len(jobs))]
    with open(out / "docs.jsonl", "wb") as fd:
        for p in parts:
            with open(p, "rb") as f:
                for line in f:
                    d = orjson.loads(line)
                    d["html"] = img_re.sub(fix_img, d["html"])
                    d["images"] = list(dict.fromkeys(keymap[x] for x in d["images"] if x in keymap))
                    fd.write(orjson.dumps(d) + b"\n")
    for name in ("chunks", "redirects"):
        with open(out / f"{name}.jsonl", "wb") as fo:
            for p in (part_dir / f"{name}-{j:07d}.jsonl" for j in range(len(jobs))):
                with open(p, "rb") as f:
                    shutil.copyfileobj(f, fo)
    shutil.rmtree(part_dir)

    summary = {
        "zim": os.path.basename(args.zim), "source": args.source, "lang": args.lang,
        "site": args.site, "zim_metadata": meta, **stats,
        "images_referenced": len(images), "images_unique": len(img_records), "image_errors": img_errors,
        "chunking": {"tokenizer": TOKENIZER, "target": CHUNK_TOKENS, "max_block": CHUNK_MAX_TOKENS},
        "image_encoding": {"widths": IMG_WIDTHS, "quality": IMG_QUALITY, "format": "webp"},
        "seconds": {"docs": round(t_docs, 1), "total": round(time.time() - t0, 1)},
    }
    (out / "source.json").write_bytes(orjson.dumps(summary, option=orjson.OPT_INDENT_2))
    print(orjson.dumps({k: v for k, v in summary.items() if k != "zim_metadata"}, option=orjson.OPT_INDENT_2).decode())


if __name__ == "__main__":
    sys.exit(main())
