"""The ebook shelf: Project Gutenberg EPUBs that people download one by one or by category and read in
the app's reader (no full-text search; titles and authors are searchable in the shelf index).

  uv run python books/epub_shelf.py fetch <catalog.jsonl> <shelf dir> [--illustrated ids.txt] [--limit N]
  uv run python books/epub_shelf.py index <catalog.jsonl> <shelf dir> [--illustrated ids.txt] [--limit N]

Selection: English texts that pass the worldwide rights rule (books_extract.rights_ok). Books whose
authors can't be dated ("check") are kept only if the text has no copyright notice after 1955.
fetch, most-downloaded first, resumable, spread over mirrors that carry Gutenberg's generated files
(cache/epub/<id>/):
  epub/<id>.epub           text-only EPUB (pg<id>.epub), every book
  epub-images/<id>.epub    illustrated EPUB3 (pg<id>-images-3.epub), only books in --illustrated
  covers/<id>.jpg          pg<id>.cover.medium.jpg, when Gutenberg has one
  missing/<id>             marker: no EPUB on the mirrors
index writes shelf-text.jsonl (per book: Gutenberg's description and subjects, for the hub's search) and shelf.json: categories (Gutenberg's "Category: …" bookshelves, under a few groups) and, per
book: id, t(itle), a(uthor), c(ategory indexes), r(ank by downloads), s(ize), si (illustrated size), cs (cover size). Rights-failed books are left out.
The EPUBs keep Gutenberg's licence; they are served as-is (free redistribution).
"""
import argparse, gzip, io, json, queue, re, sys, threading, time, urllib.error, urllib.request, zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from books_extract import author_name, clean_title, late_copyright, rights_ok, short_title  # noqa: E402

# (base URL, worker threads). Gutenberg asks bulk downloads to use mirrors; these carry cache/epub.
MIRRORS = [("https://gutenberg.nabasny.com", 6), ("https://gutenberg.pglaf.org", 3), ("https://aleph.pglaf.org", 2)]
UA = {"User-Agent": "survivalpedia-shelf/1.0 (offline library build; low rate)"}

GROUPS = {
    "Fiction": ["Novels", "Adventure", "Science-Fiction & Fantasy", "Historical Novels", "Romance",
                "Crime, Thrillers and Mystery", "Short Stories", "Humour", "Children & Young Adult Reading"],
    "Literature": ["Classics of Literature", "British Literature", "American Literature", "French Literature",
                   "German Literature", "Russian Literature", "Literature - Other", "Poetry", "Plays/Films/Dramas",
                   "Essays, Letters & Speeches", "Mythology, Legends & Folklore", "Journals"],
    "Practical": ["How To ...", "Cooking & Drinking", "Nature/Gardening/Animals", "Health & Medicine", "Nutrition",
                  "Science - Earth/Agricultural/Farming", "Engineering & Technology", "Sports/Hobbies",
                  "Architecture", "Parenthood & Family Relations", "Fashion", "Drugs/Alcohol/Pharmacology",
                  "Old Age & the Elderly"],
    "Science & reference": ["Science - Biology", "Science - Physics", "Science - Chemistry/Biochemistry", "Mathematics",
                            "Encyclopedias/Dictionaries/Reference", "Language & Communication",
                            "Research Methods/Statistics/Information Sys", "Environmental Issues",
                            "Archaeology & Anthropology", "Psychiatry/Psychology"],
    "History": ["History - Ancient", "History - Medieval/Middle Ages", "History - Early Modern (c. 1450-1750)",
                "History - Modern (1750+)", "History - American", "History - British", "History - European",
                "History - Warfare", "History - Religious", "History - Royalty", "History - Schools & Universities",
                "History - Other"],
    "Lives & travel": ["Biographies", "Travel Writing"],
    "Society & ideas": ["Philosophy & Ethics", "Religion/Spirituality", "Politics", "Economics", "Law & Criminology",
                        "Sociology", "Gender & Sexuality Studies", "Teaching & Education", "Business/Management",
                        "Journalism/Media/Writing", "Reports & Conference Proceedings", "Sexuality & Erotica"],
    "Arts": ["Art", "Music"],
}
OTHER = "Other"


def selected(catalog):
    """English texts passing the rights rule, most-downloaded first: (book, verdict)."""
    out = []
    for l in open(catalog):
        b = json.loads(l)
        if b.get("type") != "Text" or "en" not in b.get("languages", []):
            continue
        v = rights_ok(b)
        if v:
            out.append((b, v))
    out.sort(key=lambda bv: -(bv[0].get("downloads") or 0))
    return out


def get(url, tries=3):
    for k in range(tries):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return None
            time.sleep(3 * (k + 1))
        except Exception:
            time.sleep(3 * (k + 1))
    raise RuntimeError(f"failed: {url}")


def epub_text(data: bytes) -> str:
    """Plain-ish text of an EPUB's (X)HTML, for the copyright-notice check."""
    out = []
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        for n in z.namelist():
            if n.endswith((".html", ".xhtml", ".htm")):
                out.append(z.read(n).decode("utf-8", "replace"))
    return re.sub(r"<[^>]+>", " ", "\n".join(out))


def fetch(a):
    out = Path(a.shelf)
    for d in ("epub", "epub-images", "covers", "missing", "rights"):
        (out / d).mkdir(parents=True, exist_ok=True)
    illus = set(int(x) for x in open(a.illustrated).read().split()) if a.illustrated else set()
    books = selected(a.catalog)
    if a.limit:
        books = books[: a.limit]
    # pass 1: every text edition and cover; pass 2: the big illustrated editions
    q = queue.Queue()
    for bv in books:
        q.put((bv, False))
    for bv in books:
        if bv[0]["id"] in illus:
            q.put((bv, True))
    lock, t0, total = threading.Lock(), time.time(), q.qsize()
    st = {"ok": 0, "skip": 0, "missing": 0, "rights": 0, "err": 0, "bytes": 0}

    bases = [m for m, _ in MIRRORS]

    def mirror_get(first, rel):
        """GET rel from `first`, then the other mirrors (not every mirror has every generated file)."""
        for m in [first] + [x for x in bases if x != first]:
            data = get(f"{m}/{rel}")
            if data is not None:
                return data
        return None

    def one(base, b, v, illustrated):
        i = b["id"]
        d = f"cache/epub/{i}"
        if illustrated:
            dest = out / "epub-images" / f"{i}.epub"
            if dest.exists() or not (out / "epub" / f"{i}.epub").exists():
                return "skip", 0
            im = mirror_get(base, f"{d}/pg{i}-images-3.epub")
            if not im:
                return "missing", 0
            (out / "epub-images" / f"{i}.epub.tmp").write_bytes(im)
            (out / "epub-images" / f"{i}.epub.tmp").rename(dest)
            return "ok", len(im)
        if (out / "epub" / f"{i}.epub").exists() or (out / "missing" / str(i)).exists() or (out / "rights" / str(i)).exists():
            return "skip", 0
        # text-only EPUB; a few books only have the illustrated one
        data = mirror_get(base, f"{d}/pg{i}.epub") or mirror_get(base, f"{d}/pg{i}-images.epub")
        if data is None:
            (out / "missing" / str(i)).write_text("")
            return "missing", 0
        if v == "check" and late_copyright(epub_text(data)):
            (out / "rights" / str(i)).write_text("copyright notice after 1955\n")
            return "rights", 0
        n = len(data)
        (out / "epub" / f"{i}.epub.tmp").write_bytes(data)
        (out / "epub" / f"{i}.epub.tmp").rename(out / "epub" / f"{i}.epub")
        cover = out / "covers" / f"{i}.jpg"
        if not cover.exists():
            c = mirror_get(base, f"{d}/pg{i}.cover.medium.jpg")
            if c:
                cover.write_bytes(c)
                n += len(c)
        return "ok", n

    def worker(base):
        while True:
            try:
                (b, v), ill = q.get_nowait()
            except queue.Empty:
                return
            try:
                s, n = one(base, b, v, ill)
            except Exception as e:
                s, n = "err", 0
                print(f"{b['id']}: {e}", file=sys.stderr, flush=True)
            with lock:
                st[s] += 1
                st["bytes"] += n
                done = sum(st[k] for k in ("ok", "skip", "missing", "rights", "err"))
                if done % 500 == 0 or done == total:
                    print(f"{done}/{total} {st} {st['bytes'] / 1e6 / (time.time() - t0):.2f} MB/s", flush=True)

    threads = [threading.Thread(target=worker, args=(m,)) for m, k in MIRRORS for _ in range(k)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    print("done", json.dumps(st), flush=True)


def tidy(t):
    """Drop a bracket left open by cutting the subtitle ("Dictionary (part 1 of 4: A-D)")."""
    if t.count("(") > t.count(")") and " (" in t:
        t = t[: t.rfind(" (")]
    return t


def index(a):
    out = Path(a.shelf)
    illus = set(int(x) for x in open(a.illustrated).read().split()) if a.illustrated else set()
    names = [c for g in GROUPS.values() for c in g] + [OTHER]
    cat_ix = {c: k for k, c in enumerate(names)}
    books, counts, texts = [], [0] * len(names), []
    for rank, (b, v) in enumerate(selected(a.catalog), 1):
        i = b["id"]
        e = out / "epub" / f"{i}.epub"
        if not e.exists():
            continue
        if a.limit and len(books) >= a.limit:
            break  # a small shelf of the most-read books (trying out the app)
        cats = [s[10:] for s in b.get("bookshelves", []) if s.startswith("Category: ") and s[10:] in cat_ix] or [OTHER]
        cs = [cat_ix[c] for c in cats]
        for c in cs:
            counts[c] += 1
        summary = re.sub(r"\s*\(This is an automatically generated summary\.\)\s*$", "", b.get("summary") or "").strip()
        texts.append({"id": i, "d": summary, "sub": [x.split(" -- ")[0] for x in b.get("subjects", [])]})
        im = out / "epub-images" / f"{i}.epub"
        books.append({
            "id": i,
            "t": tidy(short_title(clean_title(b["title"]))),
            "a": author_name(b),
            "c": cs,
            "r": rank,
            "s": e.stat().st_size,
            **({"si": im.stat().st_size} if i in illus and im.exists() else {}),
            **({"cs": cv.stat().st_size} if (cv := out / "covers" / f"{i}.jpg").exists() else {}),
        })
    groups = [{"name": g, "categories": [cat_ix[c] for c in cs if counts[cat_ix[c]]]} for g, cs in GROUPS.items()]
    if counts[cat_ix[OTHER]]:
        groups.append({"name": OTHER, "categories": [cat_ix[OTHER]]})
    shelf = {
        "version": 1,
        "source": "Project Gutenberg (gutenberg.org); public domain, with the Project Gutenberg licence in each book",
        "categories": [{"name": n, "count": counts[k]} for k, n in enumerate(names)],
        "groups": groups,
        "books": books,
    }
    tmp = out / "shelf.json.tmp"
    tmp.write_text(json.dumps(shelf, ensure_ascii=False, separators=(",", ":")))
    tmp.rename(out / "shelf.json")
    # descriptions and subjects for the hub's book search (/api/books/search) and book details
    with open(out / "shelf-text.jsonl.tmp", "w") as f:
        for t in texts:
            f.write(json.dumps(t, ensure_ascii=False) + "\n")
    (out / "shelf-text.jsonl.tmp").rename(out / "shelf-text.jsonl")
    gz = out / "shelf.json.gz.tmp"
    gz.write_bytes(gzip.compress((out / "shelf.json").read_bytes(), 9))
    gz.rename(out / "shelf.json.gz")
    tot = sum(b["s"] for b in books)
    print(f"{len(books):,} books, {tot / 1e9:.1f} GB text, {sum(b.get('si', 0) for b in books) / 1e9:.1f} GB illustrated, "
          f"{sum(1 for b in books if 'cs' in b):,} covers; shelf.json {(out / 'shelf.json').stat().st_size / 1e6:.1f} MB")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["fetch", "index"])
    ap.add_argument("catalog")
    ap.add_argument("shelf")
    ap.add_argument("--illustrated")
    ap.add_argument("--limit", type=int)
    a = ap.parse_args()
    fetch(a) if a.cmd == "fetch" else index(a)
