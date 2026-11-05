//! IVF vector index: k-means lists, sign-bit codes for the scan, int8 rerank (DESIGN §7).
//!
//! Files in `index/vec/` (little-endian):
//!   vec.json        n, dims, lists
//!   centroids.f32   lists × dims, unit-norm
//!   offsets.u32     lists + 1 prefix offsets into the list-ordered arrays
//!   ids.u32         n chunk ordinals, list order
//!   codes.bin       n × dims/8 sign bits, MSB first (np.packbits order), list order
//!   i8.bin          n × dims int8, list order: x ≈ i8 / 127 * scale[dim]
//!   scale.f32       dims

use anyhow::{Context, Result, ensure};
use memmap2::Mmap;
use rayon::prelude::*;
use std::fs::File;
use std::path::Path;

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn normalize(v: &mut [f32]) {
    let n = dot(v, v).sqrt().max(1e-12);
    v.iter_mut().for_each(|x| *x /= n);
}

fn nearest(c: &[f32], d: usize, v: &[f32]) -> u32 {
    let mut best = (f32::MIN, 0u32);
    for (j, cj) in c.chunks_exact(d).enumerate() {
        let s = dot(cj, v);
        if s > best.0 {
            best = (s, j as u32);
        }
    }
    best.1
}

/// Spherical k-means on a strided sample; returns unit-norm centroids (k × d).
pub fn kmeans(data: &[f32], d: usize, k: usize, iters: usize, max_sample: usize) -> Vec<f32> {
    let n = data.len() / d;
    let step = (n / max_sample.max(1)).max(1);
    let sample: Vec<&[f32]> = data.chunks_exact(d).step_by(step).collect();
    let k = k.min(sample.len());
    let pick = (sample.len() / k).max(1);
    let mut c: Vec<f32> = (0..k).flat_map(|i| sample[i * pick].iter().copied()).collect();
    for it in 0..iters {
        let assign: Vec<u32> = sample.par_iter().map(|v| nearest(&c, d, v)).collect();
        let mut sums = vec![0f32; k * d];
        let mut counts = vec![0u32; k];
        for (v, &a) in sample.iter().zip(&assign) {
            counts[a as usize] += 1;
            sums[a as usize * d..(a as usize + 1) * d].iter_mut().zip(v.iter()).for_each(|(s, x)| *s += x);
        }
        let mut empty = 0;
        for j in 0..k {
            let cj = &mut sums[j * d..(j + 1) * d];
            if counts[j] == 0 {
                // re-seed an empty list from a deterministic sample point
                cj.copy_from_slice(sample[(j * 7919 + it * 104729) % sample.len()]);
                empty += 1;
            }
            normalize(cj);
        }
        c = sums;
        eprintln!("kmeans iter {}/{iters}: {empty} empty lists", it + 1);
    }
    c
}

/// Builds the index from unit-norm vectors; row i belongs to chunk ordinal `row_ids[i]`.
pub fn build(dir: &Path, data: &[f32], row_ids: &[u32], d: usize, lists: usize) -> Result<serde_json::Value> {
    ensure!(d % 8 == 0, "dims must be a multiple of 8");
    std::fs::create_dir_all(dir)?;
    let n = data.len() / d;
    ensure!(row_ids.len() == n, "row_ids length {} != rows {n}", row_ids.len());
    let c = kmeans(data, d, lists, 12, 300_000);
    let k = c.len() / d;
    let assign: Vec<u32> = data.par_chunks_exact(d).map(|v| nearest(&c, d, v)).collect();

    let mut offsets = vec![0u32; k + 1];
    for &a in &assign {
        offsets[a as usize + 1] += 1;
    }
    for j in 0..k {
        offsets[j + 1] += offsets[j];
    }
    let mut pos = offsets.clone();
    let mut rows = vec![0u32; n]; // list order -> data row
    for (i, &a) in assign.iter().enumerate() {
        rows[pos[a as usize] as usize] = i as u32;
        pos[a as usize] += 1;
    }
    let ids: Vec<u32> = rows.iter().map(|&r| row_ids[r as usize]).collect();

    let mut scale = vec![0f32; d];
    for v in data.chunks_exact(d) {
        for (s, x) in scale.iter_mut().zip(v) {
            *s = s.max(x.abs());
        }
    }
    scale.iter_mut().for_each(|s| *s = s.max(1e-6));

    let cb = d / 8;
    let mut codes = vec![0u8; n * cb];
    let mut q8 = vec![0i8; n * d];
    codes.par_chunks_exact_mut(cb).zip(q8.par_chunks_exact_mut(d)).zip(rows.par_iter()).for_each(|((code, q), &r)| {
        let v = &data[r as usize * d..(r as usize + 1) * d];
        for (j, &x) in v.iter().enumerate() {
            if x > 0.0 {
                code[j / 8] |= 0x80 >> (j % 8);
            }
            q[j] = (x / scale[j] * 127.0).round().clamp(-127.0, 127.0) as i8;
        }
    });

    let f32s = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let u32s = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    std::fs::write(dir.join("centroids.f32"), f32s(&c))?;
    std::fs::write(dir.join("offsets.u32"), u32s(&offsets))?;
    std::fs::write(dir.join("ids.u32"), u32s(&ids))?;
    std::fs::write(dir.join("codes.bin"), &codes)?;
    std::fs::write(dir.join("i8.bin"), q8.iter().map(|&x| x as u8).collect::<Vec<u8>>())?;
    std::fs::write(dir.join("scale.f32"), f32s(&scale))?;
    let sizes: Vec<usize> = offsets.windows(2).map(|w| (w[1] - w[0]) as usize).collect();
    let meta = serde_json::json!({
        "n": n, "dims": d, "lists": k,
        "list_size_max": sizes.iter().max(), "list_size_min": sizes.iter().min(),
    });
    std::fs::write(dir.join("vec.json"), serde_json::to_vec_pretty(&meta)?)?;
    Ok(meta)
}

pub struct VecIndex {
    pub n: usize,
    pub d: usize,
    pub lists: usize,
    centroids: Vec<f32>,
    offsets: Vec<u32>,
    scale: Vec<f32>,
    ids: Mmap,
    codes: Mmap,
    q8: Mmap,
}

fn map(p: &Path) -> Result<Mmap> {
    let f = File::open(p).with_context(|| format!("opening {}", p.display()))?;
    Ok(unsafe { Mmap::map(&f)? })
}

fn read_f32(p: &Path) -> Result<Vec<f32>> {
    Ok(std::fs::read(p)?.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect())
}

pub struct VecParams {
    pub nprobe: usize,
    /// Candidates kept from the binary scan for int8 rerank.
    pub rerank: usize,
}

impl Default for VecParams {
    fn default() -> Self {
        VecParams { nprobe: 32, rerank: 400 }
    }
}

impl VecIndex {
    pub fn open(dir: &Path) -> Result<Self> {
        let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("vec.json"))?)?;
        let offsets = std::fs::read(dir.join("offsets.u32"))?
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        Ok(VecIndex {
            n: meta["n"].as_u64().unwrap_or(0) as usize,
            d: meta["dims"].as_u64().unwrap_or(0) as usize,
            lists: meta["lists"].as_u64().unwrap_or(0) as usize,
            centroids: read_f32(&dir.join("centroids.f32"))?,
            offsets,
            scale: read_f32(&dir.join("scale.f32"))?,
            ids: map(&dir.join("ids.u32"))?,
            codes: map(&dir.join("codes.bin"))?,
            q8: map(&dir.join("i8.bin"))?,
        })
    }

    /// Chunk ordinals in this index (list order).
    pub fn chunk_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.ids.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }

    /// Top-k (chunk ordinal, cosine estimate) for a unit-norm query of `d` dims.
    pub fn search(&self, q: &[f32], k: usize, p: &VecParams) -> Vec<(u32, f32)> {
        let d = self.d;
        let cb = d / 8;
        // probe lists
        let mut cs: Vec<(f32, usize)> =
            self.centroids.chunks_exact(d).enumerate().map(|(j, c)| (dot(c, q), j)).collect();
        let np = p.nprobe.min(cs.len());
        cs.select_nth_unstable_by(np.saturating_sub(1), |a, b| b.0.total_cmp(&a.0));

        // asymmetric binary scan: score = Σ_j q_j · (bit_j ? +1 : -1), via per-byte lookup tables
        let qsum: f32 = q.iter().sum();
        let mut lut = vec![0f32; cb * 256];
        for b in 0..cb {
            for v in 0..256usize {
                let mut s = 0f32;
                for bit in 0..8 {
                    if v & (0x80 >> bit) != 0 {
                        s += q[b * 8 + bit];
                    }
                }
                lut[b * 256 + v] = 2.0 * s;
            }
        }
        let mut cand: Vec<(f32, u32)> = Vec::new();
        for &(_, j) in &cs[..np] {
            let (a, z) = (self.offsets[j] as usize, self.offsets[j + 1] as usize);
            for pos in a..z {
                let code = &self.codes[pos * cb..(pos + 1) * cb];
                let s: f32 = code.iter().enumerate().map(|(b, &v)| lut[b * 256 + v as usize]).sum::<f32>() - qsum;
                cand.push((s, pos as u32));
            }
        }
        let r = p.rerank.min(cand.len());
        if r == 0 {
            return Vec::new();
        }
        cand.select_nth_unstable_by(r - 1, |a, b| b.0.total_cmp(&a.0));
        cand.truncate(r);

        // int8 rerank
        let qs: Vec<f32> = q.iter().zip(&self.scale).map(|(x, s)| x * s / 127.0).collect();
        let mut out: Vec<(u32, f32)> = cand
            .iter()
            .map(|&(_, pos)| {
                let v = &self.q8[pos as usize * d..(pos as usize + 1) * d];
                let s: f32 = v.iter().zip(&qs).map(|(&x, w)| (x as i8) as f32 * w).sum();
                let id = u32::from_le_bytes(self.ids[pos as usize * 4..pos as usize * 4 + 4].try_into().unwrap());
                (id, s)
            })
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out.truncate(k);
        out
    }
}
