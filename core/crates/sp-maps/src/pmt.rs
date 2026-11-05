//! Minimal PMTiles v3 reading: header, directories, tile ids, and a local mmap archive.
//! Spec: https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md

use anyhow::{Context, Result, bail, ensure};
use memmap2::Mmap;
use pmtiles::{TileCoord, TileId};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const HEADER_LEN: usize = 127;
/// Header + root directory always fit in the first 16 KiB.
pub const ROOT_FETCH: usize = 16384;

pub const COMPRESSION_NONE: u8 = 1;
pub const COMPRESSION_GZIP: u8 = 2;

#[derive(Debug, Clone)]
pub struct Header {
    pub root_offset: u64,
    pub root_length: u64,
    pub metadata_offset: u64,
    pub metadata_length: u64,
    pub leaf_offset: u64,
    pub leaf_length: u64,
    pub data_offset: u64,
    pub data_length: u64,
    pub internal_compression: u8,
    pub tile_compression: u8,
    pub tile_type: u8,
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// min lon, min lat, max lon, max lat
    pub bounds: [f64; 4],
    pub center_zoom: u8,
    pub center: [f64; 2],
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}

fn e7_at(b: &[u8], i: usize) -> f64 {
    i32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as f64 / 1e7
}

impl Header {
    pub fn parse(b: &[u8]) -> Result<Header> {
        ensure!(b.len() >= HEADER_LEN && &b[..7] == b"PMTiles", "not a PMTiles file");
        ensure!(b[7] == 3, "PMTiles version {} (need 3)", b[7]);
        Ok(Header {
            root_offset: u64_at(b, 8),
            root_length: u64_at(b, 16),
            metadata_offset: u64_at(b, 24),
            metadata_length: u64_at(b, 32),
            leaf_offset: u64_at(b, 40),
            leaf_length: u64_at(b, 48),
            data_offset: u64_at(b, 56),
            data_length: u64_at(b, 64),
            internal_compression: b[97],
            tile_compression: b[98],
            tile_type: b[99],
            min_zoom: b[100],
            max_zoom: b[101],
            bounds: [e7_at(b, 102), e7_at(b, 106), e7_at(b, 110), e7_at(b, 114)],
            center_zoom: b[118],
            center: [e7_at(b, 119), e7_at(b, 123)],
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u32,
    /// 0 = this entry points to a leaf directory
    pub run_length: u32,
}

impl Entry {
    pub fn is_leaf(&self) -> bool {
        self.run_length == 0
    }
}

pub fn decompress(data: &[u8], compression: u8) -> Result<Vec<u8>> {
    match compression {
        COMPRESSION_NONE => Ok(data.to_vec()),
        COMPRESSION_GZIP => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(data).read_to_end(&mut out)?;
            Ok(out)
        }
        c => bail!("unsupported PMTiles compression {c}"),
    }
}

fn varint(b: &[u8], pos: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let byte = *b.get(*pos).context("truncated directory")?;
        *pos += 1;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
        shift += 7;
        ensure!(shift < 64, "bad varint");
    }
}

/// Decodes a directory (already decompressed).
pub fn parse_directory(b: &[u8]) -> Result<Vec<Entry>> {
    let mut pos = 0;
    let n = varint(b, &mut pos)? as usize;
    let mut e = vec![Entry { tile_id: 0, offset: 0, length: 0, run_length: 0 }; n];
    let mut id = 0u64;
    for x in e.iter_mut() {
        id += varint(b, &mut pos)?;
        x.tile_id = id;
    }
    for x in e.iter_mut() {
        x.run_length = varint(b, &mut pos)? as u32;
    }
    for x in e.iter_mut() {
        x.length = varint(b, &mut pos)? as u32;
    }
    for i in 0..n {
        let v = varint(b, &mut pos)?;
        e[i].offset = if v == 0 && i > 0 { e[i - 1].offset + e[i - 1].length as u64 } else { v.wrapping_sub(1) };
    }
    Ok(e)
}

/// The entry covering `id`: a tile run containing it, or the leaf directory that may contain it.
pub fn find(entries: &[Entry], id: u64) -> Option<Entry> {
    match entries.binary_search_by(|e| e.tile_id.cmp(&id)) {
        Ok(i) => Some(entries[i]),
        Err(0) => None,
        Err(i) => {
            let p = entries[i - 1];
            (p.is_leaf() || id - p.tile_id < p.run_length as u64).then_some(p)
        }
    }
}

pub fn tile_id(z: u8, x: u32, y: u32) -> Result<u64> {
    Ok(TileId::from(TileCoord::new(z, x, y)?).value())
}

pub fn tile_coord(id: u64) -> Result<TileCoord> {
    Ok(TileCoord::from(TileId::new(id)?))
}

/// Web-mercator tile range covering a lon/lat box at zoom `z`: (x0, y0, x1, y1), inclusive.
pub fn tile_range(bbox: [f64; 4], z: u8) -> (u32, u32, u32, u32) {
    let n = (1u64 << z) as f64;
    let max = (1u32 << z) - 1;
    let x = |lon: f64| (((lon + 180.0) / 360.0 * n).floor().max(0.0) as u32).min(max);
    let y = |lat: f64| {
        let lat = lat.clamp(-85.0511, 85.0511).to_radians();
        ((((1.0 - lat.tan().asinh() / std::f64::consts::PI) / 2.0) * n).floor().max(0.0) as u32).min(max)
    };
    (x(bbox[0]), y(bbox[3]), x(bbox[2]), y(bbox[1]))
}

/// A local PMTiles file, memory-mapped. Leaf directories are decoded on first use and cached.
pub struct Archive {
    pub header: Header,
    mmap: Mmap,
    root: Arc<Vec<Entry>>,
    leaves: Mutex<HashMap<u64, Arc<Vec<Entry>>>>,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Archive> {
        let f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mmap = unsafe { Mmap::map(&f)? };
        let header = Header::parse(&mmap)?;
        let r = header.root_offset as usize..(header.root_offset + header.root_length) as usize;
        let root = Arc::new(parse_directory(&decompress(&mmap[r], header.internal_compression)?)?);
        Ok(Archive { header, mmap, root, leaves: Mutex::new(HashMap::new()) })
    }

    pub fn metadata(&self) -> Result<serde_json::Value> {
        let h = &self.header;
        let r = h.metadata_offset as usize..(h.metadata_offset + h.metadata_length) as usize;
        Ok(serde_json::from_slice(&decompress(&self.mmap[r], h.internal_compression)?)?)
    }

    fn leaf(&self, e: Entry) -> Result<Arc<Vec<Entry>>> {
        if let Some(d) = self.leaves.lock().unwrap().get(&e.offset) {
            return Ok(d.clone());
        }
        let a = (self.header.leaf_offset + e.offset) as usize;
        let d = Arc::new(parse_directory(&decompress(&self.mmap[a..a + e.length as usize], self.header.internal_compression)?)?);
        self.leaves.lock().unwrap().insert(e.offset, d.clone());
        Ok(d)
    }

    /// Every tile entry (tile id, data entry), walking all leaf directories; ids ascending.
    pub fn entries(&self) -> Result<Vec<(u64, Entry)>> {
        let mut out = Vec::new();
        let mut stack: Vec<Arc<Vec<Entry>>> = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            for e in dir.iter() {
                if e.is_leaf() {
                    stack.push(self.leaf(*e)?);
                } else {
                    out.push((e.tile_id, *e));
                }
            }
        }
        out.sort_unstable_by_key(|x| x.0);
        Ok(out)
    }

    /// Bytes of a data entry (as returned by `entries`).
    pub fn data(&self, e: &Entry) -> &[u8] {
        let a = (self.header.data_offset + e.offset) as usize;
        &self.mmap[a..a + e.length as usize]
    }

    /// Raw tile bytes (compressed as `header.tile_compression`), or None if absent.
    pub fn tile(&self, z: u8, x: u32, y: u32) -> Result<Option<&[u8]>> {
        let id = tile_id(z, x, y)?;
        let mut dir = self.root.clone();
        for _ in 0..4 {
            let Some(e) = find(&dir, id) else { return Ok(None) };
            if e.is_leaf() {
                dir = self.leaf(e)?;
                continue;
            }
            let a = (self.header.data_offset + e.offset) as usize;
            return Ok(Some(&self.mmap[a..a + e.length as usize]));
        }
        bail!("directory nesting too deep")
    }
}
