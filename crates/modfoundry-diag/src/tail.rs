//! Incremental log tailing, so a frontend can show a running server's log while
//! the blocking `diag.run` capture is still in its window.

use super::parse::{parse_log, LogRecord};
use crate::run::read_tail;
use std::collections::HashMap;
use std::fs::Metadata;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many bytes before a file's old offset are kept to tell an append from a rewrite.
const KEPT_BYTES: u64 = 256;

/// One parsed record plus the log file it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailedRecord {
    pub file: PathBuf,
    pub record: LogRecord,
}

/// Reads only what the log files under a directory gained since the last poll.
///
/// Known limit: an exception block split across two polls parses as two
/// records; callers replace the live list with the run's final mapped errors.
#[derive(Debug)]
pub struct LogTail {
    dir: PathBuf,
    files: HashMap<PathBuf, Seen>,
}

/// What one log file looked like at the last look.
#[derive(Debug)]
struct Seen {
    offset: u64,
    len: u64,
    modified: Option<SystemTime>,
    /// The last up to [`KEPT_BYTES`] bytes before `offset`, or none when they could not be read.
    tail: Option<Vec<u8>>,
    partial: Vec<u8>,
}

impl LogTail {
    /// Tail the log files under `dir`, which need not exist yet.
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            files: HashMap::new(),
        }
    }

    /// Like [`LogTail::new`], but the log files already under `dir` are skipped;
    /// only what they gain from now on is read.
    pub fn after_existing(dir: &Path) -> Self {
        Self::skipping(dir, |_| true)
    }

    /// Like [`LogTail::new`], but log files last modified before `since_millis`
    /// are skipped.
    pub fn since(dir: &Path, since_millis: i64) -> Self {
        Self::skipping(dir, |meta| modified_before(meta, since_millis))
    }

    /// Starts each log file that `skip` accepts at its current length.
    fn skipping(dir: &Path, skip: impl Fn(&Metadata) -> bool) -> Self {
        let mut files = HashMap::new();
        for file in crate::find_logs(dir) {
            let Ok(meta) = std::fs::metadata(&file) else {
                continue;
            };
            if skip(&meta) {
                let seen = Seen::at_end(&file, &meta);
                files.insert(file, seen);
            }
        }
        Self {
            dir: dir.to_path_buf(),
            files,
        }
    }

    /// New records in file order, then line order. A file that changed is read
    /// from the start when it shrank or the bytes before its old offset changed; one that cannot
    /// be read is skipped until a later poll, keeping the state it had.
    pub fn poll(&mut self) -> Vec<TailedRecord> {
        let mut out = Vec::new();
        for file in crate::find_logs(&self.dir) {
            let _ = self.read_file(&file, &mut out);
        }
        out
    }

    fn read_file(&mut self, file: &Path, out: &mut Vec<TailedRecord>) -> std::io::Result<()> {
        let meta = std::fs::metadata(file)?;
        let modified = meta.modified().ok();
        let unchanged = self
            .files
            .get(file)
            .is_some_and(|seen| seen.len == meta.len() && seen.modified == modified);
        if unchanged {
            return Ok(());
        }

        // Known limit: a rewrite whose bytes before the old offset match is read as an append.
        // The state is replaced only after the reads, so a failed read keeps it.
        let mut f = std::fs::File::open(file)?;
        let resume = match self.files.get(file) {
            Some(Seen {
                offset,
                tail: Some(tail),
                ..
            }) if meta.len() >= *offset => same_bytes_before(&mut f, *offset, tail)?,
            _ => false,
        };
        let (start, kept, mut partial) = match self.files.get(file) {
            Some(seen) if resume => (seen.offset, seen.tail.clone(), seen.partial.clone()),
            _ => (0, None, Vec::new()),
        };
        f.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        let offset = start + buf.len() as u64;

        let mut complete = Vec::new();
        if !buf.is_empty() {
            // Kept as bytes until a line completes, so a multi-byte character
            // split across two polls decodes whole.
            partial.extend_from_slice(&buf);
            if let Some(i) = partial.iter().rposition(|&b| b == b'\n') {
                complete = partial.drain(..=i).collect();
            }
        }
        self.files.insert(
            file.to_path_buf(),
            Seen {
                offset,
                len: meta.len(),
                modified,
                tail: Some(kept_tail(kept, &buf)),
                partial,
            },
        );
        for record in parse_log(&String::from_utf8_lossy(&complete)) {
            out.push(TailedRecord {
                file: file.to_path_buf(),
                record,
            });
        }
        Ok(())
    }
}

/// Whether the bytes of `f` that end at `end` equal `tail`; a file too short to hold them does not.
fn same_bytes_before(f: &mut std::fs::File, end: u64, tail: &[u8]) -> std::io::Result<bool> {
    let Some(from) = end.checked_sub(tail.len() as u64) else {
        return Ok(false);
    };
    f.seek(SeekFrom::Start(from))?;
    let mut found = vec![0u8; tail.len()];
    match f.read_exact(&mut found) {
        Ok(()) => Ok(found == tail),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

/// The last up to [`KEPT_BYTES`] bytes of the old kept bytes followed by `read`.
fn kept_tail(old: Option<Vec<u8>>, read: &[u8]) -> Vec<u8> {
    let read = &read[read.len().saturating_sub(KEPT_BYTES as usize)..];
    let mut kept = old.unwrap_or_default();
    kept.extend_from_slice(read);
    let excess = kept.len().saturating_sub(KEPT_BYTES as usize);
    kept.drain(..excess);
    kept
}

impl Seen {
    /// A file [`LogTail::skipping`] starts at its end, so only what it gains is read.
    fn at_end(file: &Path, meta: &Metadata) -> Self {
        let len = meta.len().min(KEPT_BYTES);
        Self {
            offset: meta.len(),
            len: meta.len(),
            modified: meta.modified().ok(),
            tail: read_tail(file, meta.len(), len),
            partial: Vec::new(),
        }
    }
}

fn modified_before(meta: &Metadata, since_millis: i64) -> bool {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .is_some_and(|age| (age.as_millis() as i64) < since_millis)
}
