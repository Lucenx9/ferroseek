//! Throwaway prototype: compare candidate index backends on FerroSeek's
//! actual workload — case-insensitive substring search over file names,
//! with frequent small durable updates.
//!
//! Usage: backend-bench <sqlite|tantivy|fst|scan> <num_files> <work_dir>
//!
//! Each run is a separate process so that RSS numbers are not polluted by
//! another backend. Output is one line of `key=value` pairs.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- corpus

/// xorshift64* — deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const WORDS: &[&str] = &[
    "report", "invoice", "project", "draft", "final", "notes", "meeting", "budget", "photo",
    "summary", "config", "main", "test", "utils", "index", "readme", "license", "backup",
    "archive", "image", "video", "music", "document", "letter", "resume", "thesis", "chapter",
    "design", "mockup", "logo", "icon", "server", "client", "module", "build", "release",
    "data", "export", "import", "schema", "kernel", "driver", "network", "audio", "screen",
    "holiday", "family", "contract", "receipt", "tax", "plan", "roadmap", "spec", "review",
    "lecture", "slides", "paper", "dataset", "results", "analysis", "chart", "table",
    "ferro", "baloo", "plasma", "krunner", "dolphin", "konsole", "kate", "rust", "python",
];
const EXTS: &[&str] = &[
    "txt", "md", "pdf", "jpg", "png", "rs", "py", "cpp", "h", "json", "toml", "odt", "docx",
    "xlsx", "mp3", "mp4", "html", "css", "js", "ts", "zip", "tar.gz", "svg", "log", "csv",
];

fn gen_name(r: &mut Rng, i: usize) -> String {
    match r.below(10) {
        0 => format!("IMG_2024{:02}{:02}_{:06}.jpg", r.below(12) + 1, r.below(28) + 1, i),
        1 => format!("{}{}.{}", cap(WORDS[r.below(WORDS.len())]), cap(WORDS[r.below(WORDS.len())]), EXTS[r.below(EXTS.len())]),
        2 => format!("{}_{}.{}", WORDS[r.below(WORDS.len())], i % 1000, EXTS[r.below(EXTS.len())]),
        _ => format!(
            "{}-{}-{}.{}",
            WORDS[r.below(WORDS.len())],
            WORDS[r.below(WORDS.len())],
            r.below(10_000),
            EXTS[r.below(EXTS.len())]
        ),
    }
}
fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn corpus(n: usize) -> Vec<String> {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    (0..n).map(|i| gen_name(&mut r, i)).collect()
}

fn queries() -> Vec<String> {
    // Mix of selective and unselective substrings; all lowercase (case
    // insensitivity is the backend's job).
    ["report", "invo", "ferro", "img_2024", "budget-final", "readme", "krun", "tax", "ice", "e-p", "chapter", "zzzz"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

// ---------------------------------------------------------------- metrics

fn rss_kib() -> u64 {
    let s = fs::read_to_string("/proc/self/status").unwrap();
    for l in s.lines() {
        if let Some(v) = l.strip_prefix("VmRSS:") {
            return v.trim().trim_end_matches(" kB").trim().parse().unwrap();
        }
    }
    0
}
fn hwm_kib() -> u64 {
    let s = fs::read_to_string("/proc/self/status").unwrap();
    for l in s.lines() {
        if let Some(v) = l.strip_prefix("VmHWM:") {
            return v.trim().trim_end_matches(" kB").trim().parse().unwrap();
        }
    }
    0
}
fn dir_size(p: &Path) -> u64 {
    let mut t = 0;
    for e in fs::read_dir(p).unwrap() {
        let e = e.unwrap();
        let m = e.metadata().unwrap();
        t += if m.is_dir() { dir_size(&e.path()) } else { m.len() };
    }
    t
}
fn pct(v: &mut [Duration], p: f64) -> f64 {
    v.sort();
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i].as_secs_f64() * 1e3
}

/// A backend must: bulk build, reopen from disk, answer a substring query
/// (top-50 results + total hit count), and apply one durable
/// insert+delete.
trait Backend {
    fn build(dir: &Path, names: &[String]) -> Self
    where
        Self: Sized;
    fn open(dir: &Path) -> Self
    where
        Self: Sized;
    fn query(&mut self, q: &str) -> usize;
    fn update(&mut self, id: u64, name: &str, delete_id: u64);
}

// ---------------------------------------------------------------- sqlite

struct Sqlite {
    c: rusqlite::Connection,
}
impl Sqlite {
    fn conn(dir: &Path) -> rusqlite::Connection {
        let c = rusqlite::Connection::open(dir.join("idx.sqlite")).unwrap();
        let sync = std::env::var("SQLITE_SYNC").unwrap_or_else(|_| "NORMAL".into());
        c.execute_batch(&format!(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous={sync}; PRAGMA cache_size=-2000;"
        ))
        .unwrap();
        c
    }
}
impl Backend for Sqlite {
    fn build(dir: &Path, names: &[String]) -> Self {
        let mut c = Self::conn(dir);
        c.execute_batch(
            "CREATE TABLE entries(id INTEGER PRIMARY KEY, parent INTEGER, name BLOB NOT NULL, key TEXT NOT NULL);
             CREATE INDEX entries_key ON entries(key);
             CREATE VIRTUAL TABLE names USING fts5(key, content='', contentless_delete=1, tokenize='trigram');",
        )
        .unwrap();
        let tx = c.transaction().unwrap();
        {
            let mut a = tx.prepare("INSERT INTO entries(id,parent,name,key) VALUES(?,?,?,?)").unwrap();
            let mut b = tx.prepare("INSERT INTO names(rowid,key) VALUES(?,?)").unwrap();
            for (i, n) in names.iter().enumerate() {
                let k = n.to_lowercase();
                a.execute(rusqlite::params![i as i64, (i / 50) as i64, n.as_bytes(), k]).unwrap();
                b.execute(rusqlite::params![i as i64, k]).unwrap();
            }
        }
        tx.commit().unwrap();
        c.execute_batch("INSERT INTO names(names) VALUES('optimize'); PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        Sqlite { c }
    }
    fn open(dir: &Path) -> Self {
        Sqlite { c: Self::conn(dir) }
    }
    fn query(&mut self, q: &str) -> usize {
        // Trigram FTS for >=3 chars; bounded candidate fetch then
        // name retrieval, like the real engine would do.
        let mut st = self
            .c
            .prepare_cached("SELECT e.name FROM names JOIN entries e ON e.id = names.rowid WHERE names MATCH ?1 LIMIT 2000")
            .unwrap();
        let phrase = format!("\"{}\"", q.replace('"', "\"\""));
        let rows = st.query_map([phrase], |r| r.get::<_, Vec<u8>>(0)).unwrap();
        rows.count()
    }
    fn update(&mut self, id: u64, name: &str, del: u64) {
        let tx = self.c.transaction().unwrap();
        let k = name.to_lowercase();
        tx.execute("INSERT INTO entries(id,parent,name,key) VALUES(?,?,?,?)", rusqlite::params![id as i64, 0, name.as_bytes(), k]).unwrap();
        tx.execute("INSERT INTO names(rowid,key) VALUES(?,?)", rusqlite::params![id as i64, k]).unwrap();
        tx.execute("DELETE FROM entries WHERE id=?", [del as i64]).unwrap();
        tx.execute("DELETE FROM names WHERE rowid=?", [del as i64]).unwrap();
        tx.commit().unwrap();
    }
}

// ---------------------------------------------------------------- tantivy

mod tv {
    use super::*;
    use tantivy::collector::{Count, TopDocs};
    use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
    use tantivy::schema::{
        Field, INDEXED, IndexRecordOption, STORED, Schema, TextFieldIndexing, TextOptions, Value,
    };
    use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};
    use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term, doc};

    pub struct Tv {
        index: Index,
        reader: IndexReader,
        writer: Option<IndexWriter>,
        id: Field,
        name: Field,
    }

    fn schema() -> (Schema, Field, Field) {
        let mut b = Schema::builder();
        let id = b.add_u64_field("id", INDEXED | STORED);
        let opts = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default().set_tokenizer("tri").set_index_option(IndexRecordOption::Basic),
            )
            .set_stored();
        let name = b.add_text_field("name", opts);
        (b.build(), id, name)
    }
    fn register(i: &Index) {
        i.tokenizers().register(
            "tri",
            TextAnalyzer::builder(NgramTokenizer::all_ngrams(3, 3).unwrap()).filter(LowerCaser).build(),
        );
    }
    fn trigrams(q: &str) -> Vec<String> {
        let c: Vec<char> = q.chars().collect();
        if c.len() < 3 {
            return vec![q.to_string()];
        }
        c.windows(3).map(|w| w.iter().collect()).collect()
    }

    impl Backend for Tv {
        fn build(dir: &Path, names: &[String]) -> Self {
            let (s, id, name) = schema();
            let index = Index::create_in_dir(dir, s).unwrap();
            register(&index);
            let mut w: IndexWriter = index.writer_with_num_threads(1, 50_000_000).unwrap();
            for (i, n) in names.iter().enumerate() {
                w.add_document(doc!(id => i as u64, name => n.as_str())).unwrap();
            }
            w.commit().unwrap();
            w.wait_merging_threads().unwrap();
            let reader = index.reader().unwrap();
            Tv { index, reader, writer: None, id, name }
        }
        fn open(dir: &Path) -> Self {
            let index = Index::open_in_dir(dir).unwrap();
            register(&index);
            let (_, id, name) = schema();
            let reader = index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into().unwrap();
            Tv { index, reader, writer: None, id, name }
        }
        fn query(&mut self, q: &str) -> usize {
            let s = self.reader.searcher();
            let subs: Vec<(Occur, Box<dyn Query>)> = trigrams(q)
                .into_iter()
                .map(|t| {
                    (Occur::Must, Box::new(TermQuery::new(Term::from_field_text(self.name, &t), IndexRecordOption::Basic)) as Box<dyn Query>)
                })
                .collect();
            let bq = BooleanQuery::new(subs);
            let (top, _n) = s.search(&bq, &(TopDocs::with_limit(2000).order_by_score(), Count)).unwrap();
            // verify substring (trigram AND is a superset) like real engine
            let mut hits = 0;
            for (_, a) in top {
                let d: TantivyDocument = s.doc(a).unwrap();
                if let Some(v) = d.get_first(self.name).and_then(|v| v.as_str()) {
                    if v.to_lowercase().contains(q) {
                        hits += 1;
                    }
                }
            }
            hits
        }
        fn update(&mut self, i: u64, n: &str, del: u64) {
            if self.writer.is_none() {
                self.writer = Some(self.index.writer_with_num_threads(1, 15_000_000).unwrap());
            }
            let w = self.writer.as_mut().unwrap();
            w.add_document(doc!(self.id => i, self.name => n)).unwrap();
            w.delete_term(Term::from_field_u64(self.id, del));
            w.commit().unwrap();
        }
    }
}

// ---------------------------------------------------------------- fst

mod fstb {
    use super::*;
    use fst::{Automaton, IntoStreamer, Map, MapBuilder, Streamer};

    /// Substring automaton (KMP). An FST cannot prune for substrings, so
    /// this walks every key — which is exactly the property under test.
    #[derive(Clone)]
    pub struct Contains {
        pat: Vec<u8>,
        fail: Vec<usize>,
    }
    impl Contains {
        pub fn new(p: &str) -> Self {
            let pat = p.as_bytes().to_vec();
            let mut fail = vec![0; pat.len()];
            let mut k = 0;
            for i in 1..pat.len() {
                while k > 0 && pat[i] != pat[k] {
                    k = fail[k - 1];
                }
                if pat[i] == pat[k] {
                    k += 1;
                }
                fail[i] = k;
            }
            Contains { pat, fail }
        }
    }
    impl Automaton for Contains {
        type State = usize; // matched prefix length; == len means done
        fn start(&self) -> usize {
            0
        }
        fn is_match(&self, s: &usize) -> bool {
            *s == self.pat.len()
        }
        fn can_match(&self, _: &usize) -> bool {
            true
        }
        fn will_always_match(&self, s: &usize) -> bool {
            *s == self.pat.len()
        }
        fn accept(&self, s: &usize, b: u8) -> usize {
            let mut k = *s;
            if k == self.pat.len() {
                return k;
            }
            while k > 0 && self.pat[k] != b {
                k = self.fail[k - 1];
            }
            if self.pat[k] == b { k + 1 } else { 0 }
        }
    }

    pub struct F {
        map: Map<memmap2::Mmap>,
        dir: std::path::PathBuf,
    }
    impl Backend for F {
        fn build(dir: &Path, names: &[String]) -> Self {
            let mut keys: Vec<(String, u64)> =
                names.iter().enumerate().map(|(i, n)| (format!("{}\0{:x}", n.to_lowercase(), i), i as u64)).collect();
            keys.sort_unstable();
            let w = std::io::BufWriter::new(fs::File::create(dir.join("names.fst")).unwrap());
            let mut b = MapBuilder::new(w).unwrap();
            for (k, v) in keys {
                b.insert(k, v).unwrap();
            }
            b.finish().unwrap();
            Self::open(dir)
        }
        fn open(dir: &Path) -> Self {
            let f = fs::File::open(dir.join("names.fst")).unwrap();
            let m = unsafe { memmap2::Mmap::map(&f).unwrap() };
            F { map: Map::new(m).unwrap(), dir: dir.to_path_buf() }
        }
        fn query(&mut self, q: &str) -> usize {
            let mut s = self.map.search(Contains::new(q)).into_stream();
            let mut n = 0;
            while let Some((_k, _v)) = s.next() {
                n += 1;
                if n >= 2000 {
                    break;
                }
            }
            n
        }
        fn update(&mut self, _i: u64, _n: &str, _d: u64) {
            // FSTs are immutable: an update means a rebuild (or an LSM
            // delta layer that we would have to build ourselves). Measure
            // the honest cost of a full rebuild from the existing keys.
            let mut keys: Vec<(Vec<u8>, u64)> = Vec::new();
            let mut s = self.map.stream();
            while let Some((k, v)) = s.next() {
                keys.push((k.to_vec(), v));
            }
            let tmp = self.dir.join("names.fst.tmp");
            let w = std::io::BufWriter::new(fs::File::create(&tmp).unwrap());
            let mut b = MapBuilder::new(w).unwrap();
            for (k, v) in keys {
                b.insert(k, v).unwrap();
            }
            b.finish().unwrap();
            fs::File::open(&tmp).unwrap().sync_all().unwrap();
            fs::rename(&tmp, self.dir.join("names.fst")).unwrap();
            *self = Self::open(&self.dir);
        }
    }
}

// ---------------------------------------------------------------- scan

/// fsearch-style baseline: all names in RAM, linear scan per query.
/// Persistence is a flat file rewritten on update.
struct Scan {
    names: Vec<String>,
    dir: std::path::PathBuf,
}
impl Backend for Scan {
    fn build(dir: &Path, names: &[String]) -> Self {
        fs::write(dir.join("names.txt"), names.iter().map(|n| n.to_lowercase()).collect::<Vec<_>>().join("\n")).unwrap();
        Self::open(dir)
    }
    fn open(dir: &Path) -> Self {
        let s = fs::read_to_string(dir.join("names.txt")).unwrap();
        Scan { names: s.lines().map(str::to_owned).collect(), dir: dir.to_path_buf() }
    }
    fn query(&mut self, q: &str) -> usize {
        self.names.iter().filter(|n| n.contains(q)).take(2000).count()
    }
    fn update(&mut self, _i: u64, n: &str, _d: u64) {
        self.names.push(n.to_lowercase());
        self.names.swap_remove(0);
        let tmp = self.dir.join("names.tmp");
        fs::write(&tmp, self.names.join("\n")).unwrap();
        fs::File::open(&tmp).unwrap().sync_all().unwrap();
        fs::rename(tmp, self.dir.join("names.txt")).unwrap();
    }
}

// ---------------------------------------------------------------- driver

fn run<B: Backend>(name: &str, n: usize, dir: &Path) {
    let names = corpus(n);
    let rss0 = rss_kib();
    let t = Instant::now();
    let b = B::build(dir, &names);
    let build_s = t.elapsed().as_secs_f64();
    let build_hwm = hwm_kib();
    drop(b);
    drop(names);
    let size = dir_size(dir);

    // Fresh open: what the idle daemon pays.
    let rss_before_open = rss_kib();
    let t = Instant::now();
    let mut b = B::open(dir);
    let open_ms = t.elapsed().as_secs_f64() * 1e3;
    let rss_open = rss_kib();

    let qs = queries();
    for q in &qs {
        b.query(q); // warm
    }
    let mut lat = Vec::new();
    let mut hits: HashMap<String, usize> = HashMap::new();
    for _ in 0..20 {
        for q in &qs {
            let t = Instant::now();
            let h = b.query(q);
            lat.push(t.elapsed());
            hits.insert(q.clone(), h);
        }
    }
    let rss_q = rss_kib();

    // Durable incremental updates (FST/scan: few, they are full rewrites).
    let rounds = if matches!(name, "fst" | "scan") { 5 } else { 200 };
    let mut ul = Vec::new();
    for i in 0..rounds {
        let t = Instant::now();
        b.update((n + i) as u64, &format!("new-file-{i}.txt"), i as u64);
        ul.push(t.elapsed());
    }
    let rss_u = rss_kib();

    let mut hv: Vec<_> = hits.into_iter().collect();
    hv.sort();
    println!(
        "backend={name} n={n} build_s={build_s:.2} build_hwm_mib={:.1} disk_mib={:.1} open_ms={open_ms:.1} \
         rss_base_mib={:.1} rss_open_delta_mib={:.1} rss_after_queries_delta_mib={:.1} rss_after_updates_delta_mib={:.1} \
         q_p50_ms={:.3} q_p95_ms={:.3} q_p99_ms={:.3} upd_p50_ms={:.2} upd_p95_ms={:.2} hits={:?}",
        (build_hwm - rss0.min(build_hwm)) as f64 / 1024.0,
        size as f64 / 1048576.0,
        rss_before_open as f64 / 1024.0,
        (rss_open.saturating_sub(rss_before_open)) as f64 / 1024.0,
        (rss_q.saturating_sub(rss_before_open)) as f64 / 1024.0,
        (rss_u.saturating_sub(rss_before_open)) as f64 / 1024.0,
        pct(&mut lat.clone(), 0.5),
        pct(&mut lat.clone(), 0.95),
        pct(&mut lat, 0.99),
        pct(&mut ul.clone(), 0.5),
        pct(&mut ul, 0.95),
        hv
    );
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a[2].parse().unwrap();
    let dir = Path::new(&a[3]);
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).unwrap();
    match a[1].as_str() {
        "sqlite" => run::<Sqlite>("sqlite", n, dir),
        "tantivy" => run::<tv::Tv>("tantivy", n, dir),
        "fst" => run::<fstb::F>("fst", n, dir),
        "scan" => run::<Scan>("scan", n, dir),
        o => panic!("unknown backend {o}"),
    }
}
