use crate::*;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Appends to numbered shard files (`<prefix>-NNN.bin`), rolling over at SHARD_LIMIT.
struct ShardWriter {
    dir: PathBuf,
    prefix: &'static str,
    shard: u16,
    off: u64,
    file: Option<BufWriter<File>>,
}

impl ShardWriter {
    fn new(dir: &Path, prefix: &'static str) -> Self {
        ShardWriter { dir: dir.to_path_buf(), prefix, shard: 0, off: 0, file: None }
    }

    /// Writes `data` contiguously and returns (shard, offset).
    fn append(&mut self, data: &[u8]) -> Result<(u16, u64)> {
        if self.file.is_some() && self.off + data.len() as u64 > SHARD_LIMIT {
            self.file.take().unwrap().flush()?;
            self.shard += 1;
            self.off = 0;
        }
        if self.file.is_none() {
            let p = self.dir.join(format!("{}-{:03}.bin", self.prefix, self.shard));
            self.file = Some(BufWriter::with_capacity(1 << 20, File::create(&p).with_context(|| p.display().to_string())?));
        }
        self.file.as_mut().unwrap().write_all(data)?;
        let at = (self.shard, self.off);
        self.off += data.len() as u64;
        Ok(at)
    }

    fn finish(&mut self) -> Result<()> {
        if let Some(mut f) = self.file.take() {
            f.flush()?;
        }
        Ok(())
    }
}

pub struct PackWriter {
    dir: PathBuf,
    docs: ShardWriter,
    imgs: ShardWriter,
    dicts: HashMap<u8, zstd::dict::EncoderDictionary<'static>>,
    entries: Vec<DocEntry>,
    titles: Vec<u8>,
    // open block
    block: Vec<u8>,
    block_lang: u8,
    block_docs: Vec<usize>,
    title_keys: Vec<(Vec<u8>, u64)>,
    images: Vec<ImageEntry>,
    pub stats: HashMap<&'static str, u64>,
}

impl PackWriter {
    pub fn create(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(PackWriter {
            dir: dir.to_path_buf(),
            docs: ShardWriter::new(dir, "docs"),
            imgs: ShardWriter::new(dir, "img-core"),
            dicts: HashMap::new(),
            entries: Vec::new(),
            titles: Vec::new(),
            block: Vec::with_capacity(BLOCK_TARGET * 2),
            block_lang: 0,
            block_docs: Vec::new(),
            title_keys: Vec::new(),
            images: Vec::new(),
            stats: HashMap::new(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Trains and stores the zstd dictionary for `lang` from sample records.
    pub fn train_dict(&mut self, lang: &str, samples: &[Vec<u8>]) -> Result<()> {
        // A small collection (a few documents) has too little text to train on: then the samples themselves
        // are the dictionary (zstd "raw content" dictionary), which readers use the same way.
        let dict = match zstd::dict::from_samples(samples, DICT_SIZE) {
            Ok(d) => d,
            Err(_) => {
                let mut raw: Vec<u8> = samples.iter().flatten().copied().collect();
                raw.truncate(DICT_SIZE);
                raw
            }
        };
        std::fs::write(self.dir.join(format!("dict-{lang}.zst")), &dict)?;
        self.dicts.insert(lang_code(lang), zstd::dict::EncoderDictionary::copy(&dict, ZSTD_LEVEL));
        Ok(())
    }

    /// Adds one doc record (JSON bytes) and returns its doc number.
    pub fn add_doc(&mut self, lang: &str, source: u8, title: &str, record: &[u8]) -> Result<u32> {
        let lang = lang_code(lang);
        if !self.block_docs.is_empty() && (lang != self.block_lang || self.block.len() >= BLOCK_TARGET) {
            self.flush_block()?;
        }
        self.block_lang = lang;
        let n = self.entries.len();
        self.entries.push(DocEntry {
            lang,
            source,
            rec_off: self.block.len() as u32,
            rec_len: record.len() as u32,
            title_off: self.titles.len() as u32,
            title_len: title.len() as u32,
            ..Default::default()
        });
        self.titles.extend_from_slice(title.as_bytes());
        self.block.extend_from_slice(record);
        self.block_docs.push(n);
        Ok(n as u32)
    }

    pub fn flush_block(&mut self) -> Result<()> {
        if self.block_docs.is_empty() {
            return Ok(());
        }
        let dict = self.dicts.get(&self.block_lang).context("no dictionary for language")?;
        let mut enc = zstd::stream::Encoder::with_prepared_dictionary(Vec::new(), dict)?;
        enc.write_all(&self.block)?;
        let comp = enc.finish()?;
        let (shard, off) = self.docs.append(&comp)?;
        for &i in &self.block_docs {
            let e = &mut self.entries[i];
            e.shard = shard;
            e.block_off = off;
            e.block_clen = comp.len() as u32;
        }
        *self.stats.entry("doc_bytes_raw").or_default() += self.block.len() as u64;
        *self.stats.entry("doc_bytes_zstd").or_default() += comp.len() as u64;
        *self.stats.entry("doc_blocks").or_default() += 1;
        self.block.clear();
        self.block_docs.clear();
        Ok(())
    }

    /// Registers a lookup key (title or redirect) for `doc`; duplicates keep the first doc.
    pub fn add_title(&mut self, title: &str, source: u8, doc: u32) {
        let mut k = normalize_title(title).into_bytes();
        k.push(TITLE_SEP);
        k.push(source);
        self.title_keys.push((k, doc as u64));
    }

    pub fn add_image(&mut self, key: [u8; 12], variant: u8, data: &[u8]) -> Result<()> {
        let (shard, off) = self.imgs.append(data)?;
        self.images.push(ImageEntry { key, variant, shard, len: data.len() as u32, off });
        *self.stats.entry("image_bytes").or_default() += data.len() as u64;
        Ok(())
    }

    pub fn write_chunks(&self, chunks: &[ChunkEntry]) -> Result<()> {
        let mut out = Vec::with_capacity(HEADER_SIZE + chunks.len() * ChunkEntry::SIZE);
        write_header(&mut out, CHUNKS_MAGIC, chunks.len() as u64);
        for c in chunks {
            out.extend_from_slice(&c.doc.to_le_bytes());
            out.extend_from_slice(&c.block_first.to_le_bytes());
            out.extend_from_slice(&c.block_last.to_le_bytes());
        }
        std::fs::write(self.dir.join("chunks.bin"), out)?;
        Ok(())
    }

    pub fn doc_count(&self) -> usize {
        self.entries.len()
    }

    /// Flushes docs, titles and images. The manifest is written separately (`write_manifest`).
    pub fn finish(&mut self) -> Result<()> {
        self.flush_block()?;
        self.docs.finish()?;
        self.imgs.finish()?;

        let mut idx = Vec::with_capacity(HEADER_SIZE + self.entries.len() * DocEntry::SIZE);
        write_header(&mut idx, DOCS_MAGIC, self.entries.len() as u64);
        for e in &self.entries {
            e.write(&mut idx);
        }
        std::fs::write(self.dir.join("docs.idx"), idx)?;
        std::fs::write(self.dir.join("titles.bin"), &self.titles)?;

        self.title_keys.sort();
        self.title_keys.dedup_by(|a, b| a.0 == b.0);
        let mut fb = fst::MapBuilder::new(BufWriter::new(File::create(self.dir.join("titles.fst"))?))?;
        for (k, v) in &self.title_keys {
            fb.insert(k, *v)?;
        }
        fb.finish()?;
        self.stats.insert("title_keys", self.title_keys.len() as u64);

        self.images.sort_by(|a, b| (a.key, a.variant).cmp(&(b.key, b.variant)));
        let mut ii = Vec::with_capacity(HEADER_SIZE + self.images.len() * ImageEntry::SIZE);
        write_header(&mut ii, IMGS_MAGIC, self.images.len() as u64);
        for e in &self.images {
            e.write(&mut ii);
        }
        std::fs::write(self.dir.join("images.idx"), ii)?;
        Ok(())
    }
}

/// BLAKE3 and size of every file under `dir` (relative paths), excluding manifest.json.
pub fn checksum_files(dir: &Path) -> Result<Vec<(String, u64, String)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let rel = p.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
            if rel == "manifest.json" {
                continue;
            }
            let mut h = blake3::Hasher::new();
            h.update_mmap(&p)?;
            out.push((rel, p.metadata()?.len(), h.finalize().to_hex().to_string()));
        }
    }
    out.sort();
    Ok(out)
}
