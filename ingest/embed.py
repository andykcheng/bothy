"""Embed chunks with Qwen3-Embedding-0.6B (DESIGN §7).

    uv run python embed.py <extract_dir> [<extract_dir> ...] [--reuse <old_emb_dir> ...]

Reads <extract_dir>/chunks.jsonl, writes <extract_dir>/emb/:
    manifest.json     model, revision, dims, query instruction, pooling, quantisation
    chunk_ids.txt     one chunk_id per row, same order as the arrays
    hashes.npy        (N, 16) uint8  sha256(model|text)[:16], for reuse on later builds
    full_f16.npy      (N, 1024) f16  unit-norm full vectors (experiments, other dims)
    emb_f16.npy       (N, D) f16     first D dims, renormalised
    emb_i8.npy        (N, D) int8    round(x / i8_scale * 127), per-dimension scale
    i8_scale.npy      (D,) f32
    emb_bin.npy       (N, D/8) uint8 packed sign bits (np.packbits(x > 0), big-endian bit order)

Documents get no instruction prefix; queries use QUERY_INSTRUCTION (pinned).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
from pathlib import Path

import numpy as np
import orjson
import torch
import torch.nn.functional as F
from tqdm import tqdm

MODEL = "Qwen/Qwen3-Embedding-0.6B"
DIMS = 256
MAX_TOKENS = 512  # chunks target 256 tokens + title/section line; longer inputs are truncated
QUERY_INSTRUCTION = "Instruct: Given a question, retrieve passages that answer it\nQuery: "


def text_hash(text: str) -> bytes:
    return hashlib.sha256(f"{MODEL}|{text}".encode()).digest()[:16]


class Embedder:
    def __init__(self, device: str | None = None):
        from transformers import AutoModel, AutoTokenizer

        device = device or ("cuda" if torch.cuda.is_available() else "cpu")   # CPU works, just slowly

        self.tok = AutoTokenizer.from_pretrained(MODEL, padding_side="left", local_files_only=True)
        self.model = AutoModel.from_pretrained(
            MODEL, dtype=torch.bfloat16, attn_implementation="sdpa", local_files_only=True
        ).to(device).eval()
        self.device = device
        from huggingface_hub import snapshot_download

        self.revision = Path(snapshot_download(MODEL, local_files_only=True)).name

    @torch.inference_mode()
    def encode(self, texts: list[str]) -> np.ndarray:
        """Full-dimension, L2-normalised float32 vectors (last-token pooling, left padding)."""
        batch = self.tok(texts, padding=True, truncation=True, max_length=MAX_TOKENS,
                         return_tensors="pt").to(self.device)
        hidden = self.model(**batch).last_hidden_state
        vec = hidden[:, -1].float()  # left padding: the last position is the final token
        return F.normalize(vec, dim=-1).cpu().numpy()

    def encode_query(self, q: str) -> np.ndarray:
        return self.encode([QUERY_INSTRUCTION + q])[0]


class HttpEmbedder:
    """OpenAI-compatible /v1/embeddings (llama.cpp server with the same model, last pooling).
    llama-server appends <|endoftext|> itself; vectors match torch at cosine ~0.9999."""

    def __init__(self, url: str, model: str, concurrency: int = 4, timeout: float = 600):
        from concurrent.futures import ThreadPoolExecutor
        from transformers import AutoTokenizer

        self.tok = AutoTokenizer.from_pretrained(MODEL, local_files_only=True)  # length sorting only
        self.url, self.model_name = url.rstrip("/") + "/embeddings", model
        self.pool = ThreadPoolExecutor(concurrency)
        self.revision = f"http:{model}"
        self.hidden_size = 1024
        self.timeout = timeout

    def _post(self, texts: list[str]) -> np.ndarray:
        import urllib.error
        import urllib.request
        body = json.dumps({"model": self.model_name, "input": texts}).encode()
        for attempt in range(8):
            req = urllib.request.Request(self.url, data=body, headers={"Content-Type": "application/json"})
            try:
                with urllib.request.urlopen(req, timeout=self.timeout) as r:
                    data = json.load(r)["data"]
                break
            except urllib.error.HTTPError as e:  # queue full (TEI answers 429): back off, retry
                if e.code not in (429, 503) or attempt == 7:
                    raise
                time.sleep(0.25 * 2 ** attempt)
        v = np.array([d["embedding"] for d in sorted(data, key=lambda d: d["index"])], dtype=np.float32)
        return v / np.linalg.norm(v, axis=1, keepdims=True).clip(1e-12)

    def encode(self, texts: list[str]) -> np.ndarray:
        # same truncation as the torch path (MAX_TOKENS includes the <|endoftext|> the server adds)
        ids = self.tok(texts, add_special_tokens=False, truncation=True, max_length=MAX_TOKENS - 1)["input_ids"]
        texts = [t if len(i) < MAX_TOKENS - 1 else self.tok.decode(i) for t, i in zip(texts, ids)]
        # split the batch into a few concurrent requests so the server's slots stay busy
        parts = [texts[i:i + 32] for i in range(0, len(texts), 32)]
        return np.concatenate(list(self.pool.map(self._post, parts)))


def truncate(full: np.ndarray, dims: int) -> np.ndarray:
    v = full[:, :dims].astype(np.float32)
    return v / np.linalg.norm(v, axis=1, keepdims=True).clip(1e-12)


def quantise(v: np.ndarray):
    scale = np.abs(v).max(axis=0).astype(np.float32).clip(1e-6)
    i8 = np.clip(np.rint(v / scale * 127), -127, 127).astype(np.int8)
    bits = np.packbits(v > 0, axis=1)
    return i8, scale, bits


def load_reuse(dirs: list[str]) -> dict[bytes, np.ndarray]:
    cache: dict[bytes, np.ndarray] = {}
    for d in dirs:
        d = Path(d)
        if (d / "manifest.json").exists() and json.loads((d / "manifest.json").read_text())["model"] != MODEL:
            continue
        hashes = np.load(d / "hashes.npy")
        full = np.load(d / "full_f16.npy", mmap_mode="r")
        for i, h in enumerate(hashes):
            cache[h.tobytes()] = full[i]
    return cache


def load_inputs(src: Path, max_chars: int, lead_chars: int) -> tuple[list[str], list[str]]:
    """(chunk_ids, texts). lead_chars > 0: one input per doc, the first lead_chars characters
    of the doc's chunks, keyed by its first chunk_id. max_chars > 0: truncate each chunk."""
    ids, texts = [], []
    lead_doc, lead_parts = None, []

    def flush_lead():
        if lead_doc is not None:
            title = lead_parts[0].split("\n", 1)[0].split(" > ")[0]
            body = " ".join(t.split("\n", 1)[-1] for t in lead_parts)
            texts.append(f"{title}\n{body}"[:lead_chars])

    with open(src / "chunks.jsonl", "rb") as f:
        for line in f:
            c = orjson.loads(line)
            if lead_chars:
                if c["doc_id"] != lead_doc:
                    flush_lead()
                    lead_doc, lead_parts = c["doc_id"], []
                    ids.append(c["chunk_id"])
                if sum(map(len, lead_parts)) < lead_chars:
                    lead_parts.append(c["text"])
            else:
                ids.append(c["chunk_id"])
                texts.append(c["text"][:max_chars] if max_chars else c["text"])
    if lead_chars:
        flush_lead()
    return ids, texts


def embed_texts(emb: Embedder, texts: list[str], batch_tokens: int, cache: dict[bytes, np.ndarray],
                desc: str, tqdm_args: dict | None = None) -> tuple[np.ndarray, np.ndarray, list[int], int]:
    """(hashes, full f16 vectors, token lengths of the embedded texts, number embedded)."""
    n = len(texts)
    hashes = np.frombuffer(b"".join(text_hash(t) for t in texts), dtype=np.uint8).reshape(n, 16)
    full = np.zeros((n, getattr(emb, "hidden_size", None) or emb.model.config.hidden_size), dtype=np.float16)
    todo = []
    for i in range(n):
        hit = cache.get(hashes[i].tobytes())
        if hit is not None:
            full[i] = hit
        else:
            todo.append(i)

    # Length-sorted batches with a token budget keep padding and memory low.
    lens = [len(x) for x in emb.tok([texts[i] for i in todo], add_special_tokens=False,
                                    truncation=True, max_length=MAX_TOKENS)["input_ids"]]
    order = [todo[j] for j in np.argsort(lens)[::-1]]
    sorted_lens = sorted(lens, reverse=True)
    batches, cur, width = [], [], 0
    for i, ln in zip(order, sorted_lens):  # longest first: a batch's first item sets its width
        if cur and width * (len(cur) + 1) > batch_tokens:
            batches.append(cur)
            cur = []
        if not cur:
            width = ln + 2
        cur.append(i)
    if cur:
        batches.append(cur)
    for b in tqdm(batches, desc=desc, unit="batch", **(tqdm_args or {})):
        full[b] = emb.encode([texts[i] for i in b]).astype(np.float16)
    return hashes, full, lens, len(todo)


def embed_dir(emb: Embedder, src: Path, batch_tokens: int, cache: dict[bytes, np.ndarray],
              out_name: str = "emb", max_chars: int = 0, lead_chars: int = 0):
    t0 = time.time()
    ids, texts = load_inputs(src, max_chars, lead_chars)
    n = len(texts)
    hashes, full, lens, embedded = embed_texts(emb, texts, batch_tokens, cache, src.name)
    todo = range(embedded)

    out = src / out_name
    out.mkdir(exist_ok=True)
    v = truncate(full, DIMS)
    i8, scale, bits = quantise(v)
    np.save(out / "hashes.npy", hashes)
    np.save(out / "full_f16.npy", full)
    np.save(out / "emb_f16.npy", v.astype(np.float16))
    np.save(out / "emb_i8.npy", i8)
    np.save(out / "i8_scale.npy", scale)
    np.save(out / "emb_bin.npy", bits)
    (out / "chunk_ids.txt").write_text("\n".join(ids) + "\n")
    secs = time.time() - t0
    manifest = {
        "model": MODEL, "model_revision": emb.revision, "dtype": "bfloat16" if isinstance(emb, Embedder) else "gguf-f16", "pooling": "last_token",
        "normalize": "l2", "dims": DIMS, "full_dims": int(full.shape[1]), "max_tokens": MAX_TOKENS,
        "document_prefix": "", "query_instruction": QUERY_INSTRUCTION,
        "int8": "per-dimension symmetric: round(x / i8_scale * 127)",
        "binary": "np.packbits(x > 0, axis=1)",
        "input": {"max_chars": max_chars, "lead_chars": lead_chars, "max_tokens": MAX_TOKENS},
        "tokens": int(sum(lens)), "tokens_per_sec": round(sum(lens) / secs, 1) if todo else None,
        "count": n, "reused": n - len(todo), "embedded": len(todo), "seconds": round(secs, 1),
        "chunks_per_sec": round(len(todo) / secs, 1) if todo else None,
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps({"dir": str(src), **{k: manifest[k] for k in ("count", "reused", "embedded", "seconds", "chunks_per_sec", "tokens", "tokens_per_sec")}}))


def shard_index(path: Path, shard_size: int, cache: Path) -> list[int]:
    """Byte offsets of every shard_size-th line of path: shard k is offsets[k]..offsets[k+1].
    Cached in `cache` (one scan of a 40 GB file takes a few minutes on the USB disk)."""
    st = path.stat()
    key = {"size": st.st_size, "mtime_ns": st.st_mtime_ns, "shard_size": shard_size}
    if cache.exists() and (j := json.loads(cache.read_text()))["key"] == key:
        return j["offsets"]
    offsets, lines, pos = [0], 0, 0
    with open(path, "rb") as f:
        while blk := f.read(64 << 20):
            nl = np.flatnonzero(np.frombuffer(blk, np.uint8) == 10)
            js = np.arange(shard_size - lines % shard_size - 1, len(nl), shard_size)
            offsets += (pos + nl[js] + 1).tolist()
            lines += len(nl)
            pos += len(blk)
    if offsets[-1] != pos:
        offsets.append(pos)
    cache.write_text(json.dumps({"key": key, "offsets": offsets}) + "\n")
    return offsets


class ShardQueue:
    """Shards left to embed. The local GPU takes from the front, the helper from the back; a
    failed helper shard goes back. When the queue is empty, the GPU also runs shards the helper
    still has in flight, and whichever copy finishes first is kept."""

    def __init__(self, todo: list[int]):
        import threading
        from collections import deque

        self.todo, self.lock = deque(todo), threading.Lock()
        self.remote: set[int] = set()  # in flight on the helper
        self.doubled: set[int] = set()  # in flight on the helper and also taken by the GPU
        self.done: set[int] = set()

    def take(self, helper: bool) -> int | None:
        with self.lock:
            if self.todo:
                k = self.todo.pop() if helper else self.todo.popleft()
                if helper:
                    self.remote.add(k)
                return k
            if not helper:
                for k in sorted(self.remote - self.doubled - self.done):
                    self.doubled.add(k)
                    return k
            return None

    def finish(self, k: int, helper: bool, ok: bool):
        with self.lock:
            if ok:
                self.done.add(k)
            if helper:
                self.remote.discard(k)
                if not ok and k not in self.done and k not in self.doubled:
                    self.todo.append(k)
            else:
                self.doubled.discard(k)

    def busy(self) -> bool:
        with self.lock:
            return bool(self.todo or self.remote - self.done)


def run_shard(emb, src: Path, sdir: Path, offsets: list[int], k: int, batch_tokens: int,
              cache: dict[bytes, np.ndarray], who: str) -> bool:
    """Embeds shard k; False if another worker finished it first."""
    import shutil

    t0 = time.time()
    with open(src / "chunks.jsonl", "rb") as f:
        f.seek(offsets[k])
        raw = f.read(offsets[k + 1] - offsets[k])
    rows = [orjson.loads(line) for line in raw.splitlines()]
    ids, texts = [c["chunk_id"] for c in rows], [c["text"] for c in rows]
    hashes, full, lens, embedded = embed_texts(
        emb, texts, batch_tokens, cache, f"{src.name} {k:05d}/{len(offsets) - 2:05d} {who}", {"mininterval": 60})
    tmp = sdir / f"{k:05d}.tmp-{who}"
    tmp.mkdir(exist_ok=True)
    np.save(tmp / "hashes.npy", hashes)
    np.save(tmp / "full_f16.npy", full)
    (tmp / "chunk_ids.txt").write_text("\n".join(ids) + "\n")
    secs = time.time() - t0
    meta = {"count": len(ids), "embedded": embedded, "start_offset": offsets[k], "end_offset": offsets[k + 1],
            "tokens": int(sum(lens)), "seconds": round(secs, 1), "chunks_per_sec": round(len(ids) / secs, 1),
            "engine": emb.revision}
    (tmp / "meta.json").write_text(json.dumps(meta) + "\n")
    try:
        tmp.rename(sdir / f"{k:05d}")
    except OSError:  # the other worker got there first
        shutil.rmtree(tmp)
        return False
    print(json.dumps({"shard": k, "of": len(offsets) - 1, "by": who, **meta}), flush=True)
    return True


def helper_target(args) -> tuple[str, str, int, int] | None:
    """(url, model, concurrent requests, batch tokens) of the helper, or None while paused. With --helper-file the file is re-read
    every minute, so the helper can be paused or switched without restarting the job:
    {"url": "http://host:port/v1", "model": "...", "concurrency": 16, "batch_tokens": 262144}; missing, unreadable or "paused": true = paused."""
    if not args.helper_file:
        return (args.helper_url, args.helper_model, args.concurrency, args.batch_tokens) if args.helper_url else None
    try:
        j = json.loads(Path(args.helper_file).read_text())
    except (OSError, ValueError):
        return None
    if j.get("paused") or not j.get("url"):
        return None
    # a batch is split into requests of 32 texts; bigger batches keep more requests in flight
    return (j["url"], j.get("model", args.helper_model), int(j.get("concurrency", args.concurrency)),
            int(j.get("batch_tokens", 4 * args.batch_tokens)))


def helper_loop(q: ShardQueue, src: Path, sdir: Path, offsets: list[int], cache: dict[bytes, np.ndarray],
                args, ref_texts: list[str], ref_vecs: np.ndarray):
    """Uses the helper machine whenever it answers. It must reproduce the local vectors on
    reference texts (cosine >= 0.995) before it gets any work; a server that fails the check is
    not used again (unless --helper-file switches to a different url or model)."""
    import shutil

    emb, target, checked, rejected, last_log = None, None, set(), set(), ""

    def log(msg: str):
        nonlocal last_log
        if msg != last_log:
            print(f"helper: {msg}", flush=True)
            last_log = msg

    while q.busy():
        t = helper_target(args)
        if t != target:
            emb, target = None, t
        if t is None or t in rejected:
            log("paused" if t is None else f"{t[0]} rejected (vectors differ); waiting for --helper-file to change")
            time.sleep(60)
            continue
        url, model, conc, helper_batch = t
        if emb is None:
            try:
                e = HttpEmbedder(url, model, conc, timeout=10)
                e._post(["ping"])  # up, and serving the model
                e.timeout = 120
                if t not in checked:
                    cos = (e.encode(ref_texts) * ref_vecs).sum(axis=1)
                    if cos.min() < 0.995:
                        log(f"{url} {model}: vectors differ from local (min cosine {cos.min():.4f}); not using it")
                        rejected.add(t)
                        continue
                    print(f"helper: {url} {model}: vectors match (min cosine {cos.min():.5f})", flush=True)
                    checked.add(t)
                emb = e
                log(f"{url}: up ({conc} concurrent requests), taking shards from the back")
            except Exception as ex:
                log(f"{url}: not available ({ex}); retrying every minute")
                time.sleep(60)
                continue
        k = q.take(helper=True)
        if k is None:
            time.sleep(10)
            continue
        try:
            q.finish(k, True, run_shard(emb, src, sdir, offsets, k, helper_batch, cache, "helper"))
        except Exception as ex:
            q.finish(k, True, False)
            log(f"{url}: lost during shard {k} ({ex}); shard requeued")
            emb = None
            for tmp in sdir.glob(f"{k:05d}.tmp-helper"):
                shutil.rmtree(tmp, ignore_errors=True)


def embed_sharded(emb: Embedder, src: Path, cache: dict[bytes, np.ndarray], args):
    """Large sources (full Wikipedia): resumable fixed shards of args.shard_size chunks, each
    written atomically to <out>/shards/NNNNN/ (chunk_ids.txt, hashes.npy, full_f16.npy, meta.json).
    A rerun skips finished shards. With --helper-url/--helper-file, a second machine joins whenever it is up.
    When every shard is done the final files are merged; full_f16 stays in the shards."""
    import shutil
    import threading

    out = src / args.out_name
    sdir = out / "shards"
    sdir.mkdir(parents=True, exist_ok=True)
    for t in sdir.glob("*.tmp*"):
        shutil.rmtree(t)
    offsets = shard_index(src / "chunks.jsonl", args.shard_size, sdir / "index.json")
    n = len(offsets) - 1
    todo = [k for k in range(n) if not (sdir / f"{k:05d}" / "meta.json").exists()]
    print(f"{src}: {n} shards, {n - len(todo)} done", flush=True)
    q = ShardQueue(todo)
    helper = None
    if (args.helper_url or args.helper_file) and todo:
        with open(src / "chunks.jsonl", "rb") as f:
            ref_texts = [orjson.loads(f.readline())["text"] for _ in range(16)]
        ref_vecs = emb.encode(ref_texts)
        helper = threading.Thread(target=helper_loop, daemon=True,
                                  args=(q, src, sdir, offsets, cache, args, ref_texts, ref_vecs))
        helper.start()
    while q.busy():
        k = q.take(helper=False)
        if k is None:
            time.sleep(10)
            continue
        q.finish(k, False, run_shard(emb, src, sdir, offsets, k, args.batch_tokens, cache, "local"))
    if helper:
        helper.join(timeout=150)
    missing = [k for k in range(n) if not (sdir / f"{k:05d}" / "meta.json").exists()]
    if missing:
        raise SystemExit(f"{src}: shards missing after the run: {missing[:10]}")
    merge_shards(emb, out, n)


def merge_shards(emb: Embedder, out: Path, n_shards: int):
    from numpy.lib.format import open_memmap

    shards = [out / "shards" / f"{k:05d}" for k in range(n_shards)]
    metas = [json.loads((p / "meta.json").read_text()) for p in shards]
    n = sum(m["count"] for m in metas)
    # pass 1: per-dimension int8 scale over all vectors
    scale = np.zeros(DIMS, dtype=np.float32)
    for p in shards:
        scale = np.maximum(scale, np.abs(truncate(np.load(p / "full_f16.npy", mmap_mode="r"), DIMS)).max(axis=0))
    scale = scale.clip(1e-6)
    hashes = open_memmap(out / "hashes.npy", "w+", np.uint8, (n, 16))
    f16 = open_memmap(out / "emb_f16.npy", "w+", np.float16, (n, DIMS))
    i8 = open_memmap(out / "emb_i8.npy", "w+", np.int8, (n, DIMS))
    bits = open_memmap(out / "emb_bin.npy", "w+", np.uint8, (n, DIMS // 8))
    row = 0
    with open(out / "chunk_ids.txt", "w") as ids:
        for p, m in zip(shards, metas):
            v = truncate(np.load(p / "full_f16.npy", mmap_mode="r"), DIMS)
            r = slice(row, row + m["count"])
            hashes[r] = np.load(p / "hashes.npy")
            f16[r] = v.astype(np.float16)
            i8[r] = np.clip(np.rint(v / scale * 127), -127, 127).astype(np.int8)
            bits[r] = np.packbits(v > 0, axis=1)
            ids.write((p / "chunk_ids.txt").read_text())
            row += m["count"]
    for a in (hashes, f16, i8, bits):
        a.flush()
    np.save(out / "i8_scale.npy", scale)
    secs = sum(m["seconds"] for m in metas)
    tokens = sum(m["tokens"] for m in metas)
    embedded = sum(m["embedded"] for m in metas)
    manifest = {
        "model": MODEL, "model_revision": emb.revision, "dtype": "bfloat16" if isinstance(emb, Embedder) else "gguf-f16",
        "pooling": "last_token", "normalize": "l2", "dims": DIMS,
        "full_dims": getattr(emb, "hidden_size", None) or emb.model.config.hidden_size, "max_tokens": MAX_TOKENS,
        "document_prefix": "", "query_instruction": QUERY_INSTRUCTION,
        "int8": "per-dimension symmetric: round(x / i8_scale * 127)",
        "binary": "np.packbits(x > 0, axis=1)",
        "input": {"max_chars": 0, "lead_chars": 0, "max_tokens": MAX_TOKENS},
        "full_f16": "per shard: shards/NNNNN/full_f16.npy (in chunk_ids.txt order)",
        "engines": sorted({m.get("engine", emb.revision) for m in metas}),
        "shards": len(shards), "tokens": tokens, "tokens_per_sec": round(tokens / secs, 1),
        "count": n, "reused": n - embedded, "embedded": embedded, "seconds": round(secs, 1),
        "chunks_per_sec": round(n / secs, 1),
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps({"dir": str(out), **{k: manifest[k] for k in ("count", "shards", "seconds", "chunks_per_sec", "tokens_per_sec")}}), flush=True)


def main():
    global MAX_TOKENS
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--reuse", nargs="*", default=[], help="earlier emb/ dirs whose vectors may be reused")
    ap.add_argument("--batch-tokens", type=int, default=64 * 1024)
    ap.add_argument("--out-name", default="emb", help="output subdir (experiments: e.g. emb-lead1000)")
    ap.add_argument("--max-chars", type=int, default=0, help="truncate each chunk to N characters")
    ap.add_argument("--lead-chars", type=int, default=0, help="one vector per doc from its first N characters")
    ap.add_argument("--max-tokens", type=int, default=MAX_TOKENS, help="truncate inputs to N tokens")
    ap.add_argument("--engine", choices=["torch", "http"], default="torch")
    ap.add_argument("--url", default="http://localhost:19000/v1")
    ap.add_argument("--http-model", default="qwen3-embedding-0.6b-f16")
    ap.add_argument("--concurrency", type=int, default=4)
    ap.add_argument("--shard-size", type=int, default=0,
                    help="resumable shards of N chunks (large sources); full_f16 stays per shard")
    ap.add_argument("--helper-url", default="", help="with --shard-size: OpenAI-compatible server used "
                    "alongside the local GPU whenever it answers, e.g. http://another-machine:11434/v1")
    ap.add_argument("--helper-model", default="qwen3-embedding:0.6b")
    ap.add_argument("--helper-file", default="", help='JSON {"url": ..., "model": ...} re-read every minute; '
                    'missing or {"paused": true} pauses the helper (overrides --helper-url)')
    args = ap.parse_args()
    MAX_TOKENS = args.max_tokens
    emb = Embedder() if args.engine == "torch" else HttpEmbedder(args.url, args.http_model, args.concurrency)
    cache = load_reuse(args.reuse)
    for d in args.dirs:
        if args.shard_size:
            embed_sharded(emb, Path(d), cache, args)
        else:
            embed_dir(emb, Path(d), args.batch_tokens, cache, args.out_name, args.max_chars, args.lead_chars)


if __name__ == "__main__":
    sys.exit(main())
