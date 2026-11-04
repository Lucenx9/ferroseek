// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in local diagnostics. Records counts and durations, never queries or paths.

use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static TRACE: OnceLock<Mutex<Trace<io::Stderr>>> = OnceLock::new();

pub fn init() {
    if std::env::var_os("FERROSEEK_LAUNCHER_TIMINGS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        let mut trace = Trace::new(Instant::now(), io::stderr());
        trace.record("process_start", trace.start, 0, 0, 0, trace.start);
        let _ = TRACE.set(Mutex::new(trace));
    }
}

pub fn needs_frame() -> bool {
    TRACE.get().and_then(|trace| trace.lock().ok()).is_some_and(|trace| {
        trace.first_frame
            || trace.appearance.is_some()
            || trace.query.as_ref().is_some_and(|query| query.ready.is_some())
    })
}

pub fn enabled() -> bool {
    TRACE.get().is_some()
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

pub fn ready(generation: u64, rows: usize, files: usize, success: bool) {
    with_trace(|trace| trace.ready(generation, rows, files, success, Instant::now()));
}

pub fn frame(at: Instant, generation: u64, visible: bool) {
    with_trace(|trace| trace.frame(at, generation, visible));
}

pub fn appearance_changed(visible: bool) {
    with_trace(|trace| trace.appearance = visible.then(Instant::now));
}
pub fn appearance_frame(at: Instant) {
    with_trace(|trace| {
        if trace.appearance.is_some_and(|changed| at >= changed) {
            trace.appearance = None;
            trace.record("appearance_frame", at, 0, 0, 0, at);
        }
    });
}

pub fn event(event: &'static str) {
    event_at(event, Instant::now());
}

pub fn event_at(event: &'static str, at: Instant) {
    with_trace(|trace| {
        trace.record(event, at, 0, 0, 0, at);
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Wgpu,
    TinySkia,
}

pub fn renderer(backend: Backend) {
    with_trace(|trace| trace.renderer(backend, Instant::now()));
}

pub fn surface_resized(size: (u32, u32)) {
    with_trace(|trace| trace.surface_resized(size, Instant::now()));
}

struct Query {
    generation: u64,
    start: Instant,
    ready: Option<Instant>,
    rows: usize,
    files: usize,
}

struct Trace<W> {
    start: Instant,
    output: W,
    first_frame: bool,
    appearance: Option<Instant>,
    renderer: Option<Backend>,
    query: Option<Query>,
}

impl<W: Write> Trace<W> {
    fn new(start: Instant, output: W) -> Self {
        Self { start, output, first_frame: true, appearance: None, renderer: None, query: None }
    }

    fn renderer(&mut self, backend: Backend, at: Instant) {
        if self.renderer.replace(backend) != Some(backend) {
            let event = match backend {
                Backend::Wgpu => "renderer_wgpu",
                Backend::TinySkia => "renderer_tiny_skia",
            };
            self.record(event, at, 0, 0, 0, at);
        }
    }

    fn surface_resized(&mut self, (width, height): (u32, u32), at: Instant) {
        let elapsed_us = at.saturating_duration_since(self.start).as_micros();
        let _ = writeln!(
            self.output,
            "{{\"event\":\"surface_resized\",\"elapsed_us\":{elapsed_us},\"generation\":0,\"rows\":0,\"files\":0,\"query_us\":0,\"width\":{width},\"height\":{height}}}"
        );
    }

    fn record(&mut self, event: &str, at: Instant, generation: u64, rows: usize, files: usize, query_start: Instant) {
        let elapsed_us = at.saturating_duration_since(self.start).as_micros();
        let query_us = at.saturating_duration_since(query_start).as_micros();
        let _ = writeln!(
            self.output,
            "{{\"event\":\"{event}\",\"elapsed_us\":{elapsed_us},\"generation\":{generation},\"rows\":{rows},\"files\":{files},\"query_us\":{query_us}}}"
        );
    }

    fn begin(&mut self, generation: u64, at: Instant) {
        if let Some(query) = self.query.take() {
            self.record("query_superseded", at, query.generation, query.rows, query.files, query.start);
        }
        self.query = Some(Query { generation, start: at, ready: None, rows: 0, files: 0 });
        self.record("query_start", at, generation, 0, 0, at);
    }

    fn ready(&mut self, generation: u64, rows: usize, files: usize, success: bool, at: Instant) {
        let Some(query) = self.query.as_mut().filter(|query| query.generation == generation) else { return };
        query.ready = Some(at);
        query.rows = rows;
        query.files = files;
        let start = query.start;
        self.record(if success { "query_ready" } else { "query_error" }, at, generation, rows, files, start);
        if !success {
            self.query = None;
        }
    }

    fn frame(&mut self, at: Instant, generation: u64, visible: bool) {
        if self.first_frame {
            self.first_frame = false;
            self.record("first_frame", at, 0, 0, 0, self.start);
        }
        if self.query.as_ref().is_some_and(|query| query.generation != generation || !visible) {
            let query = self.query.as_ref().unwrap();
            self.record(
                if query.generation != generation { "query_superseded" } else { "query_hidden" },
                at,
                query.generation,
                query.rows,
                query.files,
                query.start,
            );
            self.query = None;
        }
        // A redraw event can have been queued before results were applied.
        if self.query.as_ref().is_some_and(|query| query.ready.is_some_and(|ready| ready <= at)) {
            let query = self.query.as_ref().unwrap();
            self.record("query_frame", at, query.generation, query.rows, query.files, query.start);
            self.query = None;
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
        trace.frame(at(1), 0, true);
        trace.renderer(Backend::Wgpu, at(1));
        trace.renderer(Backend::Wgpu, at(2));
        trace.begin(1, at(2));
        trace.ready(1, 9, 8, true, at(3));
        trace.begin(2, at(4));
        trace.ready(1, 99, 99, true, at(5)); // late success must not complete query 2
        trace.ready(1, 0, 0, false, at(6)); // nor may a late error cancel it
        trace.ready(2, 4, 3, true, at(8));
        trace.frame(at(7), 2, true); // queued before current results
        trace.frame(at(10), 2, true);
        trace.frame(at(11), 2, true); // no duplicate completion
        trace.begin(3, at(12));
        trace.ready(3, 0, 0, false, at(13));
        trace.frame(at(14), 3, true);
        trace.begin(4, at(15));
        trace.ready(4, 2, 2, true, at(16));
        trace.frame(at(17), 5, true); // navigation or refresh replaced the query
        trace.begin(6, at(18));
        trace.ready(6, 2, 2, true, at(19));
        trace.frame(at(20), 6, false); // a modal covers the results
        trace.renderer(Backend::TinySkia, at(21));
        trace.renderer(Backend::TinySkia, at(22));
        trace.surface_resized((980, 500), at(23));
        let output = String::from_utf8(trace.output).unwrap();
        assert!(output.contains("\"surface_resized\",\"elapsed_us\":23000"));
        assert!(output.contains("\"width\":980,\"height\":500"));
        assert_eq!(output.matches("\"renderer_wgpu\"").count(), 1);
        assert_eq!(output.matches("\"renderer_tiny_skia\"").count(), 1);
        assert_eq!(output.matches("\"first_frame\"").count(), 1);
        assert_eq!(output.matches("\"query_frame\"").count(), 1);
        assert!(output.contains(
            "\"query_frame\",\"elapsed_us\":10000,\"generation\":2,\"rows\":4,\"files\":3,\"query_us\":6000"
        ));
        assert!(output.contains("\"query_superseded\",\"elapsed_us\":4000,\"generation\":1"));
        assert!(output.contains("\"query_error\",\"elapsed_us\":13000,\"generation\":3"));
        assert!(output.contains("\"query_superseded\",\"elapsed_us\":17000,\"generation\":4"));
        assert!(output.contains("\"query_hidden\",\"elapsed_us\":20000,\"generation\":6"));
        assert!(!output.contains("99"));
    }
}
