//! Offline maps (DESIGN §16): serves vector tiles from the installed region archives plus the
//! MapLibre page and its static assets.
//!
//!   GET /map                     map page (MapLibre GL JS, Protomaps style)
//!   GET /tiles/{z}/{x}/{y}       one tile, from the first installed region that has it
//!   GET /maps/assets/{*path}     renderer, style, glyphs, sprites
//!   GET /api/maps                installed regions
//!   GET /api/places?q=&lon=&lat=&k=   place search across installed regions (`<region>.places/`)
//!
//! A maps dir holds `*.pmtiles` (one per region, from `sp map-extract`) and `assets/`
//! (from `scripts/fetch_map_assets.sh`).

use crate::AppState;
use crate::page::esc;
use anyhow::Result;
use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::extract::Query;
use serde::Deserialize;
use sp_maps::places::{Gazetteer, Hit};
use sp_maps::pmt::{Archive, COMPRESSION_GZIP};
use std::path::{Component, Path, PathBuf};

pub struct Region {
    pub id: String,
    pub title: String,
    pub bytes: u64,
    pub archive: Archive,
    /// Place-search index built by `sp map-places`, if present
    pub places: Option<Gazetteer>,
}

pub struct Maps {
    pub dir: PathBuf,
    /// Smallest bbox first, so detailed regions win over a low-zoom world archive.
    pub regions: Vec<Region>,
}

impl Maps {
    pub fn open(dir: &Path) -> Result<Maps> {
        let mut regions = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.extension().is_none_or(|x| x != "pmtiles") {
                continue;
            }
            let archive = Archive::open(&p)?;
            let m = archive.metadata()?;
            let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
            let sp = &m["survivalpedia"];
            let pdir = p.with_extension("places");
            let places = if pdir.join("meta.json").exists() { Some(Gazetteer::open(&pdir)?) } else { None };
            regions.push(Region {
                id: sp["region"].as_str().unwrap_or(&stem).to_string(),
                title: sp["title"].as_str().unwrap_or(&stem).to_string(),
                bytes: std::fs::metadata(&p)?.len(),
                archive,
                places,
            });
        }
        let area = |r: &Region| {
            let b = r.archive.header.bounds;
            (b[2] - b[0]) * (b[3] - b[1])
        };
        regions.sort_by(|a, b| area(a).total_cmp(&area(b)));
        Ok(Maps { dir: dir.to_path_buf(), regions })
    }

    fn max_zoom(&self) -> u8 {
        self.regions.iter().map(|r| r.archive.header.max_zoom).max().unwrap_or(0)
    }
}

pub async fn tile(State(app): State<AppState>, UrlPath((z, x, y)): UrlPath<(u8, u32, String)>) -> Response {
    let Some(maps) = &app.maps else { return StatusCode::NOT_FOUND.into_response() };
    let Ok(y) = y.trim_end_matches(".mvt").parse::<u32>() else { return StatusCode::BAD_REQUEST.into_response() };
    // the tile's lon/lat box, grown by one tile (extracts keep a one-tile margin round their shape)
    let n = (1u64 << z.min(30)) as f64;
    let lon = |x: f64| x / n * 360.0 - 180.0;
    let lat = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
    let (w, e) = (lon(x as f64 - 1.0), lon(x as f64 + 2.0));
    let (north, south) = (lat(y as f64 - 1.0), lat(y as f64 + 2.0));
    for r in &maps.regions {
        let h = &r.archive.header;
        if z < h.min_zoom || z > h.max_zoom {
            continue;
        }
        let b = h.bounds;
        if e < b[0] || w > b[2] || north < b[1] || south > b[3] {
            continue;
        }
        match r.archive.tile(z, x, y) {
            Ok(Some(data)) => {
                let mut resp = (
                    [(header::CONTENT_TYPE, "application/vnd.mapbox-vector-tile"), (header::CACHE_CONTROL, "public, max-age=86400")],
                    data.to_vec(),
                )
                    .into_response();
                if h.tile_compression == COMPRESSION_GZIP {
                    resp.headers_mut().insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
                }
                return resp;
            }
            Ok(None) => continue,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        }
    }
    // outside every installed region: an empty tile, which MapLibre draws as blank
    StatusCode::NO_CONTENT.into_response()
}

pub async fn asset(State(app): State<AppState>, UrlPath(path): UrlPath<String>) -> Response {
    let Some(maps) = &app.maps else { return StatusCode::NOT_FOUND.into_response() };
    let rel = Path::new(&path);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let p = maps.dir.join("assets").join(rel);
    let ct = match p.extension().and_then(|x| x.to_str()) {
        Some("mjs" | "js") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("pbf") => "application/x-protobuf",
        _ => "application/octet-stream",
    };
    match tokio::fs::read(&p).await {
        Ok(data) => ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "public, max-age=86400")], data).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
pub struct PlaceParams {
    #[serde(default)]
    q: String,
    lon: Option<f64>,
    lat: Option<f64>,
    k: Option<usize>,
}

pub async fn places(State(app): State<AppState>, Query(p): Query<PlaceParams>) -> Json<serde_json::Value> {
    let Some(maps) = &app.maps else { return Json(serde_json::json!([])) };
    let k = p.k.unwrap_or(10).clamp(1, 50);
    let near = p.lon.zip(p.lat);
    let mut hits: Vec<(Hit, &str)> = Vec::new();
    for r in &maps.regions {
        if let Some(g) = &r.places {
            hits.extend(g.search(&p.q, near, k).into_iter().map(|h| (h, r.id.as_str())));
        }
    }
    hits.sort_by(|a, b| b.0.score.total_cmp(&a.0.score));
    // the same place can come from overlapping regions (e.g. a country and the world overview)
    let mut out: Vec<(Hit, &str)> = Vec::new();
    for (h, rid) in hits {
        let dup = out.iter().any(|(o, _)| {
            o.name == h.name && o.class == h.class && (o.lon - h.lon).abs() < 0.05 && (o.lat - h.lat).abs() < 0.05
        });
        if !dup {
            out.push((h, rid));
        }
        if out.len() >= k {
            break;
        }
    }
    Json(serde_json::json!(out.into_iter().map(|(h, rid)| {
        let mut v = serde_json::to_value(&h).unwrap_or_default();
        v["region"] = serde_json::json!(rid);
        v
    }).collect::<Vec<_>>()))
}

pub async fn list(State(app): State<AppState>) -> Json<serde_json::Value> {
    let regions: Vec<_> = app
        .maps
        .iter()
        .flat_map(|m| &m.regions)
        .map(|r| {
            let h = &r.archive.header;
            serde_json::json!({
                "id": r.id, "title": r.title, "bytes": r.bytes, "bounds": h.bounds,
                "places": r.places.as_ref().map(|g| g.len()),
                "min_zoom": h.min_zoom, "max_zoom": h.max_zoom, "center": h.center,
            })
        })
        .collect();
    Json(serde_json::json!(regions))
}

pub async fn page(State(app): State<AppState>) -> Response {
    let Some(maps) = &app.maps else {
        return (StatusCode::NOT_FOUND, Html("<p>No maps installed. Start sp-server with --maps DIR.</p>".to_string()))
            .into_response();
    };
    // start on the most detailed region
    let (center, zoom) = maps
        .regions
        .first()
        .map(|r| (r.archive.header.center, r.archive.header.center_zoom.max(5)))
        .unwrap_or(([0.0, 20.0], 2));
    let regions: Vec<String> = maps
        .regions
        .iter()
        .map(|r| {
            let b = r.archive.header.bounds;
            format!(
                "<button data-b=\"{},{},{},{}\">{} <small>{:.0} MB</small></button>",
                b[0], b[1], b[2], b[3], esc(&r.title), r.bytes as f64 / 1e6
            )
        })
        .collect();
    Html(format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Map – Bothy</title>
<link rel="stylesheet" href="/maps/assets/maplibre-gl/maplibre-gl.css">
<style>
html,body{{margin:0;height:100%;font:15px system-ui,sans-serif}}
#map{{position:absolute;inset:0}}
#bar{{position:absolute;z-index:2;top:8px;left:8px;display:flex;gap:6px;flex-wrap:wrap;max-width:calc(100% - 60px)}}
#bar a,#bar button,#q{{background:#fff;border:1px solid #bbb;border-radius:8px;padding:6px 10px;font:inherit;color:#0b5cad;text-decoration:none;cursor:pointer}}
#q{{color:#1b1b1b;cursor:text;width:min(320px,70vw)}}
#search{{position:relative}}
#res{{position:absolute;top:38px;left:0;width:min(380px,90vw);background:#fff;border:1px solid #bbb;border-radius:8px;box-shadow:0 4px 14px #0003;max-height:60vh;overflow:auto;display:none}}
#res div{{padding:7px 10px;cursor:pointer;border-bottom:1px solid #eee}}
#res div:hover,#res div.sel{{background:#eef4fc}}
#res small{{color:#666}}
#pick{{position:absolute;z-index:3;left:8px;right:8px;bottom:24px;margin:auto;max-width:420px;background:#fff;border-radius:14px;box-shadow:0 4px 18px #0004;padding:12px 14px;display:none;color:#1b1b1b}}
#pick p{{margin:0 0 10px}} #pick .row{{display:flex;gap:8px}}
#pick button{{flex:1;font:inherit;font-weight:600;padding:10px;border-radius:999px;border:1px solid #bbb;background:#fff;color:#1b1b1b}}
#pick button.go{{background:#2F6B4F;border-color:#2F6B4F;color:#fff}} #pick button:disabled{{opacity:.45}}
@media (prefers-color-scheme: dark){{#bar a,#bar button,#q{{background:#222;color:#7fb6ff;border-color:#444}}#q{{color:#e8e8e8}}#res{{background:#222;border-color:#444}}#res div{{border-color:#333}}#res div:hover,#res div.sel{{background:#2c3442}}#res small{{color:#9a9a9a}}}}
</style></head><body>
<div id="bar"><a href="/" id="home">← Bothy</a><div id="search"><input id="q" type="search" autocomplete="off" placeholder="Search places…"><div id="res"></div></div>{regions}</div>
<div id="map"></div>
<div id="pick"><p>Tap the map at the place to share. Search above to find it.</p><div class="row"><button id="pickNo">Cancel</button><button id="pickGo" class="go" disabled>Send this place</button></div></div>
<script src="/maps/assets/basemaps/basemaps.js"></script>
<script type="module">
import * as maplibregl from '/maps/assets/maplibre-gl/maplibre-gl.mjs';
const o = location.origin;
const dark = matchMedia('(prefers-color-scheme: dark)').matches;
const flavor = dark ? 'dark' : 'light';
const lang = new URLSearchParams(location.search).get('lang') || (navigator.language || 'en').slice(0, 2);
const style = {{
  version: 8,
  glyphs: o + '/maps/assets/fonts/{{fontstack}}/{{range}}.pbf',
  sprite: o + '/maps/assets/sprites/v4/' + flavor,
  sources: {{ protomaps: {{ type: 'vector', tiles: [o + '/tiles/{{z}}/{{x}}/{{y}}'], maxzoom: {maxz},
    attribution: '<a href="https://www.openstreetmap.org/copyright">© OpenStreetMap contributors</a>' }} }},
  layers: shields(basemaps.layers('protomaps', basemaps.namedFlavor(flavor), {{ lang }})),
}};
// Road shields: the tiles' shield_text is the E-road number where a road is also a European route
// (the M1 shows E13, which isn't signed in the UK), and roads in no route get no shield. Use the
// road's own number (ref) for those instead.
function shields(layers) {{
  const sh = layers.find((l) => l.id === 'roads_shields');
  if (!sh) return layers;
  const label = ['case',
    ['all', ['has', 'ref'], ['any', ['!', ['has', 'shield_text']], ['==', ['get', 'network'], 'e-road']]], ['get', 'ref'],
    ['coalesce', ['get', 'shield_text'], '']];
  const len = ['length', label];
  sh.filter = ['all', ['in', ['get', 'kind'], ['literal', ['highway', 'major_road']]],
    ['any', ['has', 'shield_text'], ['has', 'ref']], ['>', len, 0], ['<=', len, 5]];
  sh.layout['text-field'] = label;
  sh.layout['icon-image'] = ['match', ['get', 'network'],
    'US:I', ['concat', 'US:I-', len, 'char'], 'NL:S-road', ['concat', 'NL:S-road-', len, 'char'],
    ['concat', 'generic_shield-', len, 'char']];
  return layers;
}}
const map = new maplibregl.Map({{
  container: 'map', style, center: [{lon}, {lat}], zoom: {zoom}, hash: true,
  localIdeographFontFamily: '"Noto Sans CJK SC", "Noto Sans CJK JP", "Hiragino Sans", "PingFang SC", sans-serif',
}});
map.addControl(new maplibregl.NavigationControl());
map.addControl(new maplibregl.ScaleControl());
map.addControl(new maplibregl.GeolocateControl({{ trackUserLocation: true }}));
// place search
const q = document.getElementById('q'), res = document.getElementById('res');
let marker = null, timer = null, hits = [], sel = -1;
const zoomFor = h => h.class === 'street' ? 16 : h.class === 'poi' ? 16 :
  ({{country: 5, region: 7, city: 11, town: 13, village: 14}})[h.kind] || 15;
let onGo = null; // pick mode: a search result is also a choice
function go(h) {{
  res.style.display = 'none';
  if (onGo) {{ map.flyTo({{ center: [h.lon, h.lat], zoom: zoomFor(h) }}); q.value = h.name; return onGo(h.lon, h.lat); }}
  q.value = h.name;
  map.flyTo({{ center: [h.lon, h.lat], zoom: zoomFor(h) }});
  if (marker) marker.remove();
  marker = new maplibregl.Marker().setLngLat([h.lon, h.lat])
    .setPopup(new maplibregl.Popup().setText(h.name + (h.context ? ', ' + h.context : ''))).addTo(map);
}}
function show() {{
  res.innerHTML = '';
  hits.forEach((h, i) => {{
    const d = document.createElement('div');
    if (i === sel) d.className = 'sel';
    const alt = h.matched ? ' (' + h.matched + ')' : '';
    d.innerHTML = '<b></b><span></span><br><small></small>';
    d.querySelector('b').textContent = h.name;
    d.querySelector('span').textContent = alt;
    // no nearby settlement (e.g. world overview places): show coordinates to tell same-named places apart
    const where = h.context || (Math.abs(h.lat).toFixed(2) + '°' + (h.lat >= 0 ? 'N' : 'S') + ' ' + Math.abs(h.lon).toFixed(2) + '°' + (h.lon >= 0 ? 'E' : 'W'));
    d.querySelector('small').textContent = [h.kind.replace(/_/g, ' '), where].filter(Boolean).join(' · ');
    d.onclick = () => go(h);
    res.appendChild(d);
  }});
  res.style.display = hits.length ? 'block' : 'none';
}}
async function search() {{
  const c = map.getCenter();
  const r = await fetch('/api/places?' + new URLSearchParams({{ q: q.value, lon: c.lng, lat: c.lat, k: 10 }}));
  hits = await r.json(); sel = -1; show();
}}
q.oninput = () => {{ clearTimeout(timer); timer = setTimeout(search, 150); }};
q.onkeydown = e => {{
  if (e.key === 'ArrowDown') {{ sel = Math.min(sel + 1, hits.length - 1); show(); e.preventDefault(); }}
  else if (e.key === 'ArrowUp') {{ sel = Math.max(sel - 1, 0); show(); e.preventDefault(); }}
  else if (e.key === 'Enter' && hits.length) go(hits[Math.max(sel, 0)]);
  else if (e.key === 'Escape') res.style.display = 'none';
}};
map.on('click', () => res.style.display = 'none');
const q0 = new URLSearchParams(location.search).get('q');
if (q0) {{ q.value = q0; search().then(() => hits.length && go(hits[0])); }}
// ?pin=lat,lon&label=…: a place someone shared in chat
const P0 = new URLSearchParams(location.search);
// opened from chat (a place, or picking one): the top link goes back to the chat
if ((P0.get('pin') || P0.get('pick')) && history.length > 1) {{
  const h = document.getElementById('home');
  h.textContent = '← Back';
  h.onclick = (e) => {{ e.preventDefault(); history.back(); }};
}}
const pin = (P0.get('pin') || '').split(',').map(Number);
if (pin.length === 2 && pin.every(Number.isFinite)) {{
  map.jumpTo({{ center: [pin[1], pin[0]], zoom: 16 }});
  marker = new maplibregl.Marker({{ color: '#C2453A' }}).setLngLat([pin[1], pin[0]])
    .setPopup(new maplibregl.Popup().setText(P0.get('label') || 'Shared place')).addTo(map);
  map.once('load', () => marker.togglePopup());
}}
// ?pick=1: choose a place for chat; the chat page picks it up from sessionStorage
if (P0.get('pick')) {{
  const box = document.getElementById('pick'), goBtn = document.getElementById('pickGo');
  box.style.display = 'block';
  let at = null;
  const choose = (lng, lat) => {{
    at = {{ lat: +lat.toFixed(6), lon: +lng.toFixed(6) }};
    if (marker) marker.remove();
    marker = new maplibregl.Marker({{ color: '#C2453A' }}).setLngLat([lng, lat]).addTo(map);
    goBtn.disabled = false;
  }};
  map.on('click', e => choose(e.lngLat.lng, e.lngLat.lat));
  onGo = choose;
  const leave = () => history.length > 1 ? history.back() : (location.href = '/chat');
  document.getElementById('pickNo').onclick = leave;
  goBtn.onclick = () => {{ sessionStorage.setItem('sp-pick', JSON.stringify(at)); leave(); }};
}}
for (const b of document.querySelectorAll('#bar button')) {{
  b.onclick = () => {{ const [w, s, e, n] = b.dataset.b.split(',').map(Number); map.fitBounds([[w, s], [e, n]]); }};
}}
</script></body></html>"#,
        regions = regions.join(""),
        maxz = maps.max_zoom(),
        lon = center[0],
        lat = center[1],
        zoom = zoom,
    ))
    .into_response()
}
