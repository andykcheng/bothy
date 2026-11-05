"""Local Jev/SystemOne-compatible decision server for Cloudflare Clef / Clef-flash.

  uv run python essentials/clef_server.py --model-dir ~/sp-scratch/models/clef-flash [--port 19100] [--batch 16]

POST /v1/systemone  takes the same body as https://api.typesafe.ai/v1/systemone ({model, state, questions}) and
                    returns {model, answers, usage}, so jev_classify.py --url http://<host>:19100/v1/systemone works.
GET  /health        model name, requests served, batches, queue length, tokens/s.
Concurrent requests are gathered into batches (up to --batch, waiting at most --wait-ms) and run in one forward
pass, sorted by length to keep padding small. Text-only (no images/videos). Shares the GPU with other jobs.
"""
import argparse, json, queue, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import torch

ap = argparse.ArgumentParser()
ap.add_argument("--model-dir", required=True)
ap.add_argument("--host", default="0.0.0.0")
ap.add_argument("--port", type=int, default=19100)
ap.add_argument("--batch", type=int, default=16)
ap.add_argument("--wait-ms", type=float, default=15)
ap.add_argument("--max-length", type=int, default=4096, help="max tokens per request (state is truncated to fit)")
a = ap.parse_args()

MODEL_DIR = Path(a.model_dir).expanduser()
sys.path.insert(0, str(MODEL_DIR))
from joint_schema_model import QUESTION_TYPES, ClefModel, JointSchemaHead, collate_records, encode_record, systemone_answer  # noqa: E402
from safetensors.torch import load_file  # noqa: E402
from transformers import AutoTokenizer, Qwen3_5ForConditionalGeneration  # noqa: E402

t0 = time.time()
# Same as joint_schema_model.load_release_model, but text-only: the tokenizer instead of the image/video
# processor (which needs torchvision).
backbone = Qwen3_5ForConditionalGeneration.from_pretrained(MODEL_DIR, dtype=torch.bfloat16, device_map={"": "cuda"})
backbone.config.use_cache = False
head = JointSchemaHead(**json.loads((MODEL_DIR / "joint_head_config.json").read_text()))
head.load_state_dict(load_file(MODEL_DIR / "joint_head.safetensors"), strict=True)
model = ClefModel(backbone, head.to(device="cuda", dtype=torch.bfloat16)).eval()
tok = AutoTokenizer.from_pretrained(MODEL_DIR)
DEVICE = next(model.parameters()).device
NAME = MODEL_DIR.name
print(f"{NAME} loaded in {time.time() - t0:.0f}s", flush=True)

jobs: "queue.Queue[dict]" = queue.Queue()
stats = {"model": NAME, "requests": 0, "batches": 0, "errors": 0, "tokens": 0, "busy_s": 0.0, "started": time.time()}
lock = threading.Lock()


def validate(req):
    qs = req.get("questions")
    if "state" not in req:
        raise ValueError("state is required")
    if not isinstance(qs, dict) or not qs:
        raise ValueError("at least one question is required")
    for qid, q in qs.items():
        if q.get("type") not in QUESTION_TYPES:
            raise ValueError(f"{qid}: type must be noul, choice, or score")
        if q["type"] != "noul" and not q.get("criteria"):
            raise ValueError(f"{qid}: criteria must not be empty")
    if req.get("images") or req.get("videos"):
        raise ValueError("images and videos are not supported by this server")


def worker():
    while True:
        batch = [jobs.get()]
        deadline = time.time() + a.wait_ms / 1000
        while len(batch) < a.batch:
            try:
                batch.append(jobs.get(timeout=max(0.0, deadline - time.time())))
            except queue.Empty:
                break
        batch.sort(key=lambda j: len(j["enc"].input_ids))
        t = time.time()
        try:
            with torch.inference_mode():
                logits = model(collate_records([j["enc"] for j in batch], tok.pad_token_id, DEVICE))
            for j, lg in zip(batch, logits):
                qs = j["req"]["questions"]
                j["resp"] = {"model": j["req"].get("model") or NAME, "answers": {
                    q.question_id: systemone_answer(qs[q.question_id], dict(zip(q.option_ids, ql.float().softmax(-1).tolist())))
                    for q, ql in zip(j["enc"].questions, lg)},
                    "usage": {"input_tokens": len(j["enc"].input_ids), "output_tokens": 0}}
        except Exception as e:  # e.g. out of memory: fail this batch, keep serving
            for j in batch:
                j["error"] = f"{type(e).__name__}: {e}"[:300]
            torch.cuda.empty_cache()
        with lock:
            stats["batches"] += 1; stats["busy_s"] += time.time() - t
            stats["requests"] += len(batch); stats["tokens"] += sum(len(j["enc"].input_ids) for j in batch)
        for j in batch:
            j["done"].set()


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code); self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)

    def do_GET(self):
        if self.path.rstrip("/") in ("/health", ""):
            with lock:
                s = dict(stats)
            s["queue"] = jobs.qsize()
            s["tokens_per_busy_s"] = round(s["tokens"] / s["busy_s"]) if s["busy_s"] else None
            s["requests_per_busy_s"] = round(s["requests"] / s["busy_s"], 2) if s["busy_s"] else None
            return self.reply(200, s)
        if self.path.rstrip("/") == "/v1/models":
            return self.reply(200, {"data": [{"id": NAME}]})
        self.reply(404, {"error": "not found"})

    def do_POST(self):
        if self.path.rstrip("/") != "/v1/systemone":
            return self.reply(404, {"error": "not found"})
        try:
            req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
            validate(req)
            job = {"req": req, "enc": encode_record(tok, req, max_length=a.max_length), "done": threading.Event()}
        except Exception as e:
            with lock:
                stats["errors"] += 1
            return self.reply(400, {"error": str(e)[:300]})
        jobs.put(job); job["done"].wait()
        if "error" in job:
            with lock:
                stats["errors"] += 1
            return self.reply(503, {"error": job["error"]})
        self.reply(200, job["resp"])

    def log_message(self, *x):
        pass


threading.Thread(target=worker, daemon=True).start()
print(f"serving POST http://{a.host}:{a.port}/v1/systemone  (health: /health)", flush=True)
ThreadingHTTPServer((a.host, a.port), H).serve_forever()
