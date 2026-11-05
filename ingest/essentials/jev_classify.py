"""Score candidate articles for the essentials tier with TypeSafe's Jev (a hosted classifier).

  uv run python essentials/jev_classify.py <leads.jsonl> <out.jsonl> [--limit N | --sample N] [--workers 16] [--rps 70] [--question v5] [--port 8091]

- Interruptible and resumable: one result line is appended per article as it finishes. Ctrl-C / SIGTERM stops
  cleanly (in-flight requests finish); rerunning skips page ids already in <out>.
- Control files next to <out>: <out>.PAUSE pauses (delete it to resume), <out>.STOP stops after in-flight requests.
- Progress: <out>.status.json is rewritten every few seconds, and a small page is served on --port
  (http://<host>:8091/) with done/total, rate, ETA, tokens used, the score histogram and recent examples.
- The API key is read from JEV_API_KEY in the environment or in the repo's .env (never printed).
Output rows: {page_id, title, rank, score (0..3), confidence, probabilities, tokens}.
"""
import argparse, html as htmlmod, json, os, random, signal, threading, time, urllib.error, urllib.request
from collections import deque
from concurrent.futures import ThreadPoolExecutor, FIRST_COMPLETED, wait
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from questions import QUESTIONS

ap = argparse.ArgumentParser()
ap.add_argument("leads"); ap.add_argument("out")
ap.add_argument("--limit", type=int, help="only the first N leads (in rank order)")
ap.add_argument("--sample", type=int, help="a random sample of N leads (test sets)")
ap.add_argument("--ids", help="file of page ids to classify (one per line)")
ap.add_argument("--seed", type=int, default=7)
ap.add_argument("--workers", type=int, default=16)
ap.add_argument("--port", type=int, default=8091)
ap.add_argument("--model", default="jev-latest")
ap.add_argument("--url", default="https://api.typesafe.ai/v1/systemone",
                help="SystemOne endpoint; e.g. http://localhost:19100/v1/systemone for the local Clef server")
ap.add_argument("--question", default="v5", choices=sorted(QUESTIONS))
ap.add_argument("--rps", type=float, default=70, help="max requests per second (all workers together)")
ap.add_argument("--pack", type=int, default=1, help="articles per request (5 saves ~26%% of tokens; scores agree 0.965 with single)")
ap.add_argument("--max-tokens", type=float, help="stop starting new requests once this many input tokens are used (budget)")
a = ap.parse_args()

ENV = Path(__file__).resolve().parents[2] / ".env"
key = os.environ.get("JEV_API_KEY") or next((l.split("=", 1)[1].strip().strip("'\"") for l in ENV.read_text().splitlines()
                                              if l.startswith("JEV_API_KEY=")), None) if ENV.exists() else os.environ.get("JEV_API_KEY")
LOCAL = "api.typesafe.ai" not in a.url
assert key or LOCAL, "JEV_API_KEY not set (environment or .env)"

out = Path(a.out)
PAUSE, STOP, STATUS = Path(f"{out}.PAUSE"), Path(f"{out}.STOP"), Path(f"{out}.status.json")
done_ids = set()
if out.exists():
    for line in open(out):
        try:
            done_ids.add(json.loads(line)["page_id"])
        except ValueError:
            pass   # a line cut short by a hard kill
leads = [json.loads(l) for l in open(a.leads)]
leads.sort(key=lambda r: r["rank"])
if a.ids:
    ids = {int(x) for x in open(a.ids) if x.strip()}
    leads = [r for r in leads if r["page_id"] in ids]
if a.sample:
    random.Random(a.seed).shuffle(leads); leads = leads[: a.sample]
elif a.limit:
    leads = leads[: a.limit]
total = len(leads)
todo = [r for r in leads if r["page_id"] not in done_ids]

st = {"total": total, "done_before": total - len(todo), "done": total - len(todo), "errors": 0, "throttled": 0, "retries": 0, "in_tokens": 0, "out_tokens": 0,
      "hist": [0, 0, 0, 0], "started": time.time(), "state": "running", "rate": 0.0, "recent": [], "last_error": None}
recent = deque(maxlen=25)
bands = [deque(maxlen=12) for _ in range(4)]   # latest examples per score band, for the progress page
times = deque(maxlen=400)
lock = threading.Lock()
stopping = threading.Event()


def on_signal(sig, frame):
    stopping.set(); st["state"] = "stopping"
signal.signal(signal.SIGINT, on_signal); signal.signal(signal.SIGTERM, on_signal)


_tb = {"next": 0.0}
_tb_lock = threading.Lock()


def throttle():
    with _tb_lock:
        now = time.time(); t = max(now, _tb["next"]); _tb["next"] = t + 1 / a.rps
    if t > now:
        time.sleep(t - now)


def request_body(grp):
    if len(grp) == 1:
        r = grp[0]
        return {"model": a.model, "state": f"Title: {r['title']}\n\n{r['lead']}", "questions": QUESTIONS[a.question]}
    q = QUESTIONS[a.question]["useful"]
    state = "\n\n".join(f"=== ARTICLE {n + 1} ===\nTitle: {r['title']}\n\n{r['lead']}" for n, r in enumerate(grp))
    qs = {f"useful_{n + 1}": {"type": "score", "criteria": q["criteria"],
                              "instructions": f"Article {n + 1} (\"{r['title']}\"): " + q["instructions"]} for n, r in enumerate(grp)}
    return {"model": a.model, "state": state, "questions": qs}


def call(grp):
    body = request_body(grp)
    hdr = {"content-type": "application/json"}
    if not LOCAL:
        hdr["authorization"] = f"Bearer {key}"   # never send the Jev key to other endpoints
    req = urllib.request.Request(a.url, json.dumps(body).encode(), hdr)
    delay = 1.0
    for attempt in range(8):
        throttle()
        try:
            d = json.load(urllib.request.urlopen(req, timeout=300))
            n = len(grp)
            tokens = {k: v / n for k, v in d["usage"].items() if isinstance(v, (int, float))}
            out = []
            for i, r in enumerate(grp):
                ans = d["answers"]["useful" if n == 1 else f"useful_{i + 1}"]
                out.append({"page_id": r["page_id"], "title": r["title"], "rank": r["rank"], "score": ans["score"],
                            "confidence": ans["confidence"], "probabilities": ans["probabilities"], "model": d["model"],
                            "q": a.question, "pack": n, "tokens": tokens})
            return out
        except urllib.error.HTTPError as e:
            if e.code in (429, 500, 502, 503, 504, 529):
                with lock:
                    st["throttled" if e.code in (429, 529) else "retries"] += 1
                ra = e.headers.get("retry-after")
                if ra and ra.isdigit():
                    delay = max(delay, float(ra))
                time.sleep(delay); delay = min(delay * 2, 60); continue
            raise RuntimeError(f"HTTP {e.code}: {e.read()[:200]!r}")
        except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
            with lock:
                st["retries"] += 1
            time.sleep(delay); delay = min(delay * 2, 60)
    raise RuntimeError("gave up after retries")


def write_status():
    with lock:
        el = time.time() - st["started"]
        st["elapsed_s"] = round(el)
        if len(times) > 1:
            st["rate"] = round((len(times) - 1) / max(1e-6, times[-1] - times[0]), 2)
        left = st["total"] - st["done"]
        st["eta_s"] = round(left / st["rate"]) if st["rate"] else None
        st["recent"] = list(recent)
        if PAUSE.exists() and st["state"] == "running":
            st["state"] = "paused"
        elif not PAUSE.exists() and st["state"] == "paused":
            st["state"] = "running"
        STATUS.write_text(json.dumps(st, indent=1))


PAGE = """<!doctype html><meta charset=utf-8><meta http-equiv=refresh content=5><meta name=viewport content="width=device-width,initial-scale=1">
<title>Jev {pct}%</title><style>body{{font:15px system-ui,sans-serif;margin:16px;background:#11161c;color:#e8eef4}}
.bar{{background:#2a3440;border-radius:6px;height:18px;overflow:hidden}}.bar div{{background:#4caf7a;height:100%}}
td{{padding:2px 10px 2px 0}}.m{{color:#9fb0c0}}.bands{{display:flex;flex-wrap:wrap;gap:18px}}.bands div{{min-width:260px}}
h3{{margin:18px 0 6px;font-size:16px}}</style>
<h2>Essentials classification: {state}</h2><div class=bar><div style="width:{pct}%"></div></div>
<p>{done:,} / {total:,} ({pct}%) &middot; {rate} articles/s &middot; ETA {eta} &middot; errors {errors} &middot; rate-limited {throttled} &middot; retries {retries} &middot; cap {rps}/s</p>
<p class=m>tokens in {in_tokens:,} / out {out_tokens:,} &middot; file {out}</p>
<p>Score histogram (0 not useful … 3 essential): {hist}</p>
<p class=m>Pause: <code>touch {out}.PAUSE</code> (delete to resume) &middot; Stop: <code>touch {out}.STOP</code> or Ctrl-C &middot; rerun to resume</p>
<h3>Latest examples by score</h3><div class=bands>{bands}</div>
<h3>Most recent</h3><table><tr><th align=left>score<th align=left>rank<th align=left>title</tr>{rows}</table>"""


class H(BaseHTTPRequestHandler):
    def do_GET(self):
        with lock:
            s = dict(st)
        pct = round(100 * s["done"] / max(1, s["total"]), 1)
        eta = time.strftime("%H:%M:%S", time.gmtime(s["eta_s"])) if s.get("eta_s") else "-"
        rows = "".join(f"<tr><td>{x['score']:.2f}<td>{x['rank']}<td>{htmlmod.escape(x['title'])}</tr>" for x in reversed(s["recent"]))
        names = ["3 essential (2.5+)", "2 useful (1.5-2.5)", "1 background (0.5-1.5)", "0 not useful (<0.5)"]
        with lock:
            bl = [list(b) for b in bands]
        band_html = "".join(f"<div><b>{names[3 - k]}</b> &middot; {s['hist'][k]:,}<table>" + "".join(
            f"<tr><td>{x['score']:.2f}<td class=m>{x['rank']}<td>{htmlmod.escape(x['title'])}</tr>" for x in reversed(bl[k]))
            + "</table></div>" for k in (3, 2, 1, 0))
        html = PAGE.format(pct=pct, eta=eta, rows=rows, bands=band_html, out=out, rps=a.rps, hist=" / ".join(map(str, s["hist"])),
                           **{k: s[k] for k in ("state", "done", "total", "rate", "errors", "throttled", "retries", "in_tokens", "out_tokens")})
        b = html.encode()
        self.send_response(200); self.send_header("content-type", "text/html; charset=utf-8"); self.send_header("content-length", str(len(b)))
        self.end_headers(); self.wfile.write(b)

    def log_message(self, *x):
        pass


try:
    srv = ThreadingHTTPServer(("0.0.0.0", a.port), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    print(f"progress: http://0.0.0.0:{a.port}/", flush=True)
except OSError as e:
    print("no progress page:", e, flush=True)

print(f"{total:,} articles, {st['done_before']:,} already done, {len(todo):,} to go", flush=True)
last_status = 0
with open(out, "a") as f, ThreadPoolExecutor(a.workers) as ex:
    it = iter([todo[i: i + a.pack] for i in range(0, len(todo), a.pack)]); running = set(); exhausted = False
    while True:
        if STOP.exists():
            stopping.set(); st["state"] = "stopping"
        if a.max_tokens and st["in_tokens"] + len(running) * 800 * a.pack >= a.max_tokens and not stopping.is_set():
            stopping.set(); st["state"] = "budget reached"; print("token budget reached", flush=True)
        while not stopping.is_set() and not PAUSE.exists() and len(running) < a.workers * 2:
            r = next(it, None)
            if r is None:
                exhausted = True
                break
            running.add(ex.submit(call, r))
        if running:
            fin, running = wait(running, timeout=1, return_when=FIRST_COMPLETED)
        else:
            time.sleep(1); fin = ()
        for fu in fin:
            try:
                res = fu.result()
            except Exception as e:
                with lock:
                    st["errors"] += 1; st["last_error"] = str(e)[:300]
                continue
            for res in res:
                f.write(json.dumps(res, ensure_ascii=False) + "\n")
                with lock:
                    st["done"] += 1; times.append(time.time())
                    st["in_tokens"] += res["tokens"].get("input_tokens", 0); st["out_tokens"] += res["tokens"].get("output_tokens", 0)
                    st["hist"][min(3, int(res["score"] + 0.5))] += 1
                    recent.append({"score": res["score"], "rank": res["rank"], "title": res["title"]})
                    bands[min(3, int(res["score"] + 0.5))].append({"score": res["score"], "rank": res["rank"], "title": res["title"]})
            f.flush()
        if time.time() - last_status > 3:
            write_status(); last_status = time.time()
            if st["errors"] and st["errors"] % 50 == 0:
                print("errors:", st["errors"], st["last_error"], flush=True)
        if not running and (stopping.is_set() or exhausted):
            break
if st["state"] != "budget reached":
    st["state"] = "stopped" if stopping.is_set() else "finished"
write_status()
print(json.dumps({k: st[k] for k in ("state", "done", "total", "errors", "throttled", "retries", "in_tokens", "out_tokens", "hist")}), flush=True)
