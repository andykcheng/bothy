//! Tantivy BM25 over chunks (DESIGN §7). Content is English-only (DESIGN §1), so there is one
//! analyzer: simple tokenizer, lower-casing, English stemming.

use anyhow::Result;
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, NumericOptions, Schema, TextFieldIndexing, TextOptions,
};
use tantivy::tokenizer::{LowerCaser, RemoveLongFilter, SimpleTokenizer, Stemmer, TextAnalyzer};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Term, doc};

pub const TOKENIZER: &str = "sp";

fn register(index: &Index, lang: &str) -> Result<()> {
    anyhow::ensure!(lang == "en", "BM25 supports English only (DESIGN §1), not {lang}");
    let analyzer = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(40))
        .filter(LowerCaser)
        .filter(Stemmer::new(tantivy::tokenizer::Language::English))
        .build();
    index.tokenizers().register(TOKENIZER, analyzer);
    Ok(())
}

struct Fields {
    title: Field,
    body: Field,
    chunk: Field,
}

fn schema() -> (Schema, Fields) {
    let mut sb = Schema::builder();
    let text = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default().set_tokenizer(TOKENIZER).set_index_option(IndexRecordOption::WithFreqs),
    );
    let title = sb.add_text_field("title", text.clone());
    let body = sb.add_text_field("body", text);
    let chunk = sb.add_u64_field("chunk", NumericOptions::default().set_fast().set_stored());
    (sb.build(), Fields { title, body, chunk })
}

pub struct Bm25Writer {
    writer: IndexWriter,
    f: Fields,
}

impl Bm25Writer {
    pub fn create(dir: &Path, lang: &str) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let (schema, f) = schema();
        let index = Index::create_in_dir(dir, schema)?;
        register(&index, lang)?;
        let writer = index.writer(2_000_000_000)?;
        Ok(Bm25Writer { writer, f })
    }

    pub fn add(&mut self, chunk: u32, title: &str, body: &str) -> Result<()> {
        self.writer.add_document(doc!(self.f.title => title, self.f.body => body, self.f.chunk => chunk as u64))?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.writer.commit()?;
        self.writer.wait_merging_threads()?;
        Ok(())
    }
}

pub struct Bm25Index {
    reader: IndexReader,
    analyzer: std::sync::Mutex<TextAnalyzer>,
    title: Field,
    body: Field,
}

impl Bm25Index {
    pub fn open(dir: &Path, lang: &str) -> Result<Self> {
        let index = Index::open_in_dir(dir)?;
        register(&index, lang)?;
        let (_, f) = schema();
        let analyzer = index.tokenizers().get(TOKENIZER).expect("registered");
        let reader = index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into()?;
        Ok(Bm25Index { reader, analyzer: std::sync::Mutex::new(analyzer), title: f.title, body: f.body })
    }

    /// Analysed query terms (same analyzer as indexing), deduplicated, in query order.
    pub fn terms(&self, query: &str) -> Vec<String> {
        let mut a = self.analyzer.lock().unwrap();
        let mut ts = a.token_stream(query);
        let mut out: Vec<String> = Vec::new();
        while ts.advance() {
            let t = ts.token().text.clone();
            if !t.trim().is_empty() && !out.contains(&t) {
                out.push(t);
            }
        }
        out
    }

    /// Top-k chunks as (chunk ordinal, BM25 score): OR over analysed terms, title boosted ×2.
    pub fn search(&self, query: &str, k: usize) -> Result<Vec<(u32, f32)>> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for t in self.terms(query) {
            let tq = |f: Field| TermQuery::new(Term::from_field_text(f, &t), IndexRecordOption::WithFreqs);
            clauses.push((Occur::Should, Box::new(BoostQuery::new(Box::new(tq(self.title)), 2.0))));
            clauses.push((Occur::Should, Box::new(tq(self.body))));
        }
        if clauses.is_empty() {
            return Ok(Vec::new());
        }
        let q = BooleanQuery::new(clauses);
        let searcher = self.reader.searcher();
        let top = searcher.search(&q, &TopDocs::with_limit(k).order_by_score())?;
        let mut cols = std::collections::HashMap::new();
        let mut out = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let col = match cols.entry(addr.segment_ord) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(searcher.segment_reader(addr.segment_ord).fast_fields().u64("chunk")?)
                }
            };
            if let Some(c) = col.first(addr.doc_id) {
                out.push((c as u32, score));
            }
        }
        Ok(out)
    }
}
