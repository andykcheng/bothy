"""Cut a selection of articles out of an extract (zim_extract.py output) into a new extract dir, e.g. the Essentials
Wikipedia from the full one, so it can be embedded and built as its own collection.

  uv run python essentials/subset.py <src extract dir> <list.tsv> <out dir> [--images lead|all|none] [--key title]
      [--tier essentials]

<list.tsv> needs a page_id column (rerank.py output), or a title column with --key title (wikiHow has no page ids);
with --tier only rows whose tier column matches are used. If <src>/emb/ holds merged embeddings (chunk_ids.txt,
emb_f16.npy, manifest.json), the kept chunks' rows are copied to <out>/emb/ so the subset needs no re-embedding. Writes docs.jsonl, chunks.jsonl, redirects.jsonl (only those
pointing at kept articles), images.jsonl and images/ (copied), source.json (with "subset_of").
--images lead (default) keeps each article's first image and drops the other <img> tags (and figures left empty),
so the Essentials image component stays small; all keeps every image; none drops them all.
Editor-facing notes ("Please help improve Wikipedia", [citation needed], ...) are removed with wp_scrub.py; the
embeddings are reused as they are (the notes are a few words in a handful of chunks). Animated images over
--max-anim bytes (default 150 KB) become their first frame: ~640 animations were a quarter of the image bytes.
The full docs.jsonl is ~100 GB: lines are filtered by regex on the page id, and only kept lines are parsed.
"""
import argparse, csv, json, re, shutil, sys, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from wp_scrub import scrub_doc, scrub_text  # noqa: E402

csv.field_size_limit(sys.maxsize)
ap = argparse.ArgumentParser()
ap.add_argument("src"); ap.add_argument("list"); ap.add_argument("out")
ap.add_argument("--images", choices=["lead", "all", "none"], default="lead")
ap.add_argument("--key", choices=["page_id", "title"], default="page_id")
ap.add_argument("--tier")
ap.add_argument("--max-anim", type=int, default=150_000, help="animated images over this many bytes become stills")
a = ap.parse_args()
src, out = Path(a.src), Path(a.out)
out.mkdir(parents=True, exist_ok=True)

with open(a.list) as f:
    rows = [r for r in csv.DictReader(f, delimiter="\t") if not a.tier or r.get("tier") == a.tier]
    want = {int(r["page_id"]) for r in rows} if a.key == "page_id" else {r["title"] for r in rows}
print(f"{len(want):,} articles wanted", flush=True)

PID = re.compile(r'"page_id":\s*(\d+)') if a.key == "page_id" else re.compile(r'"title":\s*"((?:[^"\\]|\\.)*)"')
IMG = re.compile(r'<img src="img:([0-9a-f]+)"[^>]*>')
EMPTY_FIG = re.compile(r"<figure>(?:(?!<img|</figure>).)*</figure>", re.S)
BLOCK_ID = re.compile(r' id="(p\d+)"')

t0 = time.time()
doc_ids, paths, keep_img = set(), set(), set()
n = 0
with open(src / "docs.jsonl") as f, open(out / "docs.jsonl", "w") as o:
    for i, line in enumerate(f):
        m = PID.search(line, 0, 2000)
        if not m or (int(m.group(1)) if a.key == "page_id" else json.loads(f'"{m.group(1)}"')) not in want:
            continue
        d = scrub_doc(json.loads(line))
        imgs = d.get("images") or []
        if a.images != "all":
            keep = set(imgs[:1]) if a.images == "lead" else set()
            html = IMG.sub(lambda m: m.group(0) if m.group(1) in keep else "", d["html"])
            html = EMPTY_FIG.sub("", html)
            ids = set(BLOCK_ID.findall(html))
            d["blocks"] = [b for b in d["blocks"] if b["id"] in ids]
            d["html"], d["images"] = html, [k for k in imgs if k in keep]
        keep_img.update(d["images"])
        doc_ids.add(d["doc_id"]); paths.add(d["path"])
        o.write(json.dumps(d, ensure_ascii=False) + "\n"); n += 1
        if n % 20000 == 0:
            print(f"docs: {n:,} kept of {i + 1:,} read ({time.time() - t0:.0f}s)", flush=True)
print(f"docs: {n:,} kept ({time.time() - t0:.0f}s)", flush=True)

SCRUB_HINT = re.compile(r"Please help improve Wikipedia|citation needed|You can help", re.I)
DOC = re.compile(r'"doc_id":\s*"((?:[^"\\]|\\.)*)"')
nc = 0
with open(src / "chunks.jsonl") as f, open(out / "chunks.jsonl", "w") as o:
    for line in f:
        m = DOC.search(line, 0, 1000)
        if m and json.loads(f'"{m.group(1)}"') in doc_ids:
            if SCRUB_HINT.search(line):
                c = json.loads(line); c["text"] = scrub_text(c["text"]); line = json.dumps(c, ensure_ascii=False) + "\n"
            o.write(line); nc += 1
print(f"chunks: {nc:,} ({time.time() - t0:.0f}s)", flush=True)

nr = 0
with open(src / "redirects.jsonl") as f, open(out / "redirects.jsonl", "w") as o:
    for line in f:
        if json.loads(line)["to_path"] in paths:
            o.write(line); nr += 1
print(f"redirects: {nr:,} ({time.time() - t0:.0f}s)", flush=True)

def still_frame(src_p, dst_p):
    """An animated WebP over --max-anim bytes is replaced by its first frame (a few KB instead of up to 8 MB)."""
    from PIL import Image
    with Image.open(src_p) as im:
        if getattr(im, "n_frames", 1) < 2:
            return False
        im.seek(0)
        im.convert("RGBA").save(dst_p, "WEBP", quality=80)
    return True


ni = n_still = 0
(out / "images").mkdir(exist_ok=True)
with open(src / "images.jsonl") as f, open(out / "images.jsonl", "w") as o:
    for line in f:
        r = json.loads(line)
        if r["key"] not in keep_img:
            continue
        o.write(line); ni += 1
        k = r["key"]
        d = out / "images" / k[:2]; d.mkdir(exist_ok=True)
        for v in r["variants"]:
            name = f"{k}.svg" if v == "svg" else f"{k}.{v}.webp"
            p = src / "images" / k[:2] / name
            if p.exists() and not (d / name).exists():
                if name.endswith(".webp") and p.stat().st_size > a.max_anim and still_frame(p, d / name):
                    n_still += 1
                else:
                    shutil.copyfile(p, d / name)
print(f"images: {ni:,}, {n_still:,} large animations made stills ({time.time() - t0:.0f}s)", flush=True)

if (src / "emb" / "emb_f16.npy").exists():
    import numpy as np
    ids = (src / "emb" / "chunk_ids.txt").read_text().split("\n")
    rows_keep = [i for i, c in enumerate(ids) if c and c.split("#", 1)[0] in doc_ids]
    e = np.load(src / "emb" / "emb_f16.npy", mmap_mode="r")
    (out / "emb").mkdir(exist_ok=True)
    np.save(out / "emb" / "emb_f16.npy", np.ascontiguousarray(e[rows_keep]))
    (out / "emb" / "chunk_ids.txt").write_text("\n".join(ids[i] for i in rows_keep) + "\n")
    m = json.loads((src / "emb" / "manifest.json").read_text())
    m.update({"count": len(rows_keep), "subset_of": str(src / "emb")})
    (out / "emb" / "manifest.json").write_text(json.dumps(m, indent=2))
    print(f"embeddings: {len(rows_keep):,} rows copied ({time.time() - t0:.0f}s)", flush=True)

s = json.loads((src / "source.json").read_text())
s.update({"docs": n, "chunks": nc, "redirects": nr, "images_unique": ni,
          "subset_of": str(src), "subset_list": str(a.list), "subset_images": a.images})
(out / "source.json").write_text(json.dumps(s, indent=2, ensure_ascii=False))
print("done", json.dumps({"docs": n, "chunks": nc, "redirects": nr, "images": ni}))
