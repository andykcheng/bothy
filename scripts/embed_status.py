#!/usr/bin/env python3
"""Status page for a sharded embedding run (ingest/embed.py --shard-size). Read-only: it looks at
the shard directories, the run's log and the helper file; it never touches the job.

    python3 scripts/embed_status.py [--port 8090] [--log LOG] [--helper-file FILE] DIR [DIR ...]

Open http://<host>:8090/ (refreshes itself); /status.json has the same numbers.
"""

from __future__ import annotations

import argparse
import html
import json
import re
import subprocess
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

WINDOW = 3600  # seconds of finished shards used for the current speed


def source_status(d: Path) -> dict:
    sdir = d / "emb" / "shards"
    total = None
    try:
        total = json.loads((d / "source.json").read_text())["chunks"]
    except (OSError, ValueError, KeyError):
        pass
    try:
        n_shards = len(json.loads((sdir / "index.json").read_text())["offsets"]) - 1
    except (OSError, ValueError, KeyError):
        n_shards = None
    shards = []
    for p in sdir.glob("[0-9][0-9][0-9][0-9][0-9]"):
        try:
            m = json.loads((p / "meta.json").read_text())
        except (OSError, ValueError):
            continue
        m["finished"] = (p / "meta.json").stat().st_mtime
        m["worker"] = "helper" if str(m.get("engine", "")).startswith("http:") else "local"
        shards.append(m)
    merged = (d / "emb" / "manifest.json").exists() and (d / "emb" / "emb_f16.npy").exists()
    return {"name": d.name, "total": total, "n_shards": n_shards, "shards": shards, "merged": merged}


def log_tail(log: Path) -> tuple[dict, list[str]]:
    """Latest progress bar per worker, and the last helper messages."""
    try:
        with open(log, "rb") as f:
            f.seek(max(0, f.seek(0, 2) - 200_000))
            text = f.read().decode("utf-8", "replace")
    except OSError:
        return {}, []
    lines = [l.strip() for l in re.split(r"[\r\n]", text) if l.strip()]
    bars = {}
    for l in lines:
        m = re.search(r"(\S+) (\d+)/(\d+) (local|helper):\s+(\d+)%\|.*?\| (\d+)/(\d+) \[([^<]*)<([^,]*),\s*([^\]]*)\]", l)
        if m:
            bars[m[4]] = {"source": m[1], "shard": int(m[2]), "pct": int(m[5]), "batches": f"{m[6]}/{m[7]}",
                          "elapsed": m[8], "left": m[9], "rate": m[10]}
    helper = [m[0] for l in lines if (m := re.search(r"helper: .*", l)) and "%|" not in m[0]]
    return bars, helper[-5:]


def gpu() -> str:
    try:
        return subprocess.run(["nvidia-smi", "--query-gpu=utilization.gpu,temperature.gpu,power.draw",
                               "--format=csv,noheader"], capture_output=True, text=True, timeout=5).stdout.strip()
    except Exception:
        return "?"


def status(args) -> dict:
    now = time.time()
    srcs = [source_status(Path(d)) for d in args.dirs]
    recent = [m for s in srcs for m in s["shards"] if now - m["finished"] < WINDOW]
    speed = {}
    for w in ("local", "helper"):
        ms = [m for m in recent if m["worker"] == w]
        # per-worker speed while it was working: chunks / its own shard seconds
        speed[w] = round(sum(m["count"] for m in ms) / sum(m["seconds"] for m in ms), 1) if ms else 0.0
    # overall: chunks finished in the window / window (or since the first shard in it)
    if recent:
        span = max(60.0, now - min(m["finished"] - m["seconds"] for m in recent))
        overall = sum(m["count"] for m in recent) / min(span, WINDOW + max(m["seconds"] for m in recent))
    else:
        overall = 0.0
    done = sum(sum(m["count"] for m in s["shards"]) for s in srcs)
    total = sum(s["total"] or 0 for s in srcs)
    left = max(0, total - done)
    bars, helper_log = log_tail(Path(args.log))
    try:
        helper_cfg = json.loads(Path(args.helper_file).read_text())
    except (OSError, ValueError):
        helper_cfg = None
    return {
        "time": time.strftime("%Y-%m-%d %H:%M:%S"),
        "sources": [{k: v for k, v in s.items() if k != "shards"} | {
            "done_chunks": sum(m["count"] for m in s["shards"]), "done_shards": len(s["shards"]),
            "by_helper": sum(1 for m in s["shards"] if m["worker"] == "helper")} for s in srcs],
        "done": done, "total": total, "overall_speed": round(overall, 1), "worker_speed": speed,
        "eta_hours": round(left / overall / 3600, 1) if overall else None,
        "running": bars, "helper_config": helper_cfg, "helper_log": helper_log, "gpu": gpu(),
    }


def fmt_eta(h):
    if h is None:
        return "–"
    return f"{h:.1f} h" if h < 48 else f"{h / 24:.1f} days"


def page(s: dict) -> str:
    e = html.escape
    rows = ""
    for src in s["sources"]:
        tot = src["total"] or 0
        pct = 100 * src["done_chunks"] / tot if tot else 0
        state = "merged ✓" if src["merged"] else ("done, merging" if tot and src["done_chunks"] >= tot else "")
        rows += (f"<tr><td>{e(src['name'])}</td><td>{src['done_shards']}/{src['n_shards'] or '?'}"
                 f" <small>({src['by_helper']} by helper)</small></td>"
                 f"<td>{src['done_chunks']:,} / {tot:,}</td>"
                 f"<td><div class=bar><div style='width:{pct:.2f}%'></div></div> {pct:.1f}% {state}</td></tr>")
    run = "".join(f"<tr><td>{w}</td><td>{e(b['source'])} shard {b['shard']}</td><td>{b['pct']}% "
                  f"({b['batches']} batches)</td><td>{e(b['elapsed'])} elapsed, {e(b['left'])} left, {e(b['rate'])}</td></tr>"
                  for w, b in sorted(s["running"].items()))
    hc = s["helper_config"]
    hstate = "no helper file" if hc is None else ("paused" if hc.get("paused") else e(hc.get("url", "")))
    pct = 100 * s["done"] / s["total"] if s["total"] else 0
    ws = s["worker_speed"]
    return f"""<!DOCTYPE html><html><head><meta charset=utf-8><meta http-equiv=refresh content=15>
<meta name=viewport content="width=device-width, initial-scale=1"><title>Embedding {pct:.1f}%</title>
<style>body{{font:15px system-ui,sans-serif;margin:16px;background:#11161c;color:#e8eef4}}
h1{{font-size:20px}} table{{border-collapse:collapse;margin:8px 0 20px}} td,th{{padding:5px 10px;text-align:left;border-bottom:1px solid #2a3540}}
.bar{{display:inline-block;width:180px;height:10px;background:#2a3540;border-radius:5px;vertical-align:middle}}
.bar div{{height:100%;background:#5fd16b;border-radius:5px}} .big{{font-size:28px;font-weight:600}}
small,.dim{{color:#7d8a96}} .k{{display:inline-block;margin-right:28px}} pre{{white-space:pre-wrap;color:#9fb0bf}}</style></head><body>
<h1>Embedding status <span class=dim>· {e(s['time'])} · refreshes every 15 s</span></h1>
<div><span class=k><div class=dim>overall</div><div class=big>{pct:.2f}%</div></span>
<span class=k><div class=dim>speed (last hour)</div><div class=big>{s['overall_speed']:.0f}/s</div></span>
<span class=k><div class=dim>time left</div><div class=big>{fmt_eta(s['eta_hours'])}</div></span>
<span class=k><div class=dim>local GPU</div><div class=big>{ws['local']:.0f}/s</div></span>
<span class=k><div class=dim>helper</div><div class=big>{ws['helper']:.0f}/s</div></span></div>
<p>{s['done']:,} of {s['total']:,} chunks</p>
<table><tr><th>source</th><th>shards</th><th>chunks</th><th>progress</th></tr>{rows}</table>
<h1>Running now</h1><table>{run or '<tr><td class=dim>nothing</td></tr>'}</table>
<p>GPU (util, temp, power): {e(s['gpu'])}<br>Helper: {hstate}</p>
<pre>{e(chr(10).join(s['helper_log']))}</pre>
<p class=dim>Speeds are chunks per second over shards finished in the last hour. JSON: <a href=status.json style=color:#7d8a96>status.json</a></p>
</body></html>"""


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--port", type=int, default=8090)
    ap.add_argument("--log", default="/mnt/bigbelly/survivalpedia/build/embed-en.log")
    ap.add_argument("--helper-file", default="/mnt/bigbelly/survivalpedia/build/embed-helper.json")
    args = ap.parse_args()

    class H(BaseHTTPRequestHandler):
        def do_GET(self):
            s = status(args)
            if self.path.startswith("/status.json"):
                body, ctype = json.dumps(s, indent=1).encode(), "application/json"
            elif self.path in ("/", "/index.html"):
                body, ctype = page(s).encode(), "text/html; charset=utf-8"
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass

    print(f"embed status on http://0.0.0.0:{args.port}/", flush=True)
    ThreadingHTTPServer(("0.0.0.0", args.port), H).serve_forever()


if __name__ == "__main__":
    main()
