// SPDX-License-Identifier: GPL-3.0-or-later
//! Local text history and the opt-in capture policy.

mod wayland;

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::usage;

pub const TEXT_LIMIT: usize = 8192;
pub const ENTRY_LIMIT: usize = 100;
const PIN_LIMIT: usize = 20;
const FILE_LIMIT: u64 = 6 * TEXT_LIMIT as u64 * ENTRY_LIMIT as u64 + 16384;
const SECONDS_PER_DAY: u64 = 86400;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Off,
    Recording,
    Paused,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Retention {
    Day,
    Week,
    Month,
}

impl Retention {
    pub fn days(self) -> u64 {
        match self {
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Day => Self::Week,
            Self::Week => Self::Month,
            Self::Month => Self::Day,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Policy {
    pub mode: Mode,
    pub retention: Retention,
    revision: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self { mode: Mode::Off, retention: Retention::Week, revision: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub text: String,
    pub copied_at: u64,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct History {
    next_id: u64,
    entries: Vec<Entry>,
}

impl History {
    fn expire(&mut self, retention: Retention, now: u64) -> bool {
        let keep_after = now.saturating_sub(retention.days() * SECONDS_PER_DAY);
        let previous = self.entries.len();
        self.entries.retain(|entry| accepts(&entry.text) && (entry.pinned || entry.copied_at >= keep_after));
        self.entries.truncate(ENTRY_LIMIT);
        previous != self.entries.len()
    }

    fn record(&mut self, text: String, retention: Retention, now: u64) {
        self.expire(retention, now);
        let entry = if let Some(index) = self.entries.iter().position(|entry| entry.text == text) {
            Entry { copied_at: now, ..self.entries.remove(index) }
        } else {
            self.next_id = self.next_id.saturating_add(1);
            Entry { id: self.next_id, text, copied_at: now, pinned: false }
        };
        self.entries.insert(0, entry);
        self.entries.sort_by_key(|entry| !entry.pinned);
        self.entries.truncate(ENTRY_LIMIT);
    }

    fn set_pinned(&mut self, id: u64, pinned: bool) -> io::Result<()> {
        let already_pinned = self.entries.iter().filter(|entry| entry.pinned).count();
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| io::Error::other("This clipboard entry was removed."))?;
        if entry.pinned == pinned {
            return Ok(());
        }
        if pinned && already_pinned >= PIN_LIMIT {
            return Err(io::Error::other(format!("You can pin up to {PIN_LIMIT} entries. Unpin one to make room.")));
        }
        entry.pinned = pinned;
        self.entries.sort_by_key(|entry| (!entry.pinned, std::cmp::Reverse(entry.copied_at)));
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Edit {
    Enable,
    Stop,
    Retain(Retention),
    Delete(u64),
    Pin { id: u64, pinned: bool },
    Clear,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorder {
    Idle,
    Starting,
    Ready,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub policy: Policy,
    pub entries: Vec<Entry>,
    pub recorder: Recorder,
}

#[derive(Debug, Clone)]
pub struct Profile(PathBuf);

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    data: Option<(u64, std::time::SystemTime)>,
    policy: Option<(u64, std::time::SystemTime)>,
    recorder: Recorder,
}

fn file_stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    Some((metadata.ino(), metadata.modified().ok()?))
}

pub fn launcher_started(private: bool) -> io::Result<()> {
    let Some(profile) = Profile::from_env() else { return Ok(()) };
    if !profile.policy_path().exists() {
        return Ok(());
    }
    if private {
        profile.edit(Edit::Private)
    } else {
        std::thread::spawn(move || {
            let _ = profile.ensure_running();
        });
        Ok(())
    }
}

impl Profile {
    pub fn from_env() -> Option<Self> {
        usage::Usage::default_path().map(|path| Self(path.with_file_name("launcher-clipboard")))
    }

    fn policy_path(&self) -> PathBuf {
        self.0.with_extension("policy")
    }

    pub fn stamp(&self) -> Stamp {
        Stamp {
            data: file_stamp(&self.0),
            policy: file_stamp(&self.policy_path()),
            recorder: self.recorder().unwrap_or(Recorder::Idle),
        }
    }

    pub fn policy(&self) -> io::Result<Policy> {
        read_private(&self.policy_path(), 4096)
    }

    pub fn snapshot(&self) -> io::Result<Snapshot> {
        if !self.0.exists() {
            return Ok(Snapshot { policy: self.policy()?, entries: Vec::new(), recorder: self.recorder()? });
        }
        let (policy, history) = usage::with_lock(&self.0, true, || {
            let policy = self.policy()?;
            let mut history: History = read_private(&self.0, FILE_LIMIT)?;
            if history.expire(policy.retention, usage::now()) {
                write_atomic(&self.0, &history)?;
            }
            Ok((policy, history))
        })?;
        Ok(Snapshot { policy, entries: history.entries, recorder: self.recorder()? })
    }

    pub fn edit(&self, edit: Edit) -> io::Result<()> {
        if matches!(edit, Edit::Enable) {
            wayland::probe()?;
        }
        usage::with_lock(&self.0, true, || {
            let mut policy = self.policy()?;
            let delete = match edit {
                Edit::Enable => {
                    policy.mode = Mode::Recording;
                    None
                }
                Edit::Stop => {
                    policy.mode = Mode::Off;
                    None
                }
                Edit::Private if policy.mode == Mode::Recording => {
                    policy.mode = Mode::Paused;
                    None
                }
                Edit::Private => return Ok(()),
                Edit::Retain(retention) => {
                    policy.retention = retention;
                    Some(None)
                }
                Edit::Pin { id, pinned } => return self.set_pinned(id, pinned, policy.retention),
                Edit::Delete(id) => Some(Some(id)),
                Edit::Clear => Some(Some(0)),
            };
            // A transfer begun before a policy edit can't restore deleted text.
            policy.revision = policy.revision.wrapping_add(1);
            write_atomic(&self.policy_path(), &policy)?;
            if let Some(delete) = delete {
                self.delete_expired_and_selected(policy.retention, delete)?;
            }
            Ok(())
        })
    }

    fn set_pinned(&self, id: u64, pinned: bool, retention: Retention) -> io::Result<()> {
        let mut history: History = read_private(&self.0, FILE_LIMIT)?;
        let now = usage::now();
        history.expire(retention, now);
        history.set_pinned(id, pinned)?;
        history.expire(retention, now);
        write_atomic(&self.0, &history)
    }

    fn delete_expired_and_selected(&self, retention: Retention, delete: Option<u64>) -> io::Result<()> {
        let mut history: History = read_private(&self.0, FILE_LIMIT)?;
        history.expire(retention, usage::now());
        match delete {
            Some(0) => history.entries.clear(),
            Some(id) => history.entries.retain(|entry| entry.id != id),
            None => {}
        }
        write_atomic(&self.0, &history)
    }

    fn record(&self, expected: Policy, text: String) -> io::Result<()> {
        if !accepts(&text) {
            return Ok(());
        }
        usage::with_lock(&self.0, true, || {
            let policy = self.policy()?;
            if policy.mode != Mode::Recording || policy.revision != expected.revision {
                return Ok(());
            }
            let mut history: History = read_private(&self.0, FILE_LIMIT)?;
            history.record(text, policy.retention, usage::now());
            write_atomic(&self.0, &history)
        })
    }

    fn watcher_lock(&self, create: bool) -> io::Result<Option<File>> {
        let lock = usage::private_options()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(self.0.with_extension("watcher"));
        match lock {
            Ok(file) if file.metadata()?.is_file() => Ok(Some(file)),
            Ok(_) => Err(io::Error::other("Clipboard recorder lock must be a regular file.")),
            Err(error) if error.kind() == io::ErrorKind::NotFound && !create => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn recorder(&self) -> io::Result<Recorder> {
        let Some(file) = self.watcher_lock(false)? else {
            return Ok(Recorder::Idle);
        };
        match file.try_lock() {
            Ok(()) => Ok(Recorder::Idle),
            Err(TryLockError::WouldBlock) => {
                let mut marker = [0; 8];
                let complete = file.read_at(&mut marker, 0)? == marker.len();
                let current = complete && u64::from_le_bytes(marker) == self.policy()?.revision;
                Ok(if current { Recorder::Ready } else { Recorder::Starting })
            }
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    pub fn ensure_running(&self) -> io::Result<()> {
        use std::process::{Command, Stdio};
        if self.policy()?.mode != Mode::Recording || self.recorder()? != Recorder::Idle {
            return Ok(());
        }
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--watch-clipboard")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    pub fn watch(&self) -> io::Result<()> {
        if self.policy()?.mode != Mode::Recording {
            return Ok(());
        }
        let Some(lock) = self.watcher_lock(true)? else {
            return Err(io::Error::other("Cannot start the clipboard recorder."));
        };
        match lock.try_lock() {
            Ok(()) => {
                loop {
                    lock.set_len(0)?;
                    if !wayland::watch(self, &lock)? {
                        break;
                    }
                }
                Ok(())
            }
            Err(TryLockError::WouldBlock) => Ok(()),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }
}

fn accepts(text: &str) -> bool {
    text.len() <= TEXT_LIMIT && !text.trim().is_empty() && !text.contains('\0')
}

fn read_private<T: DeserializeOwned + Default>(path: &Path, limit: u64) -> io::Result<T> {
    let file = match OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Clipboard history needs regular files."));
    }
    let mut data = Vec::new();
    file.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(io::Error::other("Clipboard history is too large."));
    }
    serde_json::from_slice(&data).map_err(|_| io::Error::other("Cannot read clipboard history."))
}

fn write_atomic(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::other("Clipboard history needs a state directory."))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(profile: &Profile) -> Policy {
        let policy = Policy { mode: Mode::Recording, ..Policy::default() };
        write_atomic(&profile.policy_path(), &policy).unwrap();
        policy
    }

    #[test]
    fn copies_deduplicate_and_deletion_rejects_in_flight_transfers() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile(dir.path().join("history"));
        let initial = recording(&profile);
        let payload = "First line\nÉquipe";
        profile.record(initial, payload.into()).unwrap();
        profile.record(initial, "Other".into()).unwrap();
        profile.record(initial, payload.into()).unwrap();
        let data = profile.snapshot().unwrap();
        assert_eq!(data.entries.iter().map(|entry| entry.text.as_str()).collect::<Vec<_>>(), [payload, "Other"]);
        profile.edit(Edit::Delete(data.entries[0].id)).unwrap();
        profile.record(initial, payload.into()).unwrap();
        let entries = profile.snapshot().unwrap().entries;
        assert_eq!(entries.len(), 1);
        let remaining = entries[0].id;
        profile.edit(Edit::Pin { id: remaining, pinned: true }).unwrap();
        profile.edit(Edit::Clear).unwrap();
        assert!(profile.snapshot().unwrap().entries.is_empty());
    }

    #[test]
    fn legacy_entries_can_be_pinned_and_keep_their_id_when_copied_again() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile(dir.path().join("history"));
        let initial = recording(&profile);
        let now = usage::now();
        let legacy = serde_json::json!({
            "next_id": 1,
            "entries": [{"id": 1, "text": "Keep", "copied_at": now}],
        });
        write_atomic(&profile.0, &legacy).unwrap();
        assert!(!profile.snapshot().unwrap().entries[0].pinned);
        profile.edit(Edit::Pin { id: 1, pinned: true }).unwrap();
        profile.record(initial, "Recent".into()).unwrap();
        profile.record(initial, "Keep".into()).unwrap();
        let entries = profile.snapshot().unwrap().entries;
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].id, entries[0].text.as_str(), entries[0].pinned), (1, "Keep", true));

        let mut aged: History = read_private(&profile.0, FILE_LIMIT).unwrap();
        aged.entries[0].copied_at = now.saturating_sub(2 * SECONDS_PER_DAY);
        write_atomic(&profile.0, &aged).unwrap();
        profile.edit(Edit::Retain(Retention::Day)).unwrap();
        assert_eq!(profile.snapshot().unwrap().entries.len(), 2);
        profile.edit(Edit::Pin { id: 1, pinned: false }).unwrap();
        let entries = profile.snapshot().unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "Recent");
    }

    #[test]
    fn history_capacity_preserves_pins_and_room_for_recent_copies() {
        let mut history = History::default();
        for i in 1..=PIN_LIMIT as u64 {
            history.record(i.to_string(), Retention::Day, i);
            history.set_pinned(i, true).unwrap();
        }
        for i in (PIN_LIMIT + 1)..=(PIN_LIMIT + ENTRY_LIMIT) {
            history.record(i.to_string(), Retention::Day, i as u64);
        }
        assert_eq!(history.entries.len(), ENTRY_LIMIT);
        assert!(history.entries[..PIN_LIMIT].iter().all(|entry| entry.pinned));
        assert!(!history.entries[PIN_LIMIT].pinned);
        let recent = history.entries[PIN_LIMIT].id;
        assert_eq!(recent, (PIN_LIMIT + ENTRY_LIMIT) as u64);
        assert!(history.set_pinned(recent, true).is_err());
        history.set_pinned(1, false).unwrap();
        history.set_pinned(recent, true).unwrap();
        assert!(history.entries[0].pinned && history.entries[0].id == recent);
    }

    #[test]
    fn retention_and_private_mode_leave_no_path_for_new_unapproved_entries() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile(dir.path().join("history"));
        let initial = recording(&profile);
        let mut old = History::default();
        old.record("Expired".into(), Retention::Week, usage::now().saturating_sub(2 * SECONDS_PER_DAY));
        write_atomic(&profile.0, &old).unwrap();
        profile.edit(Edit::Retain(Retention::Day)).unwrap();
        assert!(profile.snapshot().unwrap().entries.is_empty());
        let resumed = profile.policy().unwrap();
        profile.edit(Edit::Private).unwrap();
        profile.record(resumed, "Private".into()).unwrap();
        assert_eq!(profile.policy().unwrap().mode, Mode::Paused);
        assert!(profile.snapshot().unwrap().entries.is_empty());
        profile.record(initial, "Stale".into()).unwrap();
        assert!(profile.snapshot().unwrap().entries.is_empty());
    }
}
