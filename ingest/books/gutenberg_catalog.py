"""Project Gutenberg RDF catalogue -> one JSON line per book.

  uv run python books/gutenberg_catalog.py <dir with cache/epub/*/pg*.rdf> <out.jsonl>

The catalogue is https://www.gutenberg.org/cache/epub/feeds/rdf-files.tar.bz2 (extract it first).
Fields: id, title, authors [{name, birth, death}], languages, type, subjects (LCSH), lcc, bookshelves,
downloads (last 30 days), issued, rights, summary (marc520), reading_ease (marc908), files [{url, mime, size}].
"""
import json, sys
import xml.etree.ElementTree as ET
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

NS = {
    "rdf": "http://www.w3.org/1999/02/22-rdf-syntax-ns#", "dcterms": "http://purl.org/dc/terms/",
    "pgterms": "http://www.gutenberg.org/2009/pgterms/", "dcam": "http://purl.org/dc/dcam/",
}
R = "{%s}" % NS["rdf"]


def text(e, path):
    x = e.find(path, NS)
    return x.text.strip() if x is not None and x.text else None


def parse(p):
    try:
        root = ET.parse(p).getroot()
    except ET.ParseError:
        return None
    b = root.find("pgterms:ebook", NS)
    if b is None:
        return None
    bid = int(b.get(R + "about").split("/")[-1])
    authors = []
    for a in b.findall("dcterms:creator/pgterms:agent", NS):
        authors.append({"name": text(a, "pgterms:name"), "birth": text(a, "pgterms:birthdate"), "death": text(a, "pgterms:deathdate")})
    subjects, lcc = [], []
    for s in b.findall("dcterms:subject/rdf:Description", NS):
        kind = s.find("dcam:memberOf", NS).get(R + "resource").rsplit("/", 1)[-1]
        v = text(s, "rdf:value")
        (subjects if kind == "LCSH" else lcc if kind == "LCC" else []).append(v)
    files = []
    for f in b.findall("dcterms:hasFormat/pgterms:file", NS):
        mimes = [text(m, "rdf:value") for m in f.findall("dcterms:format/rdf:Description", NS)]
        files.append({"url": f.get(R + "about"), "mime": next((m for m in mimes if m), None),
                      "size": int(text(f, "dcterms:extent") or 0)})
    return {
        "id": bid,
        "title": text(b, "dcterms:title"),
        "authors": authors,
        "languages": [text(l, "rdf:value") for l in b.findall("dcterms:language/rdf:Description", NS)],
        "type": text(b, "dcterms:type/rdf:Description/rdf:value"),
        "subjects": subjects, "lcc": lcc,
        "bookshelves": [text(s, "rdf:value") for s in b.findall("pgterms:bookshelf/rdf:Description", NS)],
        "downloads": int(text(b, "pgterms:downloads") or 0),
        "issued": text(b, "dcterms:issued"),
        "rights": text(b, "dcterms:rights"),
        "summary": text(b, "pgterms:marc520"),
        "reading_ease": text(b, "pgterms:marc908"),
        "files": files,
    }


if __name__ == "__main__":
    src, out = Path(sys.argv[1]), sys.argv[2]
    paths = sorted(src.glob("cache/epub/*/pg*.rdf"))
    n = 0
    with ProcessPoolExecutor() as ex, open(out, "w") as f:
        for r in ex.map(parse, paths, chunksize=500):
            if r:
                f.write(json.dumps(r, ensure_ascii=False) + "\n"); n += 1
    print(f"{n} books from {len(paths)} RDF files -> {out}")
