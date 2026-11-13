// SPDX-License-Identifier: GPL-3.0-or-later
// Synthetic index-only diagnostic; see README.md for scope and reproduction.
use ferroseek_index::{Kind, Meta, Query, Reader, SearchOptions, Writer};
use std::{path::Path, time::Instant};
fn main() {
    let base = std::env::args().nth(1).expect("new evidence directory");
    for n in [10_000usize, 100_000] {
        let db = Path::new(&base).join(format!("probe-{n}.sqlite"));
        assert!(!db.exists());
        let (mut writer, _) = Writer::open(&db).unwrap();
        let mut tx = writer.transaction().unwrap();
        let meta = Meta { kind: Kind::Dir, size: 0, mtime_ns: 0, ino: 1 };
        let root = tx.ensure_root(Path::new("/synthetic"), meta).unwrap();
        let mut dirs = Vec::new();
        for i in 0..100 {
            dirs.push(tx.upsert(root, format!("bucket{i:03}").as_bytes(), meta).unwrap().0);
        }
        for i in 0..n {
            tx.upsert(
                dirs[i % 100],
                format!("report-{i:06}.txt").as_bytes(),
                Meta { kind: Kind::File, ino: i as u64 + 200, ..meta },
            )
            .unwrap();
        }
        tx.commit().unwrap();
        writer.close().unwrap();
        let reader = Reader::open(&db).unwrap();
        let options = SearchOptions { limit: 15, ..Default::default() };
        let cases = [
            ("exact", "report-000000.txt", 1),
            ("short-path-miss", "zz/", 0),
            ("long-path-miss", "missing/", 0),
            ("short-tokens-miss", "re zz", 0),
        ];
        for (_, q, expected) in cases {
            assert_eq!(reader.search(&Query::parse(q), &options).unwrap().len(), expected);
        }
        for round in 0..5 {
            measure_round(&reader, n, round, &cases);
        }
    }
}

fn measure_round(reader: &Reader, n: usize, round: usize, cases: &[(&str, &str, usize)]) {
    let options = SearchOptions { limit: 15, ..Default::default() };
    for offset in 0..cases.len() {
        let (label, q, expected) = cases[(offset + round) % cases.len()];
        for sample in 0..3 {
            let query = Query::parse(q);
            let start = Instant::now();
            let hits = reader.search(&query, &options).unwrap();
            let us = start.elapsed().as_secs_f64() * 1e6;
            assert_eq!(hits.len(), expected);
            if expected == 1 {
                assert_eq!(hits[0].path, Path::new("/synthetic/bucket000/report-000000.txt"));
            }
            println!("{n},{round},{sample},{label},{us:.3},{}", hits.len());
        }
    }
}
