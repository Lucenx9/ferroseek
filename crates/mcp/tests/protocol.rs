// SPDX-License-Identifier: GPL-3.0-or-later
//! Protocol-level tests: the real `ferroseek-mcp` binary over stdio
//! (newline-delimited JSON-RPC, as MCP clients speak it), against an index
//! built by the real engine from a temp tree. Nothing outside temp dirs.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use ferroseek_engine::{Config, Paths, index_once};
use serde_json::{Value, json};

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
    initialization: Value,
}

impl Mcp {
    fn spawn(args: &[&str], home: &Path) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ferroseek-mcp"))
            .args(args)
            // Never read the user's real config.
            .env("XDG_CONFIG_HOME", home.join("xdg-config"))
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut m = Mcp { child, stdin, stdout, next: 1, initialization: Value::Null };
        let init = m.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "ferroseek", "{init}");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        m.initialization = init["result"].clone();
        m.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        m
    }

    fn send(&mut self, v: &Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.read_response(id)
    }

    fn read_response(&mut self, id: u64) -> Value {
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "server closed stdout");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) {
                return v;
            }
        }
    }

    /// `tools/call` → (is_error, structured content or error text).
    fn call(&mut self, tool: &str, args: Value) -> (bool, Value) {
        let r = self.request("tools/call", json!({"name": tool, "arguments": args}));
        let res = &r["result"];
        assert!(res.is_object(), "protocol error: {r}");
        let err = res["isError"].as_bool().unwrap_or(false);
        if err {
            (true, res["content"][0]["text"].clone())
        } else {
            // Structured output is mirrored as text for older clients.
            let text: Value = serde_json::from_str(res["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(text, res["structuredContent"]);
            (false, res["structuredContent"].clone())
        }
    }

    fn paths(v: &Value) -> Vec<String> {
        let mut p: Vec<String> =
            v["results"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap().into()).collect();
        p.sort();
        p
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `index_once`, retrying on `Busy`: tests in this process run in parallel
/// and fork children; between fork and exec a child briefly shares another
/// test's flock'd file description, so a lock can outlive its holder by a
/// moment. (Never an issue for the real daemon, which is one process.)
fn index(cfg: &Config, paths: &Paths) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match index_once(cfg, paths, true, false) {
            Ok(_) => return,
            Err(ferroseek_engine::EngineError::Busy) if Instant::now() < end => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("{e}"),
        }
    }
}

struct Fx {
    t: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
}

fn touch(p: &Path, content: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

/// root/work (to be allowed) and root/private (indexed, never allowed).
fn fixture() -> Fx {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("home");
    touch(&root.join("work/reports/quarterly-report.pdf"), b"pdf");
    touch(&root.join("work/reports/annual report 2024.odt"), b"odt");
    touch(&root.join("work/src/main.rs"), b"fn main(){}");
    touch(&root.join("work/deploy/id_ed25519"), b"KEY");
    touch(&root.join("work/deploy/server-report.pem"), b"PEM");
    touch(&root.join("work/.hidden-report.txt"), b"h");
    touch(&root.join("work/.git/report-config"), b"g");
    touch(&root.join("private/tax-report.pdf"), b"secret");
    let paths = Paths { config_file: t.path().join("config.toml"), data_dir: t.path().join("data") };
    let cfg = Config { roots: vec![root.to_string_lossy().into_owned()], index_hidden: true, ..Config::default() };
    index(&cfg, &paths);
    Fx { db: paths.index_file(), root, t }
}

impl Fx {
    fn mcp(&self, extra: &[&str]) -> Mcp {
        let allow = self.root.join("work");
        let mut args = vec!["--index", self.db.to_str().unwrap(), "--allow", allow.to_str().unwrap()];
        args.extend_from_slice(extra);
        Mcp::spawn(&args, self.t.path())
    }
}

#[test]
fn tools_are_listed_read_only_with_output_schemas() {
    let fx = fixture();
    let mut m = fx.mcp(&[]);
    let r = m.request("tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["get_file_info", "index_status", "search_files"]);
    for t in tools {
        assert_eq!(t["annotations"]["readOnlyHint"], true, "{t}");
        assert!(t["outputSchema"].is_object(), "{t}");
        assert!(t["inputSchema"].is_object(), "{t}");
    }
}

#[test]
fn discovery_metadata_explains_scope_fallbacks_and_query_syntax() {
    let fx = fixture();
    let mut m = fx.mcp(&[]);
    let instructions = m.initialization["instructions"].as_str().unwrap();
    assert!(instructions.chars().count() <= 512, "critical guidance must fit the first 512 characters");
    for concept in ["repositories", "Authorized indexed", "no file-content", "new/unindexed", "Empty results"] {
        assert!(instructions.contains(concept), "missing initialization guidance: {concept}");
    }
    let r = m.request("tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().unwrap();
    let tool = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap();
    let search = tool("search_files");
    let description = search["description"].as_str().unwrap();
    for concept in
        ["filename", "partial path", "repositories", "No file-content", "fd", "rg --files", "indexing can lag"]
    {
        assert!(description.contains(concept), "missing tool guidance: {concept}");
    }
    let params = &search["inputSchema"]["properties"];
    assert!(params["query"]["description"].as_str().unwrap().contains("No glob or regex"));
    assert!(params["root"]["description"].as_str().unwrap().contains("omit"));
    assert!(tool("get_file_info")["description"].as_str().unwrap().contains("Not a general filesystem stat"));
    let status = tool("index_status")["description"].as_str().unwrap();
    assert!(status.contains("only if enabled by policy"));
    assert!(status.contains("not indexed coverage"));
}

#[test]
fn search_is_confined_to_allowed_dirs_and_filters_sensitive_names() {
    let fx = fixture();
    let mut m = fx.mcp(&[]);
    let (err, v) = m.call("search_files", json!({"query": "report"}));
    assert!(!err, "{v}");
    let w = fx.root.join("work");
    assert_eq!(
        Mcp::paths(&v),
        vec![
            w.join("reports").display().to_string(),
            w.join("reports/annual report 2024.odt").display().to_string(),
            w.join("reports/quarterly-report.pdf").display().to_string(),
        ],
        "no private/, no .hidden, no .git, no *.pem"
    );
    let first = &v["results"][0];
    assert!(first["kind"].is_string() && first["modified"].as_str().unwrap().ends_with('Z'));
    assert_eq!(v["more_available"], false);

    let (_, v) = m.call("search_files", json!({"query": "id_ed25519"}));
    assert!(v["results"].as_array().unwrap().is_empty(), "private keys are hidden");
    let (_, v) = m.call("search_files", json!({"query": "tax-report"}));
    assert!(v["results"].as_array().unwrap().is_empty(), "indexed but not authorized");

    // Root filter inside an allowed dir works; outside is refused without
    // echoing the path back.
    let (err, v) = m.call("search_files", json!({"query": "report", "root": w.join("reports").to_str().unwrap()}));
    assert!(!err);
    assert_eq!(Mcp::paths(&v).len(), 2);
    for bad in [
        fx.root.join("private").display().to_string(),
        format!("{}/../private", w.display()),
        "relative/dir".into(),
        "/".into(),
    ] {
        let (err, msg) = m.call("search_files", json!({"query": "report", "root": bad}));
        assert!(err, "{bad}");
        assert!(!msg.as_str().unwrap().contains("private"), "{msg}");
    }
}

#[test]
fn get_file_info_reports_freshness_and_refuses_unauthorized_paths() {
    let fx = fixture();
    let mut m = fx.mcp(&[]);
    let f = fx.root.join("work/src/main.rs");
    let (err, v) = m.call("get_file_info", json!({"path": f.to_str().unwrap()}));
    assert!(!err, "{v}");
    assert_eq!(v["kind"], "file");
    assert_eq!(v["size"], 11);
    assert_eq!(v["exists"], true);
    assert_eq!(v["up_to_date"], true);

    // Changed on disk but not re-indexed (no daemon here): fresh metadata,
    // flagged as not up to date.
    fs::write(&f, b"fn main() { println!(); }").unwrap();
    let (_, v) = m.call("get_file_info", json!({"path": f.to_str().unwrap()}));
    assert_eq!(v["size"], 25);
    assert_eq!(v["up_to_date"], false);

    let (_, v) = m.call("get_file_info", json!({"path": fx.root.join("work").to_str().unwrap()}));
    assert_eq!(v["kind"], "directory", "an allowed dir itself can be inspected");

    for (path, why) in [
        (fx.root.join("private/tax-report.pdf").display().to_string(), "not allowed"),
        (fx.root.join("work/deploy/id_ed25519").display().to_string(), "denied name"),
        (fx.root.join("work/.git/report-config").display().to_string(), "hidden"),
        (format!("{}/work/../private/tax-report.pdf", fx.root.display()), "traversal"),
        (fx.root.join("work/never-existed.txt").display().to_string(), "not indexed"),
        ("".into(), "empty"),
    ] {
        let (err, msg) = m.call("get_file_info", json!({"path": path}));
        assert!(err, "{why}: {msg}");
        assert!(!msg.as_str().unwrap().contains("tax-report"), "{why}: {msg}");
    }
}

#[test]
fn stale_entries_are_dropped_and_counted() {
    let fx = fixture();
    let mut m = fx.mcp(&[]);
    fs::remove_file(fx.root.join("work/reports/quarterly-report.pdf")).unwrap();
    let (_, v) = m.call("search_files", json!({"query": "quarterly"}));
    assert!(v["results"].as_array().unwrap().is_empty());
    assert_eq!(v["omitted_stale"], 1);
}

#[test]
fn index_status_lists_only_allowed_dirs() {
    let fx = fixture();
    let missing = fx.root.join("not-indexed-dir");
    let mut m = fx.mcp(&["--allow", missing.to_str().unwrap()]);
    let (err, v) = m.call("index_status", json!({}));
    assert!(!err);
    assert_eq!(v["available"], true);
    let dirs = v["allowed_dirs"].as_array().unwrap();
    assert_eq!(dirs.len(), 2);
    assert_eq!(dirs.iter().filter(|d| d["indexed"] == true).count(), 1);
    assert!(!v.to_string().contains("private"), "{v}");
    // Review finding: the database path and whole-index totals describe
    // things outside the allowlist; they are not reported by default.
    assert!(!v.to_string().contains(fx.db.to_str().unwrap()), "{v}");
    for k in ["index_path", "indexed_files", "indexed_directories", "index_size_bytes", "last_write"] {
        assert!(v.get(k).is_none(), "{k} must be opt-in: {v}");
    }
    drop(m);
    let cfg = fx.t.path().join("totals.toml");
    fs::write(&cfg, "expose_index_totals = true\n").unwrap();
    let mut m = fx.mcp(&["--config", cfg.to_str().unwrap()]);
    let (_, v) = m.call("index_status", json!({}));
    assert_eq!(v["indexed_files"], 7, ".git is excluded by the indexer itself");
    assert!(v["last_write"].as_str().unwrap().ends_with('Z'));
    assert!(!v.to_string().contains(fx.db.to_str().unwrap()), "path never exposed: {v}");
}

#[test]
fn limits_and_response_size_are_bounded() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("home");
    let long = "x".repeat(200);
    for i in 0..400 {
        touch(&root.join(format!("many/bulkfile-{i:04}-{long}.txt")), b"");
    }
    let paths = Paths { config_file: t.path().join("c.toml"), data_dir: t.path().join("data") };
    index_once(&Config { roots: vec![root.to_string_lossy().into()], ..Config::default() }, &paths, true, false)
        .unwrap();
    let mut m =
        Mcp::spawn(&["--index", paths.index_file().to_str().unwrap(), "--allow", root.to_str().unwrap()], t.path());
    let (_, v) = m.call("search_files", json!({"query": "bulkfile", "limit": 5}));
    assert_eq!(v["results"].as_array().unwrap().len(), 5);
    assert_eq!(v["more_available"], true);
    let (_, v) = m.call("search_files", json!({"query": "bulkfile", "limit": 1_000_000}));
    assert_eq!(v["results"].as_array().unwrap().len(), 50, "clamped to the default max_results");
    assert_eq!(v["truncated_by_size"], false);
    drop(m);

    // With a higher configured maximum, the byte budget is what stops it.
    let cfg = t.path().join("mcp.toml");
    fs::write(&cfg, "max_results = 200\n").unwrap();
    let mut m = Mcp::spawn(
        &[
            "--index",
            paths.index_file().to_str().unwrap(),
            "--allow",
            root.to_str().unwrap(),
            "--config",
            cfg.to_str().unwrap(),
        ],
        t.path(),
    );
    let (_, v) = m.call("search_files", json!({"query": "bulkfile", "limit": 200}));
    let n = v["results"].as_array().unwrap().len();
    assert!(n < 200, "byte budget must cut long-path results: {n}");
    assert_eq!(v["truncated_by_size"], true);
    assert!(serde_json::to_string(&v["results"]).unwrap().len() <= 24 * 1024);
}

#[test]
fn invalid_queries_and_unknown_tools_fail_gracefully() {
    let fx = fixture();
    let mut m = fx.mcp(&[]);
    for q in ["", "   ", "a\u{0}b", "line\nbreak", &"q".repeat(600)] {
        let (err, _) = m.call("search_files", json!({"query": q}));
        assert!(err, "{q:?}");
    }
    let r = m.request("tools/call", json!({"name": "read_file", "arguments": {"path": "/etc/passwd"}}));
    assert!(r["error"].is_object() || r["result"]["isError"] == true, "{r}");
    let r = m.request("tools/call", json!({"name": "search_files", "arguments": {"limit": "many"}}));
    assert!(r["error"].is_object() || r["result"]["isError"] == true, "{r}");
    // Still healthy afterwards.
    let (err, _) = m.call("search_files", json!({"query": "main"}));
    assert!(!err);
}

#[test]
fn no_allowlist_means_no_access() {
    let fx = fixture();
    let mut m = Mcp::spawn(&["--index", fx.db.to_str().unwrap()], fx.t.path());
    let (err, msg) = m.call("search_files", json!({"query": "report"}));
    assert!(err && msg.as_str().unwrap().contains("--allow"), "{msg}");
    let (err, _) = m.call("get_file_info", json!({"path": fx.root.join("work/src/main.rs").to_str().unwrap()}));
    assert!(err);
    let (_, v) = m.call("index_status", json!({}));
    assert!(v["allowed_dirs"].as_array().unwrap().is_empty());
}

#[test]
fn missing_index_is_reported_and_recovered_without_restart() {
    let fx = fixture();
    let later = fx.t.path().join("later/index.sqlite");
    let allow = fx.root.join("work");
    let mut m = Mcp::spawn(&["--index", later.to_str().unwrap(), "--allow", allow.to_str().unwrap()], fx.t.path());
    let (err, v) = m.call("index_status", json!({}));
    assert!(!err);
    assert_eq!(v["available"], false);
    assert!(v["error"].as_str().unwrap().contains("unavailable"));
    assert!(!v.to_string().contains("later"), "no database path in errors: {v}");
    let (err, msg) = m.call("search_files", json!({"query": "report"}));
    assert!(err && msg.as_str().unwrap().contains("unavailable"), "{msg}");
    assert!(!msg.as_str().unwrap().contains("later"), "{msg}");
    // The index appears (daemon started): no MCP restart needed.
    fs::create_dir_all(later.parent().unwrap()).unwrap();
    fs::copy(&fx.db, &later).unwrap();
    let (err, v) = m.call("search_files", json!({"query": "quarterly"}));
    assert!(!err, "{v}");
    assert_eq!(v["results"].as_array().unwrap().len(), 1);
}

#[test]
fn queries_succeed_while_the_index_is_being_written() {
    let fx = fixture();
    let mut m = fx.mcp(&[]);
    let work = fx.root.join("work/churn");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let (stop, work, root, data) = (stop.clone(), work.clone(), fx.root.clone(), fx.t.path().join("data"));
        std::thread::spawn(move || {
            let paths = Paths { config_file: data.join("none.toml"), data_dir: data };
            let cfg = Config { roots: vec![root.to_string_lossy().into()], index_hidden: true, ..Config::default() };
            let mut i = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                touch(&work.join(format!("churn-{i}.txt")), b"x");
                // Parallel tests fork children; between fork and exec a child
                // briefly shares the flock'd file description, so the lock can
                // outlive our previous index_once by a moment.
                match index_once(&cfg, &paths, false, false) {
                    Ok(_) => i += 1,
                    Err(ferroseek_engine::EngineError::Busy) => std::thread::sleep(Duration::from_millis(5)),
                    Err(e) => panic!("{e}"),
                }
            }
            i
        })
    };
    let end = Instant::now() + Duration::from_secs(2);
    let mut calls = 0;
    while Instant::now() < end {
        let (err, v) = m.call("search_files", json!({"query": "churn"}));
        assert!(!err, "{v}");
        calls += 1;
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let commits = writer.join().unwrap();
    assert!(calls > 10 && commits > 2, "calls {calls}, commits {commits}");
}

#[test]
fn process_exits_when_the_client_disconnects() {
    let fx = fixture();
    let mut m = fx.mcp(&[]);
    let Mcp { child, stdin, .. } = &mut m;
    drop(std::mem::replace(stdin, unsafe_dummy_stdin()));
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < end, "server did not exit after stdin EOF");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A throwaway pipe end, so the real stdin can be closed while `Mcp` keeps
/// a valid field.
fn unsafe_dummy_stdin() -> ChildStdin {
    let mut c = Command::new("true").stdin(Stdio::piped()).spawn().unwrap();
    let s = c.stdin.take().unwrap();
    let _ = c.wait();
    s
}

#[test]
fn allowing_the_index_root_or_an_ancestor_uses_the_unscoped_path_safely() {
    let fx = fixture();
    let report = |m: &mut Mcp| Mcp::paths(&m.call("search_files", json!({"query": "report"})).1);
    // Exactly the index root: private/ is now authorized too.
    let mut m = Mcp::spawn(&["--index", fx.db.to_str().unwrap(), "--allow", fx.root.to_str().unwrap()], fx.t.path());
    let p = report(&mut m);
    assert!(p.iter().any(|x| x.ends_with("private/tax-report.pdf")), "{p:?}");
    let below = |x: &String| x.strip_prefix(fx.root.to_str().unwrap()).unwrap().to_string();
    assert!(!p.iter().map(below).any(|x| x.contains("/.") || x.ends_with(".pem")), "{p:?}");
    assert_eq!(p.len(), 4, "{p:?}");
    drop(m);
    // An ancestor of the root (like `--allow ~` with roots below it).
    let mut m =
        Mcp::spawn(&["--index", fx.db.to_str().unwrap(), "--allow", fx.t.path().to_str().unwrap()], fx.t.path());
    assert_eq!(report(&mut m).len(), p.len());
}

#[test]
fn a_scope_covering_one_of_two_roots_never_leaks_the_other() {
    let t = tempfile::tempdir().unwrap();
    let (a, b) = (t.path().join("a/root-a"), t.path().join("b/root-b"));
    touch(&a.join("shared-name-a.txt"), b"");
    touch(&b.join("shared-name-b.txt"), b"");
    let paths = Paths { config_file: t.path().join("c.toml"), data_dir: t.path().join("data") };
    let cfg = Config { roots: vec![a.to_string_lossy().into(), b.to_string_lossy().into()], ..Config::default() };
    index(&cfg, &paths);
    let mut m = Mcp::spawn(
        &["--index", paths.index_file().to_str().unwrap(), "--allow", t.path().join("a").to_str().unwrap()],
        t.path(),
    );
    let (_, v) = m.call("search_files", json!({"query": "shared-name"}));
    assert_eq!(Mcp::paths(&v), vec![a.join("shared-name-a.txt").display().to_string()]);
}

/// Every property an `outputSchema` marks as required must be present in
/// real responses (Claude Code validates structured output against it).
fn assert_required_present(schema: &Value, value: &Value, defs: &Value, at: &str) {
    let schema = match schema.get("$ref").and_then(Value::as_str) {
        Some(r) => &defs[r.trim_start_matches("#/$defs/")],
        None => schema,
    };
    if let Some(req) = schema["required"].as_array() {
        for k in req {
            assert!(value.get(k.as_str().unwrap()).is_some(), "{at}: missing required {k} in {value}");
        }
    }
    if let (Some(props), Some(obj)) = (schema["properties"].as_object(), value.as_object()) {
        for (k, v) in obj {
            if let Some(ps) = props.get(k) {
                assert_required_present(ps, v, defs, &format!("{at}.{k}"));
            }
        }
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), value.as_array()) {
        for v in arr {
            assert_required_present(items, v, defs, &format!("{at}[]"));
        }
    }
}

#[test]
fn structured_outputs_satisfy_their_declared_schemas() {
    let fx = fixture();
    let mut m = fx.mcp(&[]);
    let tools = m.request("tools/list", json!({}))["result"]["tools"].clone();
    let schema = |n: &str| tools.as_array().unwrap().iter().find(|t| t["name"] == n).unwrap()["outputSchema"].clone();
    let cases = [
        ("search_files", json!({"query": "report"})),
        ("get_file_info", json!({"path": fx.root.join("work/src/main.rs").to_str().unwrap()})),
        ("index_status", json!({})),
    ];
    for (tool, args) in cases {
        let (err, v) = m.call(tool, args);
        assert!(!err, "{tool}: {v}");
        let s = schema(tool);
        assert_required_present(&s, &v, &s["$defs"], tool);
    }
}

// ---- regressions for the GPT 6 Astra review findings (2026-10-31)

#[test]
fn symlinked_ancestor_never_reveals_outside_metadata() {
    let fx = fixture();
    let sub = fx.root.join("work/sub");
    touch(&sub.join("record.txt"), b"tiny");
    let paths = Paths { config_file: fx.t.path().join("c.toml"), data_dir: fx.t.path().join("data") };
    let cfg = Config { roots: vec![fx.root.to_string_lossy().into()], index_hidden: true, ..Config::default() };
    index(&cfg, &paths);
    // After indexing, `sub` becomes a symlink to an unauthorized directory
    // holding a same-named file with a distinctive size.
    let outside = fx.t.path().join("outside");
    touch(&outside.join("record.txt"), &[b'x'; 12345]);
    fs::remove_dir_all(&sub).unwrap();
    std::os::unix::fs::symlink(&outside, &sub).unwrap();
    let mut m = fx.mcp(&[]);
    let (_, v) = m.call("search_files", json!({"query": "record.txt"}));
    assert!(v["results"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["omitted_stale"], 1);
    let (err, v) = m.call("get_file_info", json!({"path": sub.join("record.txt").to_str().unwrap()}));
    assert!(!err, "{v}");
    assert_eq!(v["exists"], false);
    assert_eq!(v["size"], 4, "indexed metadata, never the outside file's 12345: {v}");
    assert!(!v.to_string().contains("12345"));
}

fn many_files_fixture(names: &[String]) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("home");
    for n in names {
        touch(&root.join(n), b"");
    }
    let paths = Paths { config_file: t.path().join("c.toml"), data_dir: t.path().join("data") };
    let cfg = Config { roots: vec![root.to_string_lossy().into()], index_hidden: true, ..Config::default() };
    index(&cfg, &paths);
    (t, root, paths.index_file())
}

#[test]
fn more_available_is_not_an_oracle_for_filtered_matches() {
    let names: Vec<String> = (0..60).map(|i| format!("docs/.hidden-{i:02}-needle.txt")).collect();
    let (t, root, db) = many_files_fixture(&names);
    let mut m = Mcp::spawn(&["--index", db.to_str().unwrap(), "--allow", root.to_str().unwrap()], t.path());
    let (_, v) = m.call("search_files", json!({"query": ".hidden- needle", "limit": 1}));
    assert!(v["results"].as_array().unwrap().is_empty());
    assert_eq!(v["more_available"], false, "hidden matches must be undetectable: {v}");
}

#[test]
fn filtered_candidates_do_not_starve_visible_results() {
    // 300 denied names rank above the single visible one ("needle" is the
    // whole stem of the denied files, a weaker substring of the visible).
    let mut names: Vec<String> = (0..300).map(|i| format!("keys/k{i:03}/needle.pem")).collect();
    names.push("docs/the-needle-in-a-haystack-notes.txt".into());
    let (t, root, db) = many_files_fixture(&names);
    let mut m = Mcp::spawn(&["--index", db.to_str().unwrap(), "--allow", root.to_str().unwrap()], t.path());
    let (_, v) = m.call("search_files", json!({"query": "needle", "limit": 1}));
    let p = Mcp::paths(&v);
    assert_eq!(p.len(), 1, "{v}");
    assert!(p[0].ends_with("the-needle-in-a-haystack-notes.txt"), "{p:?}");
    assert_eq!(v["more_available"], false);
}

#[test]
fn more_denied_candidates_than_any_cap_still_cannot_starve_or_hide() {
    // Re-check finding: 8200 denied, higher-ranked matches exceeded the old
    // MCP-side candidate cap. With the policy inside the index budget they
    // cost nothing.
    let mut names: Vec<String> = (0..8200).map(|i| format!("keys/k{:02}/n{i:04}/needle.pem", i % 50)).collect();
    names.push("docs/the-needle-in-a-haystack-notes.txt".into());
    names.push("docs/needle".into());
    let (t, root, db) = many_files_fixture(&names);
    let mut m = Mcp::spawn(&["--index", db.to_str().unwrap(), "--allow", root.to_str().unwrap()], t.path());
    let (_, v) = m.call("search_files", json!({"query": "needle", "limit": 1}));
    let p = Mcp::paths(&v);
    assert_eq!(p.len(), 1, "{v}");
    assert!(p[0].ends_with("docs/needle"), "exact name first: {p:?}");
    assert_eq!(v["more_available"], true, "the other authorized match exists: {v}");
    let (_, v) = m.call("search_files", json!({"query": "needle", "limit": 5}));
    assert_eq!(Mcp::paths(&v).len(), 2, "{v}");
    assert_eq!(v["more_available"], false);
}

#[test]
fn more_than_8192_deleted_matches_do_not_hide_live_matches() {
    let mut names: Vec<String> = (0..8300).map(|i| format!("gone/d{i:04}/needle.txt")).collect();
    names.extend(["live/a-long-needle-survivor.txt".into(), "live/another-needle-survivor.txt".into()]);
    let (t, root, db) = many_files_fixture(&names);
    fs::remove_dir_all(root.join("gone")).unwrap();
    let mut m = Mcp::spawn(&["--index", db.to_str().unwrap(), "--allow", root.to_str().unwrap()], t.path());
    for scope in [None, Some(root.to_str().unwrap())] {
        let mut args = json!({"query": "needle", "limit": 1});
        if let Some(scope) = scope {
            args["root"] = json!(scope);
        }
        let (err, v) = m.call("search_files", args.clone());
        assert!(!err, "{v}");
        assert_eq!(v["results"].as_array().unwrap().len(), 1, "{v}");
        assert_eq!(v["more_available"], true, "{v}");
        args["limit"] = json!(10);
        let (err, v) = m.call("search_files", args);
        assert!(!err, "{v}");
        assert_eq!(
            Mcp::paths(&v),
            vec![
                root.join("live/a-long-needle-survivor.txt").display().to_string(),
                root.join("live/another-needle-survivor.txt").display().to_string(),
            ]
        );
        assert_eq!(v["more_available"], false, "{v}");
    }
}

#[test]
fn access_modes_only_expose_indexed_policy_approved_paths() {
    let t = tempfile::Builder::new().prefix("ferro-modes-").tempdir().unwrap();
    let home = t.path().join("home");
    let outside = t.path().join("outside");
    for path in [
        home.join("one/report.txt"),
        home.join("two/report.txt"),
        outside.join("report.txt"),
        outside.join("report.pem"),
        outside.join("blocked-report.txt"),
        outside.join("node_modules/report.txt"),
        outside.join(".hidden-report.txt"),
        outside.join("credentials/report.txt"),
    ] {
        touch(&path, b"fixture");
    }
    let paths = Paths { config_file: t.path().join("engine.toml"), data_dir: t.path().join("data") };
    index(
        &Config {
            roots: vec![home.to_string_lossy().into_owned(), outside.to_string_lossy().into_owned()],
            index_hidden: true,
            ..Default::default()
        },
        &paths,
    );
    touch(&outside.join("new-report.txt"), b"not indexed");
    let policy = t.path().join("policy.toml");
    fs::write(&policy, "deny = [\"blocked*\"]\ndeny_exceptions = [\"report.pem\", \"blocked*\"]\n").unwrap();
    for (mode, expected) in [
        (
            "all_indexed",
            vec![
                home.join("one/report.txt"),
                home.join("two/report.txt"),
                outside.join("report.txt"),
                outside.join("report.pem"),
            ],
        ),
        ("home", vec![home.join("one/report.txt"), home.join("two/report.txt")]),
        ("restricted", vec![home.join("one/report.txt")]),
    ] {
        let allowed = home.join("one");
        let mut m = Mcp::spawn(
            &[
                "--index",
                paths.index_file().to_str().unwrap(),
                "--config",
                policy.to_str().unwrap(),
                "--access-mode",
                mode,
                "--allow",
                allowed.to_str().unwrap(),
            ],
            &home,
        );
        let (err, found) = m.call("search_files", json!({"query":"report", "limit":100}));
        assert!(!err, "{found}");
        let mut expected: Vec<_> = expected.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        expected.sort();
        assert_eq!(Mcp::paths(&found), expected, "{mode}");
        for path in [
            outside.join("new-report.txt"),
            outside.join("node_modules/report.txt"),
            outside.join("blocked-report.txt"),
            outside.join(".hidden-report.txt"),
            outside.join("credentials/report.txt"),
        ] {
            assert!(m.call("get_file_info", json!({"path":path})).0, "{mode}: {path:?}");
        }
        assert_eq!(m.call("index_status", json!({})).1["access_mode"], mode);
    }
    // Global mode follows new indexed roots without manual authorization edits.
    let mut m = Mcp::spawn(&["--index", paths.index_file().to_str().unwrap(), "--access-mode", "all_indexed"], &home);
    let extra = t.path().join("extra");
    touch(&extra.join("unique-global.txt"), b"x");
    touch(&extra.join("sub/linked.txt"), b"indexed");
    assert!(Mcp::paths(&m.call("search_files", json!({"query":"unique-global"})).1).is_empty());
    index(
        &Config {
            roots: vec![
                home.to_string_lossy().into_owned(),
                outside.to_string_lossy().into_owned(),
                extra.to_string_lossy().into_owned(),
            ],
            ..Default::default()
        },
        &paths,
    );
    assert_eq!(
        Mcp::paths(&m.call("search_files", json!({"query":"unique-global"})).1),
        vec![extra.join("unique-global.txt").to_string_lossy().into_owned()]
    );
    // Default secret protection still applies without exceptions.
    assert!(!Mcp::paths(&m.call("search_files", json!({"query":"report"})).1).iter().any(|p| p.ends_with(".pem")));
    // A symlink inserted after indexing cannot expose non-indexed metadata.
    let target = t.path().join("not-indexed");
    touch(&target.join("linked.txt"), &[b'x'; 12345]);
    fs::remove_dir_all(extra.join("sub")).unwrap();
    std::os::unix::fs::symlink(target, extra.join("sub")).unwrap();
    assert!(Mcp::paths(&m.call("search_files", json!({"query":"linked.txt"})).1).is_empty());
    let (err, info) = m.call("get_file_info", json!({"path":extra.join("sub/linked.txt")}));
    assert!(!err, "{info}");
    assert_eq!(info["exists"], false);
    assert_eq!(info["size"], 7);
}

#[test]
fn invalid_modes_and_home_fail_closed_at_startup() {
    let t = tempfile::tempdir().unwrap();
    for (mode, home) in [("invalid", "/tmp"), ("home", "/"), ("home", "relative"), ("home", "/tmp/../home")] {
        let out = Command::new(env!("CARGO_BIN_EXE_ferroseek-mcp"))
            .args(["--access-mode", mode])
            .env("XDG_CONFIG_HOME", t.path())
            .env("HOME", home)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{mode}, {home}");
        assert!(out.stdout.is_empty());
    }
}
