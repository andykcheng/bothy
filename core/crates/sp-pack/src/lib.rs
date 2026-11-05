//! Survivalpedia pack format (DESIGN §5): writer and reader.
//!
//! Layout of a pack directory:
//!
//! ```text
//! manifest.json        version, sources, licences, embedding settings, BLAKE3 per file
//! docs-NNN.bin         zstd blocks (~64 KB uncompressed) of JSON doc records, ≤ 4 GB per shard
//! dict-<lang>.zst      zstd dictionary per language
//! docs.idx             header + DocEntry[doc_count]   (little-endian, 32 bytes each)
//! titles.bin           UTF-8 titles, addressed by DocEntry.title_off/len
//! titles.fst           normalised title or redirect + 0x1F + source index -> doc number
//! img-core-NNN.bin     image blobs; images.idx: header + ImageEntry[], sorted by (key, variant)
//! chunks.bin           header + ChunkEntry[chunk_count]: chunk ordinal -> (doc, first/last block)
//! index/               search indexes (sp-search)
//! ```
//!
//! All integers are little-endian. Doc numbers and chunk ordinals are dense u32s assigned at build.

pub mod npy;
pub mod reader;
pub mod writer;

pub use reader::Pack;
pub use writer::PackWriter;

pub const FORMAT_VERSION: u32 = 1;
pub const SHARD_LIMIT: u64 = 4_000_000_000;
pub const BLOCK_TARGET: usize = 64 * 1024;
pub const ZSTD_LEVEL: i32 = 19;
pub const DICT_SIZE: usize = 112 * 1024;

pub const DOCS_MAGIC: &[u8; 4] = b"SPDI";
pub const IMGS_MAGIC: &[u8; 4] = b"SPII";
pub const CHUNKS_MAGIC: &[u8; 4] = b"SPCI";
/// Separator between the normalised title and the source index in titles.fst keys.
pub const TITLE_SEP: u8 = 0x1F;

pub const LANGS: [&str; 3] = ["en", "zh", "ja"];

pub fn lang_code(lang: &str) -> u8 {
    LANGS.iter().position(|l| *l == lang).expect("unknown lang") as u8
}

/// Image variants, in the order stored in `ImageEntry.variant`.
pub const VARIANTS: [&str; 3] = ["320", "640", "svg"];

#[derive(Clone, Copy, Debug, Default)]
pub struct DocEntry {
    pub shard: u16,
    pub lang: u8,
    pub source: u8,
    pub block_clen: u32,
    pub block_off: u64,
    pub rec_off: u32,
    pub rec_len: u32,
    pub title_off: u32,
    pub title_len: u32,
}

impl DocEntry {
    pub const SIZE: usize = 32;

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.shard.to_le_bytes());
        out.push(self.lang);
        out.push(self.source);
        out.extend_from_slice(&self.block_clen.to_le_bytes());
        out.extend_from_slice(&self.block_off.to_le_bytes());
        out.extend_from_slice(&self.rec_off.to_le_bytes());
        out.extend_from_slice(&self.rec_len.to_le_bytes());
        out.extend_from_slice(&self.title_off.to_le_bytes());
        out.extend_from_slice(&self.title_len.to_le_bytes());
    }

    pub fn read(b: &[u8]) -> Self {
        let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        DocEntry {
            shard: u16::from_le_bytes([b[0], b[1]]),
            lang: b[2],
            source: b[3],
            block_clen: u32_at(4),
            block_off: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            rec_off: u32_at(16),
            rec_len: u32_at(20),
            title_off: u32_at(24),
            title_len: u32_at(28),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ImageEntry {
    pub key: [u8; 12],
    pub variant: u8,
    pub shard: u16,
    pub len: u32,
    pub off: u64,
}

impl ImageEntry {
    pub const SIZE: usize = 28;

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.key);
        out.push(self.variant);
        out.push(0);
        out.extend_from_slice(&self.shard.to_le_bytes());
        out.extend_from_slice(&self.len.to_le_bytes());
        out.extend_from_slice(&self.off.to_le_bytes());
    }

    pub fn read(b: &[u8]) -> Self {
        ImageEntry {
            key: b[0..12].try_into().unwrap(),
            variant: b[12],
            shard: u16::from_le_bytes([b[14], b[15]]),
            len: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            off: u64::from_le_bytes(b[20..28].try_into().unwrap()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ChunkEntry {
    pub doc: u32,
    pub block_first: u32,
    pub block_last: u32,
}

impl ChunkEntry {
    pub const SIZE: usize = 12;
}

/// 24-hex-char image key -> 12 bytes.
pub fn parse_image_key(hex: &str) -> Option<[u8; 12]> {
    if hex.len() != 24 {
        return None;
    }
    let mut k = [0u8; 12];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(k)
}

pub fn image_key_hex(k: &[u8; 12]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

/// Title normalisation for lookup: NFKC, lower-case, '_' -> ' ', collapsed whitespace.
pub fn normalize_title(t: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let s: String = t.nfkc().flat_map(char::to_lowercase).map(|c| if c == '_' { ' ' } else { c }).collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Header shared by the binary index files: magic, format version, entry count.
pub fn write_header(out: &mut Vec<u8>, magic: &[u8; 4], count: u64) {
    out.extend_from_slice(magic);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
}

pub const HEADER_SIZE: usize = 16;

pub fn read_header(b: &[u8], magic: &[u8; 4]) -> anyhow::Result<u64> {
    anyhow::ensure!(b.len() >= HEADER_SIZE && &b[0..4] == magic, "bad magic, expected {:?}", magic);
    let ver = u32::from_le_bytes(b[4..8].try_into().unwrap());
    anyhow::ensure!(ver == FORMAT_VERSION, "unsupported format version {ver}");
    Ok(u64::from_le_bytes(b[8..16].try_into().unwrap()))
}
