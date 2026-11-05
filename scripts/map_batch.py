#!/usr/bin/env python3
"""Extract map regions listed in maps/regions.json, with their place indexes (DESIGN §16).

  map_batch.py [--out DIR] [--poly DIR] [--jobs N] [--estimate] [ID_OR_PREFIX ...]

Each region becomes OUT/<id>.pmtiles + OUT/<id>.places/. Finished regions are skipped, so the
batch can be stopped and started again. Shapes come from POLY/<id>.geojson (scripts/map_regions.py);
a region with a "bbox" (world) is cut as a rectangle. With no ids, every region is done:
the UK and the world first, then the rest smallest first. Run it detached; it logs to OUT/batch.log.
"""
import argparse, json, os, shutil, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SP = REPO / "core/target/release/sp"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, default=Path.home() / "sp-scratch/maps/regions")
    ap.add_argument("--poly", type=Path, default=Path.home() / "sp-scratch/maps/poly")
    ap.add_argument("--jobs", type=int, default=3)
    ap.add_argument("--threads", type=int, default=16)
    ap.add_argument("ids", nargs="*")
    a = ap.parse_args()
    cfg = json.load(open(REPO / "maps/regions.json"))
    url = cfg["source"].format(build=cfg["build"])
    regions = cfg["regions"]
    if a.ids:
        regions = [r for r in regions if any(r["id"] == i or r["id"].startswith(i) for i in a.ids)]
    first = {"uk": 0, "world": 1}
    regions.sort(key=lambda r: (first.get(r["id"], 2), r.get("size_estimate", 0)))
    a.out.mkdir(parents=True, exist_ok=True)
    cache = a.out / f".dirs-{cfg['build']}"
    log = open(a.out / "batch.log", "a", buffering=1)

    def say(msg):
        log.write(f"{time.strftime('%F %T')} {msg}\n")

    def one(r):
        rid = r["id"]
        if (a.out / f"{rid}.pmtiles").exists() and (a.out / f"{rid}.places").is_dir():
            return 0
        tmp = a.out / f"{rid}.tmp.pmtiles"
        tmp_places = a.out / f"{rid}.tmp.places"
        if tmp_places.exists():
            shutil.rmtree(tmp_places)
        meta = {k: r[k] for k in ("continent", "country", "country_title") if k in r}
        meta["build"] = cfg["build"]
        cmd = [str(SP), "map-extract", "--url", url, "--region", rid, "--title", r["title"],
               "--max-zoom", str(r.get("max_zoom", 15)), "--threads", str(a.threads),
               "--dir-cache", str(cache), "--meta", json.dumps(meta)]
        if "bbox" in r:
            cmd += ["--bbox=" + ",".join(map(str, r["bbox"]))]
        else:
            cmd += ["--polygon", str(a.poly / f"{rid}.geojson")]
        t0 = time.time()
        for attempt in range(3):
            p = subprocess.run(cmd + [str(tmp)], capture_output=True, text=True)
            if p.returncode == 0:
                break
            say(f"FAIL {rid} attempt {attempt + 1}: {p.stderr.strip().splitlines()[-1:]}")
            time.sleep(30)
        else:
            return 1
        p = subprocess.run([str(SP), "map-places", str(tmp)], capture_output=True, text=True)
        if p.returncode != 0:
            say(f"FAIL {rid} places: {p.stderr.strip().splitlines()[-1:]}")
            return 1
        places = a.out / f"{rid}.places"
        if places.exists():
            shutil.rmtree(places)
        m = json.load(open(tmp_places / "meta.json"))
        m["archive"] = f"{rid}.pmtiles"
        (tmp_places / "meta.json").write_text(json.dumps(m))
        os.replace(tmp_places, places)
        os.replace(tmp, a.out / f"{rid}.pmtiles")
        say(f"done {rid} {os.path.getsize(a.out / f'{rid}.pmtiles') / 1e6:.0f} MB in {time.time() - t0:.0f}s")
        return 0

    def safe(r):
        try:
            return one(r)
        except Exception as e:  # one bad region must not stop the batch
            say(f"FAIL {r['id']}: {e!r}")
            return 1

    say(f"start: {len(regions)} regions, build {cfg['build']}, {a.jobs} at a time")
    # the directory cache is made by the first region; do that one alone
    fails = safe(regions[0]) if regions else 0
    with ThreadPoolExecutor(a.jobs) as ex:
        fails += sum(ex.map(safe, regions[1:]))
    say(f"FINISHED, {fails} failed")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
