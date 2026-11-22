// SPDX-License-Identifier: GPL-3.0-or-later
//! Messages on the private stdio of the Plasma window helper.

use std::io::{self, BufRead, Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{Action, Entry};

pub const HELPER: &str = "ferroseek-plasma-windows";
pub const REQUEST_LIMIT: u64 = 4096;
pub const RESPONSE_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
pub struct Request {
    pub request: u64,
    pub window: u64,
    pub action: Action,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum Response<T = Vec<Entry>> {
    Windows { entries: T, complete: bool },
    Unavailable(String),
    Finished { request: u64, error: Option<String> },
}

pub fn receive<T: DeserializeOwned>(input: &mut impl BufRead, line: &mut Vec<u8>, limit: u64) -> io::Result<Option<T>> {
    line.clear();
    if input.take(limit).read_until(b'\n', line)? == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "incomplete helper message"));
    }
    Ok(Some(serde_json::from_slice(line)?))
}

pub fn send(output: &mut impl Write, message: &impl Serialize, limit: u64) -> io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    if line.len() as u64 > limit {
        return Err(frame_too_large());
    }
    output.write_all(&line)?;
    output.flush()
}

pub fn send_windows(output: &mut impl Write, entries: &[Entry], limit: u64) -> io::Result<()> {
    let overhead = |complete| {
        serde_json::to_vec(&Response::Windows { entries: &entries[..0], complete })
            .map(|encoded| encoded.len() as u64 + 1)
    };
    let continuation_overhead = overhead(false)?;
    let final_overhead = overhead(true)?;
    let mut start = 0;
    let mut bytes = 0;
    for (index, entry) in entries.iter().enumerate() {
        let frame_overhead = if index + 1 == entries.len() { final_overhead } else { continuation_overhead };
        let size = serde_json::to_vec(entry)?.len() as u64;
        if frame_overhead + size > limit {
            return Err(frame_too_large());
        }
        let joined = bytes + size + u64::from(index > start);
        if frame_overhead + joined > limit {
            send(output, &Response::Windows { entries: &entries[start..index], complete: false }, limit)?;
            start = index;
            bytes = size;
        } else {
            bytes = joined;
        }
    }
    send(output, &Response::Windows { entries: &entries[start..], complete: true }, limit)
}

fn frame_too_large() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "helper message exceeds the frame size limit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_window_catalogs_split_at_encoded_frame_boundaries() {
        let title = "\0".repeat(20);
        let windows =
            [Entry { id: 1, title: title.clone(), ..Entry::default() }, Entry { id: 2, title, ..Entry::default() }];
        let first = Response::Windows { entries: &windows[..1], complete: false };
        let limit = serde_json::to_vec(&first).unwrap().len() as u64 + 1;
        let mut output = Vec::new();
        send_windows(&mut output, &windows, limit).unwrap();
        let mut input = output.as_slice();
        let mut line = Vec::new();
        for (index, window) in windows.into_iter().enumerate() {
            let frame: Response = receive(&mut input, &mut line, limit).unwrap().unwrap();
            match frame {
                Response::Windows { entries, complete } => {
                    assert_eq!(entries, [window]);
                    assert_eq!(complete, index == 1);
                }
                _ => panic!("expected a window frame"),
            }
        }
        assert!(receive::<Response>(&mut input, &mut line, limit).unwrap().is_none());
    }

    #[test]
    fn an_exact_size_final_window_record_fits() {
        let windows = [Entry { id: 1, title: "Report".into(), ..Entry::default() }];
        let frame = Response::Windows { entries: &windows, complete: true };
        let limit = serde_json::to_vec(&frame).unwrap().len() as u64 + 1;
        let mut output = Vec::new();
        send_windows(&mut output, &windows, limit).unwrap();
        assert_eq!(output.len() as u64, limit);
        let decoded: Response = receive(&mut output.as_slice(), &mut Vec::new(), limit).unwrap().unwrap();
        assert!(matches!(decoded, Response::Windows { entries, complete: true } if entries == windows));
    }

    #[test]
    fn a_record_that_cannot_fit_never_writes_an_oversized_frame() {
        let window = Entry { title: "\0".repeat(100), ..Entry::default() };
        let mut output = Vec::new();
        assert_eq!(send_windows(&mut output, &[window], 256).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(output.is_empty());
    }
}
