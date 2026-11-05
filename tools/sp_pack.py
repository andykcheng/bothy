#!/usr/bin/env python3
"""Reference reader for Survivalpedia packs (docs/DATA-FORMATS.md), in plain Python.

Needs only `zstandard` (pip install zstandard). The search indexes are not read here: BM25 is a Tantivy
index (use tantivy-py or the Rust code) and the vector index layout is described in DATA-FORMATS.md.

    python sp_pack.py <pack> info
    python sp_pack.py <pack> list [N]           first N titles (default 20)
    python sp_pack.py <pack> get <doc number>   the doc record as JSON
    python sp_pack.py <pack> image <key> <out>  write an image (320 px WebP, else 640, else SVG)
    python sp_pack.py <pack> dump <out.jsonl>   every doc record, one JSON object per line

As a library:
    from sp_pack import Pack
    p = Pack("packs/essentials")
    for n in range(p.doc_count):
        rec = p.record(n)          # dict: doc_id, title, url, license, html, ...
"""
from __future__ import annotations

import json
import struct
import sys
from pathlib import Path

import zstandard

LANGS = ["en", "zh", "ja"]
VARIANTS = ["320", "640", "svg"]
HEADER = 16  # magic(4) + version u32 + count u64


def _header(buf: bytes, magic: bytes) -> int:
    if buf[:4] != magic:
        raise ValueError(f"bad magic {buf[:4]!r}, expected {magic!r}")
    version, count = struct.unpack_from("<IQ", buf, 4)
    if version != 1:
        raise ValueError(f"unsupported format version {version}")
    return count


class Pack:
    def __init__(self, path):
        self.dir = Path(path)
        self.manifest = json.loads((self.dir / "manifest.json").read_text())
        self.sources = [s["id"] for s in self.manifest["sources"]]
        self._docs = (self.dir / "docs.idx").read_bytes()
        self.doc_count = _header(self._docs, b"SPDI")
        self._titles = (self.dir / "titles.bin").read_bytes()
        self._imgs = (self.dir / "images.idx").read_bytes()
        self.image_count = _header(self._imgs, b"SPII")
        self._chunks = (self.dir / "chunks.bin").read_bytes()
        self.chunk_count = _header(self._chunks, b"SPCI")
        self._dctx = {}
        for i, lang in enumerate(LANGS):
            f = self.dir / f"dict-{lang}.zst"
            if f.exists():
                d = zstandard.ZstdCompressionDict(f.read_bytes())  # raw-content dictionaries work too
                self._dctx[i] = zstandard.ZstdDecompressor(dict_data=d)
        self._block = (None, b"")

    # docs.idx: shard u16, lang u8, source u8, block_clen u32, block_off u64, rec_off u32, rec_len u32,
    #           title_off u32, title_len u32  (32 bytes)
    def entry(self, n: int) -> dict:
        if not 0 <= n < self.doc_count:
            raise IndexError(n)
        f = struct.unpack_from("<HBBIQIIII", self._docs, HEADER + 32 * n)
        return dict(zip(("shard", "lang", "source", "block_clen", "block_off", "rec_off", "rec_len",
                         "title_off", "title_len"), f))

    def title(self, n: int) -> str:
        e = self.entry(n)
        return self._titles[e["title_off"]:e["title_off"] + e["title_len"]].decode()

    def source(self, n: int) -> str:
        return self.sources[self.entry(n)["source"]]

    def _read(self, prefix: str, shard: int, off: int, length: int) -> bytes:
        with open(self.dir / f"{prefix}-{shard:03}.bin", "rb") as fh:
            fh.seek(off)
            return fh.read(length)

    def record(self, n: int) -> dict:
        """The doc's JSON record (the extract's docs.jsonl line without blocks, images and path)."""
        e = self.entry(n)
        key = (e["shard"], e["block_off"])
        if self._block[0] != key:
            comp = self._read("docs", e["shard"], e["block_off"], e["block_clen"])
            self._block = (key, self._dctx[e["lang"]].stream_reader(comp).read())
        raw = self._block[1][e["rec_off"]:e["rec_off"] + e["rec_len"]]
        return json.loads(raw)

    # images.idx: key [12], variant u8, pad u8, shard u16, len u32, off u64 (28 bytes), sorted by (key, variant)
    def image(self, key_hex: str, variants=("320", "640", "svg")) -> tuple[str, bytes] | None:
        key = bytes.fromhex(key_hex)
        lo, hi = 0, self.image_count
        while lo < hi:  # first entry with this key
            mid = (lo + hi) // 2
            if self._imgs[HEADER + 28 * mid:HEADER + 28 * mid + 12] < key:
                lo = mid + 1
            else:
                hi = mid
        found = {}
        i = lo
        while i < self.image_count:
            k, v, _, shard, length, off = struct.unpack_from("<12sBBHIQ", self._imgs, HEADER + 28 * i)
            if k != key:
                break
            found[VARIANTS[v]] = (shard, off, length)
            i += 1
        for v in variants:
            if v in found:
                shard, off, length = found[v]
                return v, self._read("img-core", shard, off, length)
        return None

    # chunks.bin: doc u32, block_first u32, block_last u32 (12 bytes); chunk ordinal = vector index id
    def chunk(self, i: int) -> tuple[int, int, int]:
        return struct.unpack_from("<III", self._chunks, HEADER + 12 * i)


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    p, cmd, args = Pack(sys.argv[1]), sys.argv[2], sys.argv[3:]
    if cmd == "info":
        m = p.manifest
        print(json.dumps({"collection": m.get("collection"), "sources": m["sources"], "counts": m["counts"],
                          "docs": p.doc_count, "images": p.image_count, "chunks": p.chunk_count}, indent=1))
    elif cmd == "list":
        for n in range(min(int(args[0]) if args else 20, p.doc_count)):
            print(n, p.source(n), p.title(n), sep="\t")
    elif cmd == "get":
        print(json.dumps(p.record(int(args[0])), indent=1, ensure_ascii=False))
    elif cmd == "image":
        got = p.image(args[0])
        if got is None:
            sys.exit("no such image")
        Path(args[1]).write_bytes(got[1])
        print(f"{got[0]}: {len(got[1])} bytes -> {args[1]}")
    elif cmd == "dump":
        with open(args[0], "w", encoding="utf-8") as fh:
            for n in range(p.doc_count):
                fh.write(json.dumps(p.record(n), ensure_ascii=False) + "\n")
        print(f"{p.doc_count} records -> {args[0]}")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
