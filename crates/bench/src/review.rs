// SPDX-License-Identifier: GPL-3.0-or-later
//! Warm query measurements with explicit expected results and repeated rounds.

use super::*;

fn thread_cpu() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: writable timespec, valid clock for the calling thread.
    assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) }, 0);
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
}

pub(super) fn run(d: &Daemon, work: &Path, db: &Path, say: &mut dyn FnMut(String)) {
    let dir = work.join("corpus/ferroseek-review-target");
    fs::create_dir(&dir).unwrap();
    let file = dir.join("report-review-sentinel.txt");
    fs::write(&file, b"benchmark sentinel").unwrap();
    let url = file_url(&file);
    wait_until(Duration::from_secs(30), || d.krunner_match("report-review-sentinel").contains(&url))
        .expect("review fixture indexed");
    let reader = Reader::open(db).unwrap();
    let ordinary = SearchOptions { limit: 15, ..Default::default() };
    let within = SearchOptions { within: Some(dir.clone()), ..ordinary.clone() };
    // Three measured requests per round; five rounds. Report small sample
    // tails as maxima, not a claim about a stable population p99.
    let cases = [
        ("exact-name", "report-review-sentinel.txt", &ordinary, true),
        ("path-only", "ferroseek-review-target/", &ordinary, true),
        ("path+name", "ferroseek-review-target/ report", &ordinary, true),
        ("within+name", "report", &within, true),
        ("path-miss", "ferroseek-review-absent/", &ordinary, false),
        ("slash-only", "//", &ordinary, false),
    ];
    say("\n#### Review query cases: five rounds × three requests, warm cache\n".into());
    say("| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |\n|---|---:|---:|---:|---:|".into());
    for (label, q, opts, present) in cases {
        let parsed = Query::parse(q);
        let check_lib = |hits: &[ferroseek_index::Hit]| {
            if present {
                assert_eq!(hits.len(), 1, "{label}: expected one hit");
                assert_eq!(hits[0].path, file, "{label}: wrong file");
            } else {
                assert!(hits.is_empty(), "{label}: unexpected hit");
            }
        };
        check_lib(&reader.search(&parsed, opts).unwrap());
        let mut rounds = Vec::new();
        let mut all = Vec::new();
        let mut cpu = 0.0;
        for _ in 0..5 {
            let mut samples = Vec::new();
            for _ in 0..3 {
                let c0 = thread_cpu();
                let t = Instant::now();
                let hits = reader.search(&parsed, opts).unwrap();
                let ms = t.elapsed().as_secs_f64() * 1e3;
                cpu += thread_cpu() - c0;
                check_lib(&hits);
                samples.push(ms);
                all.push(ms);
            }
            rounds.push(pct(&samples, 0.5));
        }
        say(format!(
            "| {label} / library | {:.3} ({:.3}–{:.3}) | {:.3} | {:.2} | 15/15 |",
            pct(&rounds, 0.5),
            pct(&rounds, 0.0),
            pct(&rounds, 1.0),
            pct(&all, 1.0),
            cpu * 1e3 / all.iter().sum::<f64>()
        ));
        if opts.within.is_some() {
            continue; // KRunner Match has no --in argument.
        }
        let check_bus = |hits: &[String]| {
            if present {
                assert_eq!(hits, std::slice::from_ref(&url), "{label}: D-Bus wrong result");
            } else {
                assert!(hits.is_empty(), "{label}: D-Bus unexpected hit");
            }
        };
        check_bus(&d.krunner_match(q));
        rounds.clear();
        all.clear();
        for _ in 0..5 {
            let mut samples = Vec::new();
            for _ in 0..3 {
                let t = Instant::now();
                let hits = d.krunner_match(q);
                let ms = t.elapsed().as_secs_f64() * 1e3;
                check_bus(&hits);
                samples.push(ms);
                all.push(ms);
            }
            rounds.push(pct(&samples, 0.5));
        }
        say(format!(
            "| {label} / D-Bus | {:.3} ({:.3}–{:.3}) | {:.3} | — | 15/15 |",
            pct(&rounds, 0.5),
            pct(&rounds, 0.0),
            pct(&rounds, 1.0),
            pct(&all, 1.0)
        ));
    }
    fs::remove_dir_all(&dir).unwrap();
    wait_until(Duration::from_secs(30), || d.krunner_match("report-review-sentinel").is_empty())
        .expect("review fixture removed");
    say("\nReview query errors/mismatches: 0 (165 measured responses checked; warmups also checked).\n".into());
    say("| metric | value |\n|---|---|".into());
}
