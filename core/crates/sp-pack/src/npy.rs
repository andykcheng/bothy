//! Minimal reader for the 2-D `.npy` arrays written by `ingest/embed.py`.

use anyhow::{Context, Result, bail, ensure};
use std::path::Path;

pub struct Npy {
    pub dtype: String,
    pub shape: Vec<usize>,
    pub data: Vec<u8>,
}

impl Npy {
    pub fn load(path: &Path) -> Result<Npy> {
        let buf = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        ensure!(buf.len() > 10 && &buf[0..6] == b"\x93NUMPY", "{} is not an .npy file", path.display());
        let (hlen, hstart) = match buf[6] {
            1 => (u16::from_le_bytes([buf[8], buf[9]]) as usize, 10),
            2 | 3 => (u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize, 12),
            v => bail!("unsupported npy version {v}"),
        };
        let header = std::str::from_utf8(&buf[hstart..hstart + hlen])?;
        let field = |name: &str| -> Result<&str> {
            let i = header.find(&format!("'{name}'")).with_context(|| format!("npy header lacks {name}"))?;
            Ok(header[i + name.len() + 3..].trim_start())
        };
        let dtype = field("descr")?;
        let dtype = dtype[1..].split('\'').next().unwrap().to_string();
        ensure!(field("fortran_order")?.starts_with("False"), "fortran-order npy not supported");
        let shape_s = field("shape")?;
        let shape_s = &shape_s[1..shape_s.find(')').unwrap()];
        let shape = shape_s
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().parse::<usize>())
            .collect::<Result<Vec<_>, _>>()?;
        let data = buf[hstart + hlen..].to_vec();
        Ok(Npy { dtype, shape, data })
    }

    /// Rows as f32, converting from little-endian f16 or f32.
    pub fn to_f32(&self) -> Result<Vec<f32>> {
        match self.dtype.as_str() {
            "<f2" => Ok(self
                .data
                .chunks_exact(2)
                .map(|b| half::f16::from_le_bytes([b[0], b[1]]).to_f32())
                .collect()),
            "<f4" => Ok(self.data.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect()),
            d => bail!("unsupported dtype {d}"),
        }
    }
}
