//! Region shapes for map extracts: a GeoJSON (Multi)Polygon, and the web-mercator tiles it touches.
//!
//! Tiles are picked per zoom by rows: in each row, a tile is wanted if a polygon edge passes
//! through it or the row's centre line is inside the polygon (even-odd), then the set grows by
//! one tile all round so coarse borders (Natural Earth) never clip the coast or a border town.

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Rings of one polygon (outer first, then holes), each a list of (lon, lat).
pub type Polygon = Vec<Vec<(f64, f64)>>;

/// Polygons from a GeoJSON Feature, FeatureCollection, Polygon, MultiPolygon or GeometryCollection.
pub fn from_geojson(v: &Value) -> Result<Vec<Polygon>> {
    let mut out = Vec::new();
    collect(v, &mut out)?;
    if out.is_empty() {
        bail!("no polygons in the GeoJSON");
    }
    Ok(out)
}

fn collect(v: &Value, out: &mut Vec<Polygon>) -> Result<()> {
    let ring = |r: &Value| -> Result<Vec<(f64, f64)>> {
        r.as_array()
            .context("ring is not an array")?
            .iter()
            .map(|p| Ok((p[0].as_f64().context("bad coordinate")?, p[1].as_f64().context("bad coordinate")?)))
            .collect()
    };
    let poly = |p: &Value| -> Result<Polygon> { p.as_array().context("polygon is not an array")?.iter().map(ring).collect() };
    match v["type"].as_str().unwrap_or("") {
        "FeatureCollection" => {
            for f in v["features"].as_array().into_iter().flatten() {
                collect(f, out)?;
            }
        }
        "Feature" => collect(&v["geometry"], out)?,
        "GeometryCollection" => {
            for g in v["geometries"].as_array().into_iter().flatten() {
                collect(g, out)?;
            }
        }
        "Polygon" => out.push(poly(&v["coordinates"])?),
        "MultiPolygon" => {
            for p in v["coordinates"].as_array().context("bad MultiPolygon")? {
                out.push(poly(p)?);
            }
        }
        _ => {}
    }
    Ok(())
}

/// (min lon, min lat, max lon, max lat) of the polygons.
pub fn bbox(polys: &[Polygon]) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for &(x, y) in polys.iter().flatten().flatten() {
        b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }
    b
}

/// Web-mercator position in tile units at zoom z (x right, y down).
fn project(lon: f64, lat: f64, n: f64) -> (f64, f64) {
    let lat = lat.clamp(-85.0511, 85.0511).to_radians();
    ((lon + 180.0) / 360.0 * n, (1.0 - lat.tan().asinh() / std::f64::consts::PI) / 2.0 * n)
}

/// Wanted tiles at zoom z: for each row y, sorted disjoint inclusive x ranges.
pub fn tile_rows(polys: &[Polygon], z: u8) -> Vec<(u32, Vec<(u32, u32)>)> {
    let n = (1u64 << z) as f64;
    let max = (1u32 << z) - 1;
    let clamp = |v: f64| (v.floor().max(0.0) as u32).min(max);
    // projected edges, per polygon (fill is even-odd within a polygon, union across polygons)
    let edges: Vec<Vec<((f64, f64), (f64, f64))>> = polys
        .iter()
        .map(|p| {
            let mut e = Vec::new();
            for r in p {
                let pts: Vec<_> = r.iter().map(|&(x, y)| project(x, y, n)).collect();
                for i in 0..pts.len() {
                    let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                    if a != b {
                        e.push((a, b));
                    }
                }
            }
            e
        })
        .collect();
    let mut ymin = f64::MAX;
    let mut ymax = f64::MIN;
    for &((_, y0), (_, y1)) in edges.iter().flatten() {
        ymin = ymin.min(y0.min(y1));
        ymax = ymax.max(y0.max(y1));
    }
    if ymin > ymax {
        return Vec::new();
    }
    let mut rows: Vec<(u32, Vec<(u32, u32)>)> = Vec::new();
    for y in clamp(ymin)..=clamp(ymax) {
        let (top, bot, mid) = (y as f64, y as f64 + 1.0, y as f64 + 0.5);
        let mut spans: Vec<(u32, u32)> = Vec::new();
        for pe in &edges {
            let mut xs: Vec<f64> = Vec::new();
            for &((x0, y0), (x1, y1)) in pe {
                // the edge's part inside this row band
                let (lo, hi) = (y0.min(y1), y0.max(y1));
                if hi >= top && lo <= bot {
                    let (a, b) = if y1 == y0 {
                        (x0, x1)
                    } else {
                        let at = |yy: f64| x0 + (x1 - x0) * (yy - y0) / (y1 - y0);
                        (at(lo.max(top)), at(hi.min(bot)))
                    };
                    spans.push((clamp(a.min(b)), clamp(a.max(b))));
                }
                // crossings of the centre line (half-open rule so vertices count once)
                if (y0 <= mid) != (y1 <= mid) {
                    xs.push(x0 + (x1 - x0) * (mid - y0) / (y1 - y0));
                }
            }
            xs.sort_by(f64::total_cmp);
            for p in xs.chunks_exact(2) {
                spans.push((clamp(p[0]), clamp(p[1])));
            }
        }
        rows.push((y, merge(spans)));
    }
    // grow by one tile all round
    let mut grown: Vec<(u32, Vec<(u32, u32)>)> = Vec::new();
    let first = rows.first().map_or(0, |r| r.0).saturating_sub(1);
    let last = rows.last().map_or(0, |r| r.0).saturating_add(1).min(max);
    for y in first..=last {
        let mut spans = Vec::new();
        for (ry, s) in &rows {
            if ry.abs_diff(y) <= 1 {
                spans.extend(s.iter().map(|&(a, b)| (a.saturating_sub(1), (b + 1).min(max))));
            }
        }
        let spans = merge(spans);
        if !spans.is_empty() {
            grown.push((y, spans));
        }
    }
    grown
}

fn merge(mut s: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    s.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (a, b) in s {
        match out.last_mut() {
            Some(l) if a <= l.1 + 1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Number of tiles `tile_rows` gives.
pub fn count(rows: &[(u32, Vec<(u32, u32)>)]) -> u64 {
    rows.iter().flat_map(|r| &r.1).map(|&(a, b)| (b - a + 1) as u64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        vec![vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]]
    }

    #[test]
    fn square_matches_bbox_plus_margin() {
        let p = vec![square(-5.0, 50.0, 1.0, 55.0)];
        for z in [6u8, 10, 12] {
            let (x0, y0, x1, y1) = crate::pmt::tile_range([-5.0, 50.0, 1.0, 55.0], z);
            let want = (x1 - x0 + 3) as u64 * (y1 - y0 + 3) as u64;
            assert_eq!(count(&tile_rows(&p, z)), want, "z{z}");
        }
    }

    #[test]
    fn hole_and_triangle_are_smaller_than_bbox() {
        let mut p = square(0.0, 0.0, 10.0, 10.0);
        p.extend(square(2.0, 2.0, 8.0, 8.0));
        let rows = tile_rows(&[p], 13);
        let (x0, y0, x1, y1) = crate::pmt::tile_range([0.0, 0.0, 10.0, 10.0], 13);
        let full = (x1 - x0 + 3) as u64 * (y1 - y0 + 3) as u64;
        assert!(count(&rows) < full * 3 / 4, "{} of {full}", count(&rows));
        let tri = vec![vec![vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (0.0, 0.0)]]];
        let c = count(&tile_rows(&tri, 13));
        assert!(c < full * 6 / 10 && c > full * 4 / 10, "{c} of {full}");
    }

    #[test]
    fn tiny_polygon_gets_its_tile() {
        let p = vec![square(0.1001, 0.1001, 0.1002, 0.1002)];
        assert_eq!(count(&tile_rows(&p, 14)), 9);
    }
}
