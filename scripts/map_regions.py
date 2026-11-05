#!/usr/bin/env python3
"""Map regions from Natural Earth borders (public domain), DESIGN §16.

  map_regions.py countries NE_DIR OUT_DIR   one GeoJSON per country (+ index.json)
  map_regions.py list POLY_DIR REGIONS_JSON [EST_DIR]  write maps/regions.json from the above
                                                   (countries with a <id>-states.json are replaced by their parts)
  map_regions.py states NE_DIR OUT_DIR ISO2 [FIELD]  one GeoJSON per state/province of a country,
                                                   or per value of an admin-1 FIELD (France: region)

Ids: ISO 3166-1 alpha-2 in lower case ("fr"), "uk" for the United Kingdom (the first extract's id),
ADM0_A3 where there is no ISO code; states are "<country>-<state code>" ("us-ca").
"""
import json, re, sys
from pathlib import Path


def slug(s):
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")


def country_id(p):
    a3 = p["ADM0_A3"]
    if a3 == "GBR":
        return "uk"
    if a3 == "TWN":
        return "tw"  # NE has CN-TW
    a2 = p["ISO_A2"]
    if a2 == "-99" and p["TYPE"] in ("Country", "Sovereign country", "Sovereignty"):
        a2 = p["ISO_A2_EH"]  # France, Norway: the code is only in the _EH column
    return a2.lower() if a2 != "-99" else a3.lower()


def countries(ne, out):
    d = json.load(open(ne / "ne_10m_admin_0_countries.geojson"))
    idx = []
    for f in d["features"]:
        p = f["properties"]
        rid = country_id(p)
        g = {"type": "Feature", "properties": {}, "geometry": f["geometry"]}
        (out / f"{rid}.geojson").write_text(json.dumps(g))
        idx.append({"id": rid, "title": p["NAME_EN"] or p["ADMIN"], "continent": p["CONTINENT"], "a3": p["ADM0_A3"]})
    ids = [r["id"] for r in idx]
    dup = {i for i in ids if ids.count(i) > 1}
    assert not dup, dup
    (out / "index.json").write_text(json.dumps(sorted(idx, key=lambda r: r["id"]), indent=1))
    print(len(idx), "countries")


def states(ne, out, iso2, group=None):
    """One region per state, or per value of the admin-1 field `group` (France: "region")."""
    d = json.load(open(ne / "ne_10m_admin_1_states_provinces.geojson"))
    cid = "uk" if iso2.upper() == "GB" else iso2.lower()
    parts = {}
    nameless = []
    feats = [f for f in d["features"] if f["properties"]["iso_a2"] == iso2.upper()]
    for f in feats:
        p = f["properties"]
        if not (p["name"] or p["name_en"]):
            nameless.append(f)  # a sliver with no name (one in Russia): joins the nearest region below
            continue
        if group:
            title = p[group] or p["name"]
            key = slug((p.get("region_cod") or title).split("-")[-1])
        else:
            title = p["name_en"] or p["name"]
            key = slug((p.get("iso_3166_2") or "").split("-")[-1] or p["name"])
        r = parts.setdefault(f"{cid}-{key}", {"title": title, "features": []})
        r["features"].append({"type": "Feature", "properties": {}, "geometry": f["geometry"]})
        r.setdefault("at", (p["latitude"], p["longitude"]))
    for f in nameless:
        p = f["properties"]
        near = min(parts.values(), key=lambda r: (r["at"][0] - p["latitude"]) ** 2 + (r["at"][1] - p["longitude"]) ** 2)
        near["features"].append({"type": "Feature", "properties": {}, "geometry": f["geometry"]})
        print("nameless part joined", near["title"])
    idx = []
    for rid, r in parts.items():
        (out / f"{rid}.geojson").write_text(json.dumps({"type": "FeatureCollection", "features": r["features"]}))
        idx.append({"id": rid, "title": r["title"], "country": cid})
    (out / f"{cid}-states.json").write_text(json.dumps(sorted(idx, key=lambda r: r["id"]), indent=1))
    print(len(idx), "parts of", cid)


CONTINENTS = {"Seven seas (open ocean)": "Oceans"}
SPLIT = {"us": None, "ca": None, "ru": None, "de": None, "cn": None, "br": None, "fr": "region"}  # see DESIGN §16


def build_list(poly, dest, est=None):
    countries = json.load(open(poly / "index.json"))
    regions = [{"id": "world", "title": "World overview", "continent": "World", "bbox": [-180, -85.05, 180, 85.05], "max_zoom": 6}]
    for c in countries:
        cont = CONTINENTS.get(c["continent"], c["continent"])
        parts = poly / f"{c['id']}-states.json"
        if parts.exists():
            for s in json.load(open(parts)):
                regions.append({"id": s["id"], "title": s["title"], "continent": cont, "country": c["id"], "country_title": c["title"]})
        else:
            regions.append({"id": c["id"], "title": c["title"], "continent": cont})
    for r in regions:
        if est and (est / f"{r['id']}.json").exists():
            r["size_estimate"] = json.load(open(est / f"{r['id']}.json"))["out_bytes"]
    old = json.load(open(dest)) if dest.exists() else {}
    out = {
        "_comment": "Downloadable map regions (DESIGN §16), built by scripts/map_batch.py. Shapes: Natural Earth 10m "
        "admin-0 countries, the biggest split into admin-1 parts (scripts/map_regions.py); world is a bbox. "
        "max_zoom defaults to 15.",
        "source": old.get("source", "https://build.protomaps.com/{build}.pmtiles"),
        "build": old.get("build"),
        "regions": regions,
    }
    dest.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(len(regions), "regions")


if __name__ == "__main__":
    if sys.argv[1] == "list":
        build_list(Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4]) if len(sys.argv) > 4 else None)
        sys.exit()
    cmd, ne, out = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
    out.mkdir(parents=True, exist_ok=True)
    {"countries": lambda: countries(ne, out), "states": lambda: states(ne, out, sys.argv[4], (sys.argv[5:] or [None])[0])}[cmd]()
