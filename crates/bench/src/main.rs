// SPDX-License-Identifier: GPL-3.0-or-later
//! FerroSeek benchmark runner.
//!
//!   ferroseek-bench --files N --work DIR [--bin-dir target/release] [--seed S]
//!
//! Generates a deterministic corpus under DIR/corpus, then drives the real
//! `ferroseekd` binary on a *private* D-Bus session (re-executes itself
//! under `dbus-run-session`) with isolated config/data dirs. Prints a
//! Markdown report on stdout. Nothing outside DIR is written.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ferroseek_index::{Query, Reader, SearchOptions};

mod review;

// ------------------------------------------------------------ corpus

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
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[self.below(v.len())]
    }
}

const WORDS: &[&str] = &[
    "report", "invoice", "project", "draft", "final", "notes", "meeting", "budget", "photo", "summary", "config",
    "main", "test", "utils", "index", "readme", "license", "backup", "archive", "image", "video", "music", "document",
    "letter", "resume", "thesis", "chapter", "design", "mockup", "logo", "icon", "server", "client", "module", "build",
    "release", "data", "export", "import", "schema", "kernel", "driver", "network", "audio", "screen", "holiday",
    "family", "contract", "receipt", "tax", "plan", "roadmap", "spec", "review", "lecture", "slides", "paper",
    "dataset", "results", "analysis", "chart", "table", "garden", "recipe", "travel", "wedding", "school", "work",
    "personal", "scan",
];
const EXTS: &[&str] = &[
    "txt", "md", "pdf", "jpg", "png", "rs", "py", "cpp", "h", "json", "toml", "odt", "docx", "xlsx", "mp3", "flac",
    "mp4", "html", "css", "js", "ts", "zip", "tar.gz", "svg", "log", "csv", "kra",
];
const UNICODE: &[&str] = &["Übersicht", "résumé", "日本語", "Ελληνικά", "café", "naïve", "Straße", "русский"];

fn file_name(r: &mut Rng, i: usize) -> Vec<u8> {
    let s = match r.below(20) {
        0..=1 => format!("IMG_20{:02}{:02}{:02}_{:06}.jpg", 15 + r.below(11), r.below(12) + 1, r.below(28) + 1, i),
        2..=4 => {
            let (a, b) = (r.pick(WORDS), r.pick(WORDS));
            format!("{}{}{}.{}", &a[..1].to_uppercase(), &a[1..], cap(b), r.pick(EXTS))
        }
        5..=7 => format!("{}_{}.{}", r.pick(WORDS), i % 997, r.pick(EXTS)),
        8 => format!(".{}rc", r.pick(WORDS)), // hidden: excluded by default
        9 => format!("{} {} ({}).{}", cap(r.pick(WORDS)), r.pick(UNICODE), r.below(9), r.pick(EXTS)),
        10 => {
            // Long name.
            let n = 6 + r.below(20);
            let w: Vec<&str> = (0..n).map(|_| r.pick(WORDS)).collect();
            format!("{}-{i}.{}", w.join("-"), r.pick(EXTS))
        }
        _ => format!("{}-{}-{}.{}", r.pick(WORDS), r.pick(WORDS), r.below(10_000), r.pick(EXTS)),
    };
    s.into_bytes()
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

struct Corpus {
    files: Vec<PathBuf>,
    dirs: usize,
    excluded: usize,
}

/// Directory tree with ~20 files per directory on average, skewed sizes,
/// depth up to ~10, some excluded trees (node_modules, .git), Unicode and a
/// few non-UTF-8 names. File sizes are sparse (`set_len`), so stat() sees
/// realistic sizes without writing data.
fn generate(root: &Path, n: usize, seed: u64) -> Corpus {
    let mut r = Rng(seed | 1);
    fs::create_dir_all(root).unwrap();
    let n_dirs = (n / 20).max(1);
    let mut dirs: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while dirs.len() < n_dirs {
        let (parent, depth) = {
            // Prefer shallow parents so the tree is bushy, with a long tail.
            let i = if r.below(4) == 0 { r.below(dirs.len()) } else { r.below(dirs.len().min(50)) };
            dirs[i].clone()
        };
        if depth >= 10 {
            continue;
        }
        let name = match r.below(12) {
            0 => format!("{} {}", cap(r.pick(WORDS)), r.pick(UNICODE)),
            1 => format!("{}-{}", r.pick(WORDS), 2000 + r.below(26)),
            _ => format!(
                "{}{}",
                cap(r.pick(WORDS)),
                if r.below(3) == 0 { format!("_{}", r.below(100)) } else { String::new() }
            ),
        };
        let p = parent.join(name);
        if fs::create_dir(&p).is_ok() {
            dirs.push((p, depth + 1));
        }
    }
    let mut files = Vec::with_capacity(n);
    let mut excluded = 0;
    let mut i = 0;
    while files.len() < n {
        i += 1;
        // Skew: a few directories get many files.
        let d = if r.below(10) == 0 { r.below(dirs.len().min(20)) } else { r.below(dirs.len()) };
        let mut name = file_name(&mut r, i);
        if r.below(1000) == 0 {
            name.extend_from_slice(b"-\xff\xfe-latin1");
        }
        let p = dirs[d].0.join(OsStr::from_bytes(&name));
        if let Ok(f) = fs::File::create_new(&p) {
            let bits = r.below(20) + 1;
            let _ = f.set_len(r.below(1 << bits) as u64);
            files.push(p);
        }
    }
    // Excluded trees that a naive indexer would walk.
    for k in 0..(n / 2000).max(1) {
        let nm = dirs[r.below(dirs.len())].0.join(format!("webapp{k}/node_modules/pkg/lib"));
        fs::create_dir_all(&nm).unwrap();
        for j in 0..40 {
            fs::write(nm.join(format!("index{j}.js")), b"").unwrap();
            excluded += 1;
        }
        let git = dirs[r.below(dirs.len())].0.join(format!("repo{k}/.git/objects/ab"));
        fs::create_dir_all(&git).unwrap();
        for j in 0..20 {
            fs::write(git.join(format!("{j:038x}")), b"").unwrap();
            excluded += 1;
        }
    }
    Corpus { files, dirs: dirs.len(), excluded }
}

// ------------------------------------------------------------ process metrics

fn proc_status(pid: u32) -> HashMap<String, u64> {
    let mut m = HashMap::new();
    if let Ok(s) = fs::read_to_string(format!("/proc/{pid}/status")) {
        for l in s.lines() {
            if let Some((k, v)) = l.split_once(':')
                && let Some(n) = v.split_whitespace().next().and_then(|x| x.parse().ok())
            {
                m.insert(k.to_string(), n);
            }
        }
    }
    m
}

fn proc_io(pid: u32) -> (u64, u64) {
    let s = fs::read_to_string(format!("/proc/{pid}/io")).unwrap_or_default();
    let get = |k: &str| s.lines().find_map(|l| l.strip_prefix(k)).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    (get("read_bytes:"), get("write_bytes:"))
}

/// utime+stime in clock ticks.
fn proc_cpu_ticks(pid: u32) -> u64 {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let after = s.rsplit_once(')').map(|x| x.1).unwrap_or("");
    let f: Vec<&str> = after.split_whitespace().collect();
    f.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0)
        + f.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0)
}

/// Sum of context switches over all threads (≈ wakeups).
fn proc_ctxt(pid: u32) -> u64 {
    let mut t = 0;
    if let Ok(rd) = fs::read_dir(format!("/proc/{pid}/task")) {
        for e in rd.flatten() {
            let s = fs::read_to_string(e.path().join("status")).unwrap_or_default();
            for l in s.lines() {
                if l.starts_with("voluntary_ctxt_switches") || l.starts_with("nonvoluntary_ctxt_switches") {
                    t += l.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
                }
            }
        }
    }
    t
}

fn ticks_per_sec() -> f64 {
    // SAFETY: plain sysconf.
    unsafe { libc::sysconf(libc::_SC_CLK_TCK) as f64 }
}

// ------------------------------------------------------------ daemon control

struct Daemon {
    child: Child,
    conn: zbus::Connection,
}

const BUS: &str = "io.github.ferroseek.FerroSeek";

impl Daemon {
    fn start(bin: &Path, work: &Path) -> Daemon {
        let child = Command::new(bin.join("ferroseekd"))
            .env("FERROSEEK_CONFIG", work.join("config.toml"))
            .env("FERROSEEK_DATA_DIR", work.join("data"))
            .env("FERROSEEK_LOG", "warn")
            .env_remove("JOURNAL_STREAM")
            .stdout(Stdio::null())
            .stderr(fs::OpenOptions::new().create(true).append(true).open(work.join("daemon.log")).unwrap())
            .spawn()
            .expect("spawn ferroseekd");
        let conn = zbus::block_on(zbus::Connection::session()).expect("private session bus");
        let d = Daemon { child, conn };
        let t = Instant::now();
        while d.status().is_none() {
            assert!(t.elapsed() < Duration::from_secs(30), "daemon did not come up; see daemon.log");
            std::thread::sleep(Duration::from_millis(5));
        }
        d
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn status(&self) -> Option<HashMap<String, String>> {
        zbus::block_on(async {
            let m = self
                .conn
                .call_method(
                    Some(BUS),
                    "/io/github/ferroseek/FerroSeek",
                    Some("io.github.ferroseek.FerroSeek1"),
                    "Status",
                    &(),
                )
                .await
                .ok()?;
            m.body().deserialize::<HashMap<String, String>>().ok()
        })
    }

    fn wait_idle(&self, timeout: Duration) -> Duration {
        let t = Instant::now();
        loop {
            if let Some(s) = self.status()
                && s.get("state").map(String::as_str) == Some("idle")
                && s.get("queued_dirs").map(String::as_str) == Some("0")
            {
                return t.elapsed();
            }
            assert!(t.elapsed() < timeout, "daemon did not become idle");
            // Fine-grained at first, then back off: each Status runs a COUNT
            // over the index and holds a read snapshot, which would delay WAL
            // checkpoints and distort the write measurements.
            std::thread::sleep(if t.elapsed() < Duration::from_secs(2) {
                Duration::from_millis(10)
            } else {
                Duration::from_millis(250)
            });
        }
    }

    /// End-to-end KRunner-equivalent query.
    fn krunner_match(&self, q: &str) -> Vec<String> {
        zbus::block_on(async {
            let m =
                self.conn.call_method(Some(BUS), "/runner", Some("org.kde.krunner1"), "Match", &(q,)).await.unwrap();
            type M = (String, String, String, i32, f64, HashMap<String, zbus::OwnedVariant>);
            m.body().deserialize::<Vec<M>>().unwrap().into_iter().map(|x| x.0).collect()
        })
    }

    fn stop(mut self) {
        // SAFETY: signalling our own child.
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let _ = self.child.wait();
    }

    fn kill9(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ------------------------------------------------------------ stats

fn pct(v: &[f64], p: f64) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    if s.is_empty() {
        return f64::NAN;
    }
    s[(((s.len() - 1) as f64) * p).round() as usize]
}

fn lat(v: &[f64]) -> String {
    format!("p50 {:.2} / p95 {:.2} / p99 {:.2} / max {:.2} ms", pct(v, 0.5), pct(v, 0.95), pct(v, 0.99), pct(v, 1.0))
}

fn mib(kib: u64) -> String {
    format!("{:.1} MiB", kib as f64 / 1024.0)
}

fn file_url(p: &Path) -> String {
    let mut s = String::from("file://");
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn count_hits(db: &Path, q: &str, limit: usize) -> usize {
    Reader::open(db)
        .and_then(|r| r.search(&Query::parse(q), &SearchOptions { limit, ..Default::default() }))
        .map(|h| h.len())
        .expect("benchmark search failed")
}

fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < timeout {
        if f() {
            return Some(t.elapsed());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

// ------------------------------------------------------------ main

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let n: usize = get("--files").and_then(|v| v.parse().ok()).expect("--files N");
    let work = PathBuf::from(get("--work").expect("--work DIR"));
    let bin = PathBuf::from(get("--bin-dir").unwrap_or_else(|| "target/release".into()));
    let seed: u64 = get("--seed").and_then(|v| v.parse().ok()).unwrap_or(42);
    let only_initial = args.iter().any(|a| a == "--only-initial");
    let initial_cpu: u32 = get("--cpu").map(|v| v.parse().expect("--cpu 1..100")).unwrap_or(100);
    assert!((1..=100).contains(&initial_cpu), "--cpu must be 1..100");
    let reuse = args.iter().any(|a| a == "--reuse-corpus");
    let queries_only = args.iter().any(|a| a == "--queries-only");
    assert!(n > 0, "--files must be positive");
    assert!(!queries_only || reuse, "--queries-only requires --reuse-corpus");
    assert!(!reuse || only_initial || queries_only, "--reuse-corpus requires --only-initial or --queries-only");
    if !reuse && work.exists() {
        assert!(
            fs::read_dir(&work).expect("work directory").next().is_none(),
            "--work must be a new or empty directory; existing files are never removed"
        );
    }
    if reuse {
        let report = fs::read_to_string(work.join("report.md")).expect("existing benchmark report required for reuse");
        assert!(
            work.join("corpus").is_dir()
                && work.join("config.toml").is_file()
                && report.starts_with(&format!("### {n} files (seed {seed})")),
            "reuse requires a matching generated benchmark corpus/report/config"
        );
    }
    let bin = fs::canonicalize(&bin).expect("bin dir");

    if std::env::var_os("FERROSEEK_BENCH_ISOLATED").is_none() {
        // Private session bus: never touch the user's real one.
        let st = Command::new("dbus-run-session")
            .arg("--")
            .args(&args)
            .env("FERROSEEK_BENCH_ISOLATED", "1")
            .status()
            .expect("dbus-run-session");
        std::process::exit(st.code().unwrap_or(1));
    }

    fs::create_dir_all(&work).unwrap();
    let work = fs::canonicalize(&work).unwrap();
    let corpus_dir = work.join("corpus");
    let db = work.join("data/index.sqlite");
    let mut out = String::new();
    macro_rules! say { ($($t:tt)*) => {{ let l = format!($($t)*); eprintln!("{l}"); out.push_str(&l); out.push('\n'); }} }

    if queries_only {
        let d = Daemon::start(&bin, &work);
        d.wait_idle(Duration::from_secs(3600));
        let queries: Vec<String> = WORDS.iter().map(|q| q.to_string()).collect();
        for q in &queries {
            d.krunner_match(q);
        }
        let mut times = Vec::new();
        for _ in 0..5 {
            for q in &queries {
                let t = Instant::now();
                let hits = d.krunner_match(q);
                assert!(!hits.is_empty(), "expected corpus word {q}");
                times.push(t.elapsed().as_secs_f64() * 1e3);
            }
        }
        say!("Ordinary word queries D-Bus: {}", lat(&times));
        for q in ["corpus/", "corpus/ report", "home/"] {
            let mut samples = Vec::new();
            d.krunner_match(q);
            for _ in 0..15 {
                let t = Instant::now();
                let hits = d.krunner_match(q);
                samples.push(t.elapsed().as_secs_f64() * 1e3);
                assert!(!hits.is_empty(), "broad query {q}");
            }
            say!("Broad query {q:?} D-Bus: {}", lat(&samples));
        }
        review::run(&d, &work, &db, &mut |l| say!("{l}"));
        d.stop();
        println!("{out}");
        return;
    }

    let t = Instant::now();
    let corpus = if reuse && corpus_dir.exists() {
        // Experiments only: file list is not needed for the initial pass.
        Corpus { files: vec![PathBuf::new(); n], dirs: n / 20, excluded: 0 }
    } else {
        generate(&corpus_dir, n, seed)
    };
    say!("### {} files (seed {seed})\n", n);
    if reuse {
        say!("Reusing an existing corpus; --files is a caller-provided label. Actual indexed counts follow.\n");
    } else {
        say!(
            "Corpus: {} files in {} directories (+{} files in excluded `node_modules`/`.git` trees), generated in {:.1}s.\n",
            corpus.files.len(),
            corpus.dirs,
            corpus.excluded,
            t.elapsed().as_secs_f64()
        );
    }
    let write_cfg = |cpu: u32| {
        fs::write(
            work.join("config.toml"),
            format!("roots = [\"{}\"]\nmax_cpu_percent = {cpu}\n", corpus_dir.display()),
        )
        .unwrap();
    };

    // 1. Initial indexing, default throttle and unthrottled.
    say!("| metric | value |\n|---|---|");
    let passes: &[u32] = if only_initial { &[initial_cpu] } else { &[50, 100] };
    for &cpu in passes {
        let _ = fs::remove_dir_all(work.join("data"));
        write_cfg(cpu);
        let t0 = Instant::now();
        let d = Daemon::start(&bin, &work);
        d.wait_idle(Duration::from_secs(3600));
        let wall = t0.elapsed();
        let st = proc_status(d.pid());
        let (rb, wb) = proc_io(d.pid());
        let cpu_s = proc_cpu_ticks(d.pid()) as f64 / ticks_per_sec();
        let s = d.status().unwrap();
        say!(
            "| initial index, max_cpu_percent={cpu}: wall | {:.2} s ({:.0} files/s) |",
            wall.as_secs_f64(),
            corpus.files.len() as f64 / wall.as_secs_f64()
        );
        say!("| ↳ CPU time (user+sys) | {cpu_s:.2} s |");
        say!("| ↳ peak RSS (VmHWM) | {} |", mib(st["VmHWM"]));
        say!(
            "| ↳ disk read / written (/proc/pid/io) | {:.1} MiB / {:.1} MiB |",
            rb as f64 / 1048576.0,
            wb as f64 / 1048576.0
        );
        say!("| ↳ indexed files / dirs / watched dirs | {} / {} / {} |", s["files"], s["dirs"], s["watched_dirs"]);
        if cpu == 50 || only_initial {
            if only_initial {
                say!(
                    "| index size on disk (db + wal) | {:.1} MiB |",
                    s["disk_bytes"].parse::<f64>().unwrap_or(0.0) / 1048576.0
                );
            }
            d.stop();
        } else {
            // Keep this daemon for the remaining measurements.
            let db_size = fs::metadata(&db).map(|m| m.len()).unwrap_or(0) as f64;
            let wal = fs::metadata(work.join("data/index.sqlite-wal")).map(|m| m.len()).unwrap_or(0) as f64;
            say!(
                "| index size on disk: database / WAL | {:.1} MiB ({:.0} B/entry) / {:.1} MiB |",
                db_size / 1048576.0,
                db_size / (corpus.files.len() + corpus.dirs) as f64,
                wal / 1048576.0
            );
            run_rest(d, &bin, &work, &corpus, &db, n, &mut |l| say!("{l}"));
        }
    }
    fs::write(work.join("report.md"), &out).unwrap();
    println!("{out}");
}

fn run_rest(d: Daemon, bin: &Path, work: &Path, corpus: &Corpus, db: &Path, n: usize, say: &mut dyn FnMut(String)) {
    let pid = d.pid();
    // 2. Idle: let the query pool threads expire, then observe 30 s.
    std::thread::sleep(Duration::from_secs(12));
    let (c0, x0, t0) = (proc_cpu_ticks(pid), proc_ctxt(pid), Instant::now());
    std::thread::sleep(Duration::from_secs(30));
    let secs = t0.elapsed().as_secs_f64();
    let cpu = (proc_cpu_ticks(pid) - c0) as f64 / ticks_per_sec();
    let wake = (proc_ctxt(pid) - x0) as f64 / secs;
    let st = proc_status(pid);
    say(format!(
        "| idle RSS (anon + file-backed) | {} ({} anon, {} file) |",
        mib(st["VmRSS"]),
        mib(st["RssAnon"]),
        mib(st["RssFile"])
    ));
    say(format!("| idle CPU over {secs:.0} s | {:.3} % ({cpu:.2} s) |", 100.0 * cpu / secs));
    say(format!("| idle context switches (all threads) | {wake:.2} /s |"));
    say(format!("| idle threads | {} |", st["Threads"]));

    // 3. Query latency: deterministic query set from the corpus.
    let mut r = Rng(7);
    let mut queries: Vec<String> = Vec::new();
    for _ in 0..300 {
        let f = &corpus.files[r.below(corpus.files.len())];
        let name = f.file_name().unwrap().to_string_lossy().to_lowercase();
        let chars: Vec<char> = name.chars().collect();
        let len = (3 + r.below(6)).min(chars.len());
        let start = r.below(chars.len() - len + 1);
        queries.push(chars[start..start + len].iter().collect());
    }
    for _ in 0..30 {
        queries.push(format!("{} {}", r.pick(WORDS), &r.pick(WORDS)[..3]));
        queries.push(r.pick(WORDS)[..2].to_string());
        queries.push(format!("zq{}x", r.below(1000)));
    }
    let reader = Reader::open(db).unwrap();
    let opts = SearchOptions { limit: 15, ..Default::default() };
    for q in &queries {
        let _ = reader.search(&Query::parse(q), &opts); // warm
    }
    let mut lib_ms = Vec::new();
    for q in &queries {
        let t = Instant::now();
        let _ = reader.search(&Query::parse(q), &opts).unwrap();
        lib_ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let mut dbus_ms = Vec::new();
    for q in &queries {
        let t = Instant::now();
        let _ = d.krunner_match(q);
        dbus_ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    say(format!("| query latency, library (limit 15, {} queries) | {} |", queries.len(), lat(&lib_ms)));
    say(format!("| query latency, D-Bus `Match` end-to-end | {} |", lat(&dbus_ms)));
    let st = proc_status(pid);
    say(format!("| RSS after query burst | {} |", mib(st["VmRSS"])));

    // 4. Relevance on real corpus names.
    let mut top1 = 0;
    let mut top5_stem = 0;
    let mut found_sub = 0;
    let sample = 200.min(corpus.files.len());
    let mut eligible = 0;
    for _ in 0..sample {
        let f = &corpus.files[r.below(corpus.files.len())];
        let name = f.file_name().unwrap();
        if name.as_bytes().first() == Some(&b'.') || name.to_str().is_none() {
            continue; // hidden (excluded) or non-UTF-8: not queryable as text
        }
        eligible += 1;
        let name = name.to_str().unwrap();
        let hits = reader.search(&Query::parse(name), &SearchOptions { limit: 20, ..Default::default() }).unwrap();
        // Exact duplicates of a name in other dirs tie; accept any rank-1 hit with the same name.
        if hits.first().is_some_and(|h| h.path.file_name() == f.file_name()) {
            top1 += 1;
        }
        let stem = name.rsplit_once('.').map(|x| x.0).unwrap_or(name);
        let hits = reader.search(&Query::parse(stem), &SearchOptions { limit: 5, ..Default::default() }).unwrap();
        // Success if the file is in the top 5, or the top 5 are all exact
        // stem ties (then no ranking could place every duplicate there).
        let stem_of = |p: &Path| {
            let n = p.file_name().unwrap().to_string_lossy().to_lowercase();
            n.rsplit_once('.').map(|x| x.0.to_string()).unwrap_or(n)
        };
        let ties = hits.len() == 5 && hits.iter().all(|h| stem_of(&h.path) == stem.to_lowercase());
        if ties || hits.iter().any(|h| h.path.file_name() == f.file_name()) {
            top5_stem += 1;
        }
        let words: Vec<&str> = stem.split(['-', '_', ' ', '(', ')']).filter(|w| w.chars().count() >= 3).collect();
        let q = words.iter().take(2).copied().collect::<Vec<_>>().join(" ");
        let hits = reader.search(&Query::parse(&q), &SearchOptions { limit: 100_000, ..Default::default() }).unwrap();
        if q.is_empty() || hits.iter().any(|h| &h.path == f) {
            found_sub += 1;
        }
    }
    say(format!(
        "| relevance (eligible {eligible} of sample {sample}): exact name ranked #1 / stem in top-5 (ties counted) / multi-word recall | {:.1} % / {:.1} % / {:.1} % |",
        100.0 * top1 as f64 / eligible.max(1) as f64,
        100.0 * top5_stem as f64 / eligible.max(1) as f64,
        100.0 * found_sub as f64 / eligible.max(1) as f64
    ));

    review::run(&d, work, db, say);

    // 5. Incremental latency through D-Bus Match (includes 200 ms debounce).
    let dir = corpus.files[0].parent().unwrap().to_path_buf();
    let (mut c_ms, mut r_ms, mut d_ms) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..30 {
        let a = dir.join(format!("bench-incr-{i}-alpha.txt"));
        let b = dir.join(format!("bench-incr-{i}-omega.txt"));
        let (ua, ub) = (file_url(&a), file_url(&b));
        let t = Instant::now();
        fs::write(&a, b"x").unwrap();
        wait_until(Duration::from_secs(10), || d.krunner_match(&format!("bench-incr-{i}-alpha")).contains(&ua))
            .expect("create");
        c_ms.push(t.elapsed().as_secs_f64() * 1e3);
        let t = Instant::now();
        fs::rename(&a, &b).unwrap();
        wait_until(Duration::from_secs(10), || d.krunner_match(&format!("bench-incr-{i}-omega")).contains(&ub))
            .expect("rename");
        r_ms.push(t.elapsed().as_secs_f64() * 1e3);
        let t = Instant::now();
        fs::remove_file(&b).unwrap();
        wait_until(Duration::from_secs(10), || d.krunner_match(&format!("bench-incr-{i}-omega")).is_empty())
            .expect("delete");
        d_ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    say(format!("| change → visible in `Match`: create (debounce 200 ms) | {} |", lat(&c_ms)));
    say(format!("| ↳ rename | {} |", lat(&r_ms)));
    say(format!("| ↳ delete | {} |", lat(&d_ms)));

    // 6. Bursts: 10k creates, 10k renames, directory rename, 10k deletes.
    let burst = 10_000.min(n);
    let bdir = dir.join("burst-dir");
    fs::create_dir(&bdir).unwrap();
    let hwm_before = proc_status(pid)["VmHWM"];
    let (rb0, wb0) = proc_io(pid);
    let mut peak = 0u64;
    let sample_rss = |peak: &mut u64| *peak = (*peak).max(proc_status(pid).get("VmRSS").copied().unwrap_or(0));
    let t = Instant::now();
    for i in 0..burst {
        fs::write(bdir.join(format!("burstfile-{i:06}.dat")), b"").unwrap();
    }
    let write_time = t.elapsed();
    let done = wait_until(Duration::from_secs(600), || {
        sample_rss(&mut peak);
        count_hits(db, "burstfile-", burst + 10) == burst
    })
    .expect("burst create");
    say(format!(
        "| burst: create {burst} files ({:.2} s to write) → all searchable | {:.2} s after last write |",
        write_time.as_secs_f64(),
        done.as_secs_f64()
    ));
    let t = Instant::now();
    for i in 0..burst {
        fs::rename(bdir.join(format!("burstfile-{i:06}.dat")), bdir.join(format!("renamedfile-{i:06}.dat"))).unwrap();
    }
    let write_time = t.elapsed();
    let done = wait_until(Duration::from_secs(600), || {
        sample_rss(&mut peak);
        count_hits(db, "renamedfile-", burst + 10) == burst && count_hits(db, "burstfile-", 10) == 0
    })
    .expect("burst rename");
    say(format!(
        "| burst: rename {burst} files ({:.2} s) → consistent | {:.2} s after last rename |",
        write_time.as_secs_f64(),
        done.as_secs_f64()
    ));
    let moved = dir.join("burst-dir-moved");
    let t = Instant::now();
    fs::rename(&bdir, &moved).unwrap();
    wait_until(Duration::from_secs(60), || count_hits(db, "burst-dir-moved/ renamedfile-", burst + 10) == burst)
        .expect("dir rename");
    say(format!(
        "| rename directory holding {burst} files → visible under new path | {:.0} ms |",
        t.elapsed().as_secs_f64() * 1e3
    ));
    let t = Instant::now();
    fs::remove_dir_all(&moved).unwrap();
    let write_time = t.elapsed();
    let done = wait_until(Duration::from_secs(600), || {
        sample_rss(&mut peak);
        count_hits(db, "renamedfile-", 10) == 0
    })
    .expect("burst delete");
    say(format!(
        "| delete tree of {burst} files ({:.2} s) → gone from index | {:.2} s after deletion |",
        write_time.as_secs_f64(),
        done.as_secs_f64()
    ));
    let (rb1, wb1) = proc_io(pid);
    let s = d.status().unwrap();
    say(format!(
        "| bursts: peak RSS sampled / VmHWM before→after | {} / {} → {} |",
        mib(peak),
        mib(hwm_before),
        mib(proc_status(pid)["VmHWM"])
    ));
    say(format!(
        "| bursts: disk read / written; inotify overflows | {:.1} MiB / {:.1} MiB; {} |",
        (rb1 - rb0) as f64 / 1048576.0,
        (wb1 - wb0) as f64 / 1048576.0,
        s["overflows"]
    ));

    // 7. Warm restart (graceful stop, unchanged tree, quick reconcile).
    d.stop();
    let t = Instant::now();
    let d = Daemon::start(bin, work);
    d.wait_idle(Duration::from_secs(3600));
    say(format!(
        "| restart on unchanged tree → idle (quick reconcile + watches) | {:.2} s |",
        t.elapsed().as_secs_f64()
    ));
    std::thread::sleep(Duration::from_secs(12));
    let st = proc_status(d.pid());
    say(format!(
        "| steady-state idle after restart: RSS (anon / file), VmHWM | {} ({} / {}), {} |",
        mib(st["VmRSS"]),
        mib(st["RssAnon"]),
        mib(st["RssFile"]),
        mib(st["VmHWM"])
    ));
    let expected: u64 = d.status().unwrap()["files"].parse().unwrap();
    d.stop();

    // 8. Recovery: SIGKILL in the middle of a fresh initial index.
    let _ = fs::remove_dir_all(work.join("data"));
    let d = Daemon::start(bin, work);
    let t = Instant::now();
    wait_until(Duration::from_secs(600), || {
        d.status().and_then(|s| s["files"].parse::<u64>().ok()).unwrap_or(0) > expected / 3
    });
    let killed_at = t.elapsed();
    let partial: u64 = d.status().map(|s| s["files"].parse().unwrap_or(0)).unwrap_or(0);
    d.kill9();
    let t = Instant::now();
    let d = Daemon::start(bin, work);
    d.wait_idle(Duration::from_secs(3600));
    let rec = t.elapsed();
    let after: u64 = d.status().unwrap()["files"].parse().unwrap();
    say(format!(
        "| recovery: SIGKILL after {:.2} s ({partial} files committed) → restart → complete | {:.2} s; {after}/{expected} files {} |",
        killed_at.as_secs_f64(),
        rec.as_secs_f64(),
        if after == expected { "✔" } else { "✘ MISMATCH" }
    ));
    d.stop();
    let _ = std::io::stderr().flush();
}
