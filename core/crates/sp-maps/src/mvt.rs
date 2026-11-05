//! Minimal Mapbox Vector Tile (MVT 2.1) decoding: layers, feature properties and a
//! representative point per feature. Enough to build a gazetteer from basemap tiles.
//! Spec: https://github.com/mapbox/vector-tile-spec/tree/master/2.1

use anyhow::{Result, bail};

#[derive(Debug, Clone)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        if let Value::Str(s) = self { Some(s) } else { None }
    }
    pub fn as_f64(&self) -> Option<f64> {
        if let Value::Num(n) = self { Some(*n) } else { None }
    }
}

#[derive(Debug)]
pub struct Feature {
    pub geom_type: u32, // 1 point, 2 line, 3 polygon
    pub props: Vec<(String, Value)>,
    /// Representative point in tile pixels (0..extent): the first point, the middle vertex of a
    /// line, or the vertex average of a polygon's outer ring.
    pub point: (f64, f64),
}

impl Feature {
    pub fn get(&self, k: &str) -> Option<&Value> {
        self.props.iter().find(|(key, _)| key == k).map(|(_, v)| v)
    }
}

#[derive(Debug)]
pub struct Layer {
    pub name: String,
    pub extent: u32,
    pub features: Vec<Feature>,
}

struct Pb<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Pb<'a> {
    fn new(b: &'a [u8]) -> Self {
        Pb { b, pos: 0 }
    }
    fn done(&self) -> bool {
        self.pos >= self.b.len()
    }
    fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let Some(&byte) = self.b.get(self.pos) else { bail!("truncated varint") };
            self.pos += 1;
            v |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
            if shift >= 64 {
                bail!("bad varint");
            }
        }
    }
    fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.varint()? as usize;
        let Some(s) = self.b.get(self.pos..self.pos + n) else { bail!("truncated field") };
        self.pos += n;
        Ok(s)
    }
    fn fixed(&mut self, n: usize) -> Result<&'a [u8]> {
        let Some(s) = self.b.get(self.pos..self.pos + n) else { bail!("truncated fixed") };
        self.pos += n;
        Ok(s)
    }
    /// Skips a field of the given wire type.
    fn skip(&mut self, wire: u64) -> Result<()> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => {
                self.fixed(8)?;
            }
            2 => {
                self.bytes()?;
            }
            5 => {
                self.fixed(4)?;
            }
            w => bail!("unsupported wire type {w}"),
        }
        Ok(())
    }
    fn packed(&mut self) -> Result<Vec<u32>> {
        let mut p = Pb::new(self.bytes()?);
        let mut v = Vec::new();
        while !p.done() {
            v.push(p.varint()? as u32);
        }
        Ok(v)
    }
}

fn value(b: &[u8]) -> Result<Value> {
    let mut p = Pb::new(b);
    let mut v = Value::Bool(false);
    while !p.done() {
        let key = p.varint()?;
        v = match (key >> 3, key & 7) {
            (1, 2) => Value::Str(String::from_utf8_lossy(p.bytes()?).into_owned()),
            (2, 5) => Value::Num(f32::from_le_bytes(p.fixed(4)?.try_into()?) as f64),
            (3, 1) => Value::Num(f64::from_le_bytes(p.fixed(8)?.try_into()?)),
            (4, 0) | (5, 0) => Value::Num(p.varint()? as i64 as f64),
            (6, 0) => {
                let n = p.varint()?;
                Value::Num(((n >> 1) as i64 ^ -((n & 1) as i64)) as f64)
            }
            (7, 0) => Value::Bool(p.varint()? != 0),
            (_, w) => {
                p.skip(w)?;
                continue;
            }
        };
    }
    Ok(v)
}

fn zigzag(n: u32) -> i32 {
    ((n >> 1) as i32) ^ -((n & 1) as i32)
}

fn rep_point(geom_type: u32, g: &[u32]) -> (f64, f64) {
    let mut pts: Vec<(i32, i32)> = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    let mut i = 0;
    let mut rings = 0;
    while i < g.len() {
        let cmd = g[i] & 7;
        let count = (g[i] >> 3) as usize;
        i += 1;
        if cmd == 1 && geom_type == 3 {
            rings += 1;
            if rings > 1 {
                break; // outer ring only
            }
        }
        if cmd == 1 || cmd == 2 {
            for _ in 0..count {
                if i + 1 >= g.len() {
                    break;
                }
                x += zigzag(g[i]);
                y += zigzag(g[i + 1]);
                i += 2;
                pts.push((x, y));
            }
        }
        if geom_type == 1 && !pts.is_empty() {
            break;
        }
    }
    match (geom_type, pts.len()) {
        (_, 0) => (0.0, 0.0),
        (2, n) => {
            let p = pts[n / 2];
            (p.0 as f64, p.1 as f64)
        }
        (3, n) => {
            let (sx, sy) = pts.iter().fold((0f64, 0f64), |a, p| (a.0 + p.0 as f64, a.1 + p.1 as f64));
            (sx / n as f64, sy / n as f64)
        }
        _ => (pts[0].0 as f64, pts[0].1 as f64),
    }
}

/// Decodes the layers named in `want` (all layers when empty) from an uncompressed tile.
pub fn decode(tile: &[u8], want: &[&str]) -> Result<Vec<Layer>> {
    let mut out = Vec::new();
    let mut p = Pb::new(tile);
    while !p.done() {
        let key = p.varint()?;
        if key >> 3 != 3 || key & 7 != 2 {
            p.skip(key & 7)?;
            continue;
        }
        let lb = p.bytes()?;
        // first pass: name, so unwanted layers are skipped cheaply
        let mut name = String::new();
        let mut q = Pb::new(lb);
        while !q.done() {
            let k = q.varint()?;
            if k == (1 << 3 | 2) {
                name = String::from_utf8_lossy(q.bytes()?).into_owned();
                break;
            }
            q.skip(k & 7)?;
        }
        if !want.is_empty() && !want.contains(&name.as_str()) {
            continue;
        }
        let (mut keys, mut values, mut raw_feats, mut extent) = (Vec::new(), Vec::new(), Vec::new(), 4096u32);
        let mut q = Pb::new(lb);
        while !q.done() {
            let k = q.varint()?;
            match (k >> 3, k & 7) {
                (2, 2) => raw_feats.push(q.bytes()?),
                (3, 2) => keys.push(String::from_utf8_lossy(q.bytes()?).into_owned()),
                (4, 2) => values.push(value(q.bytes()?)?),
                (5, 0) => extent = q.varint()? as u32,
                (_, w) => q.skip(w)?,
            }
        }
        let mut features = Vec::with_capacity(raw_feats.len());
        for fb in raw_feats {
            let mut f = Pb::new(fb);
            let (mut tags, mut geom, mut geom_type) = (Vec::new(), Vec::new(), 0);
            while !f.done() {
                let k = f.varint()?;
                match (k >> 3, k & 7) {
                    (2, 2) => tags = f.packed()?,
                    (3, 0) => geom_type = f.varint()? as u32,
                    (4, 2) => geom = f.packed()?,
                    (_, w) => f.skip(w)?,
                }
            }
            let props = tags
                .chunks_exact(2)
                .filter_map(|kv| Some((keys.get(kv[0] as usize)?.clone(), values.get(kv[1] as usize)?.clone())))
                .collect();
            features.push(Feature { geom_type, props, point: rep_point(geom_type, &geom) });
        }
        out.push(Layer { name, extent, features });
    }
    Ok(out)
}

/// Tile pixel position -> (lon, lat).
pub fn to_lonlat(z: u8, x: u32, y: u32, extent: u32, px: (f64, f64)) -> (f64, f64) {
    let n = (1u64 << z) as f64;
    let fx = (x as f64 + px.0 / extent as f64) / n;
    let fy = (y as f64 + px.1 / extent as f64) / n;
    let lon = fx * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * fy)).sinh().atan().to_degrees();
    (lon, lat)
}
