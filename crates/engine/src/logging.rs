// SPDX-License-Identifier: GPL-3.0-or-later
//! A minimal stderr logger. Under systemd (`JOURNAL_STREAM` set) each line
//! carries a `<N>` syslog priority prefix that journald parses; otherwise
//! plain `level: message`. Level from `FERROSEEK_LOG` (error, warn, info,
//! debug, trace); default `info`.

use std::io::Write;

struct Logger {
    journal: bool,
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= log::max_level()
    }

    fn log(&self, r: &log::Record<'_>) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let mut e = std::io::stderr().lock();
        let _ = if self.journal {
            let prio = match r.level() {
                log::Level::Error => 3,
                log::Level::Warn => 4,
                log::Level::Info => 6,
                _ => 7,
            };
            writeln!(e, "<{prio}>{}", r.args())
        } else {
            writeln!(e, "{}: {}", r.level().as_str().to_lowercase(), r.args())
        };
    }

    fn flush(&self) {}
}

pub fn init() {
    let level = std::env::var("FERROSEEK_LOG")
        .ok()
        .and_then(|v| v.parse::<log::LevelFilter>().ok())
        .unwrap_or(log::LevelFilter::Info);
    let journal = std::env::var_os("JOURNAL_STREAM").is_some();
    if log::set_boxed_logger(Box::new(Logger { journal })).is_ok() {
        log::set_max_level(level);
    }
}
