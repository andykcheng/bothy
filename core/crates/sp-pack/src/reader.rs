use crate::*;
use anyhow::{Context, Result, ensure};
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Pack {
    pub dir: PathBuf,
    pub manifest: serde_json::Value,
    pub sources: Vec<String>,
    docs_idx: Mmap,
    doc_count: usize,
    titles: Mmap,
    titles_fst: fst::Map<Mmap>,
    images_idx: Mmap,
    image_count: usize,
    chunks: Mmap,
    chunk_count: usize,
    dicts: HashMap<u8, zstd::dict::DecoderDictionary<'static>>,
    shards: Mutex<HashMap<(&'static str, u16), File>>,
    block_cache: Mutex<Option<((u16, u64), std::sync::Arc<Vec<u8>>)>>,
}

fn map(p: &Path) -> Result<Mmap> {
    let f = File::open(p).with_context(|| format!("opening {}", p.display()))?;
    Ok(unsafe { Mmap::map(&f)? })
}

pub struct Doc {
    pub num: u32,
    pub title: String,
    pub lang: &'static str,
    pub source: String,
    /// The JSON record: doc_id, title, url, source, lang, license, rev_date, categories, html.
    pub record: serde_json::Value,
}

impl Pack {
    pub fn open(dir: &Path) -> Result<Pack> {
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("manifest.json"))?)?;
        let sources = manifest["sources"]
            .as_array()
            .context("manifest.sources")?
            .iter()
            .map(|s| s["id"].as_str().unwrap_or("").to_string())
            .collect();
        let docs_idx = map(&dir.join("docs.idx"))?;
        let doc_count = read_header(&docs_idx, DOCS_MAGIC)? as usize;
        let images_idx = map(&dir.join("images.idx"))?;
        let image_count = read_header(&images_idx, IMGS_MAGIC)? as usize;
        let chunks = map(&dir.join("chunks.bin"))?;
        let chunk_count = read_header(&chunks, CHUNKS_MAGIC)? as usize;
        let mut dicts = HashMap::new();
        for lang in LANGS {
            let p = dir.join(format!("dict-{lang}.zst"));
            if p.exists() {
                dicts.insert(lang_code(lang), zstd::dict::DecoderDictionary::copy(&std::fs::read(p)?));
            }
        }
        Ok(Pack {
            dir: dir.to_path_buf(),
            manifest,
            sources,
            docs_idx,
            doc_count,
            titles: map(&dir.join("titles.bin"))?,
            titles_fst: fst::Map::new(map(&dir.join("titles.fst"))?)?,
            images_idx,
            image_count,
            chunks,
            chunk_count,
            dicts,
            shards: Mutex::new(HashMap::new()),
            block_cache: Mutex::new(None),
        })
    }

    pub fn doc_count(&self) -> usize {
        self.doc_count
    }

    pub fn chunk_count(&self) -> usize {
        self.chunk_count
    }

    pub fn entry(&self, doc: u32) -> DocEntry {
        let i = HEADER_SIZE + doc as usize * DocEntry::SIZE;
        DocEntry::read(&self.docs_idx[i..i + DocEntry::SIZE])
    }

    pub fn title(&self, doc: u32) -> &str {
        let e = self.entry(doc);
        std::str::from_utf8(&self.titles[e.title_off as usize..(e.title_off + e.title_len) as usize]).unwrap_or("")
    }

    pub fn lang(&self, doc: u32) -> &'static str {
        LANGS[self.entry(doc).lang as usize]
    }

    pub fn source(&self, doc: u32) -> &str {
        &self.sources[self.entry(doc).source as usize]
    }

    fn read_at(&self, prefix: &'static str, shard: u16, off: u64, len: usize) -> Result<Vec<u8>> {
        let mut shards = self.shards.lock().unwrap();
        let f = match shards.entry((prefix, shard)) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(File::open(self.dir.join(format!("{prefix}-{shard:03}.bin")))?)
            }
        };
        let mut buf = vec![0u8; len];
        f.read_exact_at(&mut buf, off)?;
        Ok(buf)
    }

    fn block(&self, e: &DocEntry) -> Result<std::sync::Arc<Vec<u8>>> {
        let key = (e.shard, e.block_off);
        if let Some((k, b)) = self.block_cache.lock().unwrap().as_ref() {
            if *k == key {
                return Ok(b.clone());
            }
        }
        let comp = self.read_at("docs", e.shard, e.block_off, e.block_clen as usize)?;
        let dict = self.dicts.get(&e.lang).context("missing dictionary")?;
        let mut dec = zstd::stream::Decoder::with_prepared_dictionary(&comp[..], dict)?;
        let mut raw = Vec::with_capacity(BLOCK_TARGET * 2);
        dec.read_to_end(&mut raw)?;
        let raw = std::sync::Arc::new(raw);
        *self.block_cache.lock().unwrap() = Some((key, raw.clone()));
        Ok(raw)
    }

    /// Raw JSON bytes of a doc record.
    pub fn record_bytes(&self, doc: u32) -> Result<Vec<u8>> {
        ensure!((doc as usize) < self.doc_count, "doc {doc} out of range");
        let e = self.entry(doc);
        let b = self.block(&e)?;
        Ok(b[e.rec_off as usize..(e.rec_off + e.rec_len) as usize].to_vec())
    }

    pub fn get(&self, doc: u32) -> Result<Doc> {
        let record: serde_json::Value = serde_json::from_slice(&self.record_bytes(doc)?)?;
        Ok(Doc {
            num: doc,
            title: self.title(doc).to_string(),
            lang: self.lang(doc),
            source: self.source(doc).to_string(),
            record,
        })
    }

    /// Exact (normalised) title lookup across sources; redirects included.
    pub fn lookup_title(&self, title: &str) -> Vec<u32> {
        let mut prefix = normalize_title(title).into_bytes();
        prefix.push(TITLE_SEP);
        self.prefix_scan(&prefix, usize::MAX).into_iter().map(|(_, d)| d).collect()
    }

    /// Prefix autocomplete: (normalised key, doc), up to `limit` results.
    pub fn complete(&self, prefix: &str, limit: usize) -> Vec<(String, u32)> {
        self.prefix_scan(normalize_title(prefix).as_bytes(), limit)
            .into_iter()
            .map(|(k, d)| {
                let end = k.iter().position(|&b| b == TITLE_SEP).unwrap_or(k.len());
                (String::from_utf8_lossy(&k[..end]).into_owned(), d)
            })
            .collect()
    }

    fn prefix_scan(&self, prefix: &[u8], limit: usize) -> Vec<(Vec<u8>, u32)> {
        use fst::{IntoStreamer, Streamer};
        let mut upper = prefix.to_vec();
        let mut stream = loop {
            // smallest key greater than every key with this prefix
            match upper.last_mut() {
                Some(b) if *b < 0xFF => {
                    *b += 1;
                    break self.titles_fst.range().ge(prefix).lt(&upper).into_stream();
                }
                Some(_) => {
                    upper.pop();
                }
                None => break self.titles_fst.range().ge(prefix).into_stream(),
            }
        };
        let mut out = Vec::new();
        while let Some((k, v)) = stream.next() {
            out.push((k.to_vec(), v as u32));
            if out.len() >= limit {
                break;
            }
        }
        out
    }

    pub fn image(&self, key: &[u8; 12], variant: &str) -> Result<Option<Vec<u8>>> {
        let want = VARIANTS.iter().position(|v| *v == variant).context("unknown variant")? as u8;
        let (mut lo, mut hi) = (0usize, self.image_count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let i = HEADER_SIZE + mid * ImageEntry::SIZE;
            let e = ImageEntry::read(&self.images_idx[i..i + ImageEntry::SIZE]);
            match (e.key, e.variant).cmp(&(*key, want)) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Ok(Some(self.read_at("img-core", e.shard, e.off, e.len as usize)?)),
            }
        }
        Ok(None)
    }

    pub fn chunk(&self, ordinal: u32) -> ChunkEntry {
        let i = HEADER_SIZE + ordinal as usize * ChunkEntry::SIZE;
        let b = &self.chunks[i..i + ChunkEntry::SIZE];
        let u = |j: usize| u32::from_le_bytes(b[j..j + 4].try_into().unwrap());
        ChunkEntry { doc: u(0), block_first: u(4), block_last: u(8) }
    }
}
