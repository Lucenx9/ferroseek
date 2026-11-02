// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in local diagnostics. Records counts and durations, never queries or paths.

use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static TRACE: OnceLock<Mutex<Trace<io::Stderr>>> = OnceLock::new();

pub fn init() {
    if std::env::var_os("FERROSEEK_LAUNCHER_TIMINGS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        let mut trace = Trace::new(Instant::now(), io::stderr());
        trace.record("process_start", trace.start, 0, 0, trace.start);
        let _ = TRACE.set(Mutex::new(trace));
    }
}

pub fn needs_frame() -> bool {
    TRACE
        .get()
        .and_then(|trace| trace.lock().ok())
        .is_some_and(|trace| trace.first_frame || trace.query.as_ref().is_some_and(|query| query.ready.is_some()))
}

fn with_trace(f: impl FnOnce(&mut Trace<io::Stderr>)) {
    if let Some(trace) = TRACE.get()
        && let Ok(mut trace) = trace.lock()
    {
        f(&mut trace);
    }
}

pub fn begin(generation: u64) {
    with_trace(|trace| trace.begin(generation, Instant::now()));
}

pub fn ready(generation: u64, rows: usize, success: bool) {
    with_trace(|trace| trace.ready(generation, rows, success, Instant::now()));
}

pub fn frame(at: Instant) {
    with_trace(|trace| trace.frame(at));
}

pub fn action_input() {
    with_trace(|trace| {
        let at = Instant::now();
        trace.record("action_input", at, 0, 0, at);
    });
}

struct Query {
    generation: u64,
    start: Instant,
    ready: Option<Instant>,
    rows: usize,
}

struct Trace<W> {
    start: Instant,
    output: W,
    first_frame: bool,
    query: Option<Query>,
}

impl<W: Write> Trace<W> {
    fn new(start: Instant, output: W) -> Self {
        Self { start, output, first_frame: true, query: None }
    }

    fn record(&mut self, event: &str, at: Instant, generation: u64, rows: usize, query_start: Instant) {
        let elapsed_us = at.saturating_duration_since(self.start).as_micros();
        let query_us = at.saturating_duration_since(query_start).as_micros();
        let _ = writeln!(
            self.output,
            "{{\"event\":\"{event}\",\"elapsed_us\":{elapsed_us},\"generation\":{generation},\"rows\":{rows},\"query_us\":{query_us}}}"
        );
    }

    fn begin(&mut self, generation: u64, at: Instant) {
        if let Some(query) = self.query.take() {
            self.record("query_superseded", at, query.generation, query.rows, query.start);
        }
        self.query = Some(Query { generation, start: at, ready: None, rows: 0 });
        self.record("query_start", at, generation, 0, at);
    }

    fn ready(&mut self, generation: u64, rows: usize, success: bool, at: Instant) {
        let Some(query) = self.query.as_mut().filter(|query| query.generation == generation) else { return };
        query.ready = Some(at);
        query.rows = rows;
        let start = query.start;
        self.record(if success { "query_ready" } else { "query_error" }, at, generation, rows, start);
        if !success {
            self.query = None;
        }
    }

    fn frame(&mut self, at: Instant) {
        if self.first_frame {
            self.first_frame = false;
            self.record("first_frame", at, 0, 0, self.start);
        }
        // A redraw event can have been queued before results were applied.
        if self.query.as_ref().is_some_and(|query| query.ready.is_some_and(|ready| ready <= at)) {
            let query = self.query.take().unwrap();
            self.record("query_frame", at, query.generation, query.rows, query.start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn only_current_successful_results_reach_a_later_frame() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut trace = Trace::new(start, Vec::new());
        trace.frame(at(1));
        trace.begin(1, at(2));
        trace.ready(1, 9, true, at(3));
        trace.begin(2, at(4));
        trace.ready(1, 99, true, at(5)); // late success must not complete query 2
        trace.ready(1, 0, false, at(6)); // nor may a late error cancel it
        trace.ready(2, 4, true, at(8));
        trace.frame(at(7)); // queued before current results
        trace.frame(at(10));
        trace.frame(at(11)); // no duplicate completion
        trace.begin(3, at(12));
        trace.ready(3, 0, false, at(13));
        trace.frame(at(14));
        let output = String::from_utf8(trace.output).unwrap();
        assert_eq!(output.matches("\"first_frame\"").count(), 1);
        assert_eq!(output.matches("\"query_frame\"").count(), 1);
        assert!(output.contains("\"query_frame\",\"elapsed_us\":10000,\"generation\":2,\"rows\":4,\"query_us\":6000"));
        assert!(output.contains("\"query_superseded\",\"elapsed_us\":4000,\"generation\":1"));
        assert!(output.contains("\"query_error\",\"elapsed_us\":13000,\"generation\":3"));
        assert!(!output.contains("99"));
    }
}
