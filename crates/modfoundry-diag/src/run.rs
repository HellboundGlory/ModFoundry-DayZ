//! Process bridge + log capture. Spawns the engine binary from a profile, watches the
//! `-profiles` dir for the RPT/script.log files the process writes, and stops
//! the child after a bounded window or on exit — capturing what it produced.
//!
//! IO lives here (the boundary); nothing in `parse`/`map`/`profile` touches the
//! filesystem or processes, so they run headless.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A cooperative cancel flag shared with a running capture. `Clone` shares the
/// same underlying flag, so a caller can stop a launch from another thread.
#[derive(Clone, Default)]
pub struct StopSignal(Arc<AtomicBool>);

impl StopSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stop(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Whether `other` is a clone of this signal.
    pub fn same_flag(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What `capture_run` collected from a single launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    /// Exit code (`None` = killed by signal / timeout).
    pub exit_code: Option<i32>,
    /// Log files (RPT / script.log / etc.) found under the profiles dir.
    pub logs: Vec<PathBuf>,
    /// Whether the process was still running when the window elapsed.
    pub timed_out: bool,
    /// Whether the capture ended because [`StopSignal`] was set.
    #[serde(default)]
    pub stopped: bool,
}

/// Build and spawn the child. `ends_with_parent` makes the child die with the thread that spawns
/// it, which is what a server run wants; the client does not set it, because `capture_client`
/// returns while its launcher keeps running and the signal follows that thread.
/// `ExecutableFileBusy` (ETXTBSY) can transiently fire when a freshly-written script is exec'd
/// while another thread in the process still holds it open for writing; retry briefly rather than
/// fail a legitimate launch (rust-lang/rust#37528 class).
fn spawn_retry(cmd: &LaunchCommand, ends_with_parent: bool) -> anyhow::Result<std::process::Child> {
    let mut command = Command::new(&cmd.program);
    command
        .args(&cmd.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &cmd.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &cmd.env {
        command.env(key, value);
    }
    for key in &cmd.env_remove {
        command.env_remove(key);
    }
    if ends_with_parent {
        modfoundry_platform::end_with_parent(&mut command);
    }
    let mut last_err = None;
    for _ in 0..20 {
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(anyhow::Error::from(last_err.unwrap()))
}

/// The last [`OUTPUT_TAIL_BYTES`] of a child's combined stdout and stderr.
#[derive(Default)]
struct OutputTail(std::sync::Mutex<std::collections::VecDeque<u8>>);

/// How much of a child's output an error keeps.
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;

impl OutputTail {
    fn push(&self, bytes: &[u8]) {
        let mut tail = self.0.lock().unwrap();
        tail.extend(bytes);
        let excess = tail.len().saturating_sub(OUTPUT_TAIL_BYTES);
        tail.drain(..excess);
    }

    /// The kept output, trimmed.
    fn text(&self) -> String {
        let mut tail = self.0.lock().unwrap();
        String::from_utf8_lossy(tail.make_contiguous())
            .trim()
            .to_owned()
    }
}

/// Drain the child's stdout/stderr on background threads so a chatty engine
/// can't fill the pipe buffer and stall (logs come from files, not the pipes).
/// A `sink` keeps the tail of what the child printed; the returned handles say
/// when the drains have closed, so a caller that reports the output can wait.
fn drain_stdio(
    child: &mut std::process::Child,
    sink: Option<Arc<OutputTail>>,
) -> Vec<std::thread::JoinHandle<()>> {
    use std::io::Read;
    fn drain<R: Read + Send + 'static>(
        rd: R,
        sink: Option<Arc<OutputTail>>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut rd = rd;
            let mut buf = [0u8; 8192];
            loop {
                match rd.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if let Some(tail) = &sink {
                            tail.push(&buf[..read]);
                        }
                    }
                }
            }
        })
    }
    let mut handles = Vec::new();
    if let Some(o) = child.stdout.take() {
        handles.push(drain(o, sink.clone()));
    }
    if let Some(e) = child.stderr.take() {
        handles.push(drain(e, sink));
    }
    handles
}

/// Spawn `exe` with `argv` (the arguments passed after the executable) as a child,
/// wait up to `timeout` seconds while the
/// child writes logs into `profiles_dir`, then stop it and collect the log
/// files present. Returns whatever the run produced.
///
/// The child is spawned with its **cwd set to `exe`'s parent directory** — the
/// DayZ engine resolves `-mod`, config and addon paths relative to its install
/// dir (empirically it ignores absolute `-mod=` paths on Linux), so callers must
/// pass mod/config paths relative to that cwd. `profiles_dir` must already exist.
pub fn capture_run(
    exe: &Path,
    argv: &[String],
    profiles_dir: &Path,
    timeout_secs: u64,
    stop: &StopSignal,
) -> anyhow::Result<RunReport> {
    // The engine resolves -mod/config/addon paths relative to its install dir
    // (ignores absolute -mod= on Linux), so spawn in the exe's parent dir.
    let cwd = exe
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let cmd = LaunchCommand {
        program: exe.to_path_buf(),
        args: argv.iter().map(Into::into).collect(),
        cwd: Some(cwd.to_path_buf()),
        env: Vec::new(),
        env_remove: Vec::new(),
    };
    capture_command(&cmd, profiles_dir, timeout_secs, stop)
}

/// Spawn `cmd` (program, args, cwd, env already set) and wait up to `timeout_secs`
/// for it to exit, capturing the log files it writes into `profiles_dir` — the
/// same bounded window, stop signal and snapshot as [`capture_run`]. A timeout or a stop ends
/// the child's whole process tree the host can still find through it, not only the child
/// itself: under Proton the child is a launcher over a chain of wrappers. `profiles_dir` must
/// already exist.
pub fn capture_command(
    cmd: &LaunchCommand,
    profiles_dir: &Path,
    timeout_secs: u64,
    stop: &StopSignal,
) -> anyhow::Result<RunReport> {
    require_dir(profiles_dir)?;
    // Checked before the spawn: a panic after it would leave the server running.
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(timeout_secs))
        .ok_or_else(|| anyhow::anyhow!("a run length of {timeout_secs} s is too long"))?;
    let before = log_states(profiles_dir);
    let mut child = spawn_retry(cmd, true)?;
    drain_stdio(&mut child, None);

    let mut timed_out = false;
    let mut stopped = false;
    let mut exit_code: Option<i32> = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code();
                break;
            }
            Ok(None) => {
                if stop.is_stopped() {
                    stopped = true;
                    break;
                }
                if Instant::now() >= deadline {
                    timed_out = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => anyhow::bail!("waiting on child: {e}"),
        }
    }

    // If it outlived the window or was stopped, stop it so the run is bounded.
    if timed_out || stopped {
        modfoundry_platform::stop_process_tree(child.id());
        let _ = child.wait();
    }

    // Snapshot after the process has stopped writing (it exited or we timed out
    // and killed it — a short grace lets the engine flush its RPT on shutdown).
    std::thread::sleep(Duration::from_millis(300));
    let logs = find_logs(profiles_dir)
        .into_iter()
        .filter(|p| !unchanged(p, before.get(p)))
        .collect();

    Ok(RunReport {
        exit_code,
        logs,
        timed_out,
        stopped,
    })
}

/// RPT and script-log file extensions we treat as captures.
fn is_log_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".rpt") || lower.ends_with(".log") || lower.ends_with(".adm")
}

/// Enumerate the RPT + script.log files under `dir` (newest-suffix named, so
/// multiple runs coexist). Pure-ish: reads the directory.
pub fn find_logs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .is_some_and(|n| is_log_name(&n.to_string_lossy()))
        })
        .collect();
    logs.sort();
    logs
}

/// A process launch description: program, argv, working dir and environment
/// deltas. Built at the boundary (platform/proton) so the client capture stays
/// independent of where the command came from.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchCommand {
    pub program: PathBuf,
    /// Arguments, excluding the program itself.
    pub args: Vec<OsString>,
    /// Working directory; `None` inherits the caller's.
    pub cwd: Option<PathBuf>,
    /// Environment entries to set.
    pub env: Vec<(String, OsString)>,
    /// Environment variable names to drop from the inherited environment.
    pub env_remove: Vec<String>,
}

/// What a client launch collected: which log files the client wrote into its
/// Proton prefix dir, plus whether we stopped at the window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRunReport {
    /// The launched process's exit code when it had already exited, else `None`
    /// — under Proton the runtime may live as long as the game, so a `None`
    /// here does not mean the launch failed.
    pub exit_code: Option<i32>,
    /// Log files that appeared under the capture dir *after* launch.
    pub logs: Vec<PathBuf>,
    /// The log files already present when the capture started, so a caller can
    /// re-collect everything written after the capture settled.
    #[serde(skip)]
    pub before: std::collections::HashSet<PathBuf>,
    /// Whether the capture window elapsed without the logs settling.
    pub timed_out: bool,
    /// Whether the capture ended because [`StopSignal`] was set.
    #[serde(default)]
    pub stopped: bool,
}

/// Total byte size of a set of files (missing files count as 0).
fn total_size(files: &[PathBuf]) -> u64 {
    files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

/// A missing capture folder would read as a run that wrote no logs.
fn require_dir(dir: &Path) -> anyhow::Result<()> {
    if !dir.is_dir() {
        anyhow::bail!("capture folder {} does not exist", dir.display());
    }
    Ok(())
}

/// How long after a clean exit the client's first log may still appear: a
/// launcher hands off to Steam and exits before the game writes anything.
pub const CLIENT_EXIT_GRACE_SECS: u64 = 60;

/// How long a still-running client may take to write its first log.
pub const CLIENT_FIRST_LOG_SECS: u64 = 300;

/// The limits [`capture_client`] watches a launch under.
#[derive(Debug, Clone, Copy)]
pub struct ClientWait {
    /// The whole capture window.
    pub timeout_secs: u64,
    /// The quiet period that marks a complete client log.
    pub settle_secs: u64,
    /// How long after a clean exit the first log may still appear.
    pub exit_grace: Duration,
    /// How long a running client may take to write its first log.
    pub first_log: Duration,
}

impl ClientWait {
    /// The limits a real launch uses, with [`CLIENT_EXIT_GRACE_SECS`] and
    /// [`CLIENT_FIRST_LOG_SECS`].
    pub fn new(timeout_secs: u64, settle_secs: u64) -> Self {
        Self {
            timeout_secs,
            settle_secs,
            exit_grace: Duration::from_secs(CLIENT_EXIT_GRACE_SECS),
            first_log: Duration::from_secs(CLIENT_FIRST_LOG_SECS),
        }
    }
}

/// How often the capture re-checks the log dir before any log appears, so a
/// launcher that fails at once is reported at once.
const FIRST_LOG_POLL: Duration = Duration::from_millis(25);

/// How often the capture re-checks a log dir that already holds new logs.
const SETTLED_POLL: Duration = Duration::from_millis(250);

/// The kept output as the closing clause of an error.
fn tail_clause(tail: &OutputTail) -> String {
    let text = tail.text();
    if text.is_empty() {
        "no output".to_owned()
    } else {
        format!("last output:\n{text}")
    }
}

/// Launch `cmd` and capture the log files the client writes into `log_dir` —
/// the Proton prefix's `AppData/Local/DayZ`, not a `-profiles` dir.
///
/// The client is started under Steam's Proton runtime (see
/// `modfoundry_platform::proton_launch`), so the spawned process may live as
/// long as the game; this snapshots the log dir, launches, then polls for *new*
/// log files until they stop growing for `settle_secs` (a crashed/broken client
/// flushes and goes quiet) or `timeout_secs` elapses. Returns the new files.
///
/// Ends with an error, naming what happened and the client's last output, when
/// the launcher exits without leaving a log or a running client stays silent for
/// `first_log`: either way the run can never produce a report.
///
/// `settle_secs` must exceed the client's longest quiet gap during a normal
/// boot (observed ~27s between the RPT header and world/entity output), or the
/// capture returns before the mod's scripts finish compiling. `log_dir` need not
/// exist yet: a prefix the game has not run in has no folder, so the capture
/// starts with nothing to skip and picks the logs up once the client writes them.
pub fn capture_client(
    cmd: &LaunchCommand,
    log_dir: &Path,
    wait: &ClientWait,
    stop: &StopSignal,
) -> anyhow::Result<ClientRunReport> {
    let before: HashSet<PathBuf> = find_logs(log_dir).into_iter().collect();
    let tail = Arc::new(OutputTail::default());

    let started = Instant::now();
    let deadline = started
        .checked_add(Duration::from_secs(wait.timeout_secs))
        .ok_or_else(|| anyhow::anyhow!("a run length of {} s is too long", wait.timeout_secs))?;
    let mut child = spawn_retry(cmd, false)?;
    let drains = drain_stdio(&mut child, Some(Arc::clone(&tail)));

    let settle = Duration::from_secs(wait.settle_secs);
    let mut timed_out = false;
    let mut stopped = false;
    let mut exited_at = None;
    let mut last_size = 0u64;
    let mut last_change = Instant::now();
    let mut failure = None;
    let new_logs: Vec<PathBuf> = loop {
        let logs: Vec<PathBuf> = find_logs(log_dir)
            .into_iter()
            .filter(|p| !before.contains(p))
            .collect();
        let size = total_size(&logs);
        let no_logs = size == 0;
        if size != last_size {
            last_size = size;
            last_change = Instant::now();
        }
        let now = Instant::now();
        if !no_logs && now.duration_since(last_change) >= settle {
            break logs;
        }
        if stop.is_stopped() {
            stopped = true;
            break logs;
        }
        if now >= deadline {
            timed_out = true;
            break logs;
        }
        if let Some(status) = child.try_wait().ok().flatten() {
            let at = *exited_at.get_or_insert(now);
            let code = status.code();
            if code == Some(0) && no_logs {
                if now.duration_since(at) >= wait.exit_grace {
                    failure = Some(format!(
                        "exited without writing a log in {} (waited {} s after it exited)",
                        log_dir.display(),
                        wait.exit_grace.as_secs()
                    ));
                    break logs;
                }
            } else if no_logs {
                failure = Some(code.map_or_else(
                    || "exited before writing a log".to_owned(),
                    |code| format!("exited with code {code} before writing a log"),
                ));
                break logs;
            }
        } else if no_logs && now.duration_since(started) >= wait.first_log {
            modfoundry_platform::stop_process_tree(child.id());
            failure = Some(format!(
                "wrote no log in {} within {} s and was killed",
                log_dir.display(),
                wait.first_log.as_secs()
            ));
            break logs;
        }
        // The child (Proton runtime) may run for as long as the game; keep
        // polling the log dir regardless of its state.
        std::thread::sleep(if no_logs {
            FIRST_LOG_POLL
        } else {
            SETTLED_POLL
        });
    };

    if let Some(reason) = failure {
        let _ = child.wait();
        // A killed client's children may still hold its pipes open, so the
        // output tail lands within a moment or is read as it is.
        let until = Instant::now() + Duration::from_millis(200);
        while drains.iter().any(|drain| !drain.is_finished()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        anyhow::bail!("the client {reason}; {}", tail_clause(&tail));
    }

    // Best-effort: reap the child if it already exited; a still-running game
    // process is left alone.
    let exit_code = match child.try_wait() {
        Ok(Some(status)) => status.code(),
        _ => None,
    };

    Ok(ClientRunReport {
        exit_code,
        logs: new_logs,
        before,
        timed_out,
        stopped,
    })
}

/// Outcome of [`wait_for_log_marker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerWait {
    /// `marker` appeared on a line in a log file under the dir.
    Found,
    /// The deadline elapsed without the marker appearing.
    TimedOut,
    /// The wait ended because [`StopSignal`] was set.
    Stopped,
}

/// A log's size and modified time, to tell a file this run wrote to from one
/// it only found (a mod's fixed-name log is rewritten every run).
fn log_state(log: &Path) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata(log).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// How many bytes of a log [`log_states`] keeps, to tell an append from a rewrite.
const LOG_TAIL_BYTES: u64 = 256;

/// A log as it was before a run: its size and modified time, and the last bytes it
/// held, so a log the run appended to is told from one it rewrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogState {
    size: u64,
    modified: std::time::SystemTime,
    /// `None` when the bytes could not be read; such a log is read whole.
    tail: Option<Vec<u8>>,
}

impl LogState {
    fn of(log: &Path) -> Option<Self> {
        let (size, modified) = log_state(log)?;
        Some(Self {
            size,
            modified,
            tail: read_tail(log, size, size.min(LOG_TAIL_BYTES)),
        })
    }
}

/// The `len` bytes of `log` that end at `end`.
pub(crate) fn read_tail(log: &Path, end: u64, len: u64) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut tail = vec![0u8; len as usize];
    let mut file = std::fs::File::open(log).ok()?;
    file.seek(SeekFrom::Start(end.checked_sub(len)?)).ok()?;
    file.read_exact(&mut tail).ok()?;
    Some(tail)
}

/// Each log under a folder as it was, taken before a run so a kept folder's older
/// logs are told apart from what the run writes.
pub type LogStates = HashMap<PathBuf, LogState>;

/// The [`LogStates`] of `dir` now.
pub fn log_states(dir: &Path) -> LogStates {
    find_logs(dir)
        .into_iter()
        .filter_map(|log| LogState::of(&log).map(|state| (log, state)))
        .collect()
}

/// Whether `log` still has the size and modified time recorded in `earlier`.
fn unchanged(log: &Path, earlier: Option<&LogState>) -> bool {
    earlier.map(|state| (state.size, state.modified)) == log_state(log)
}

/// Where this run's part of `log` begins: its earlier size when it only grew since
/// `earlier` was taken, else 0.
fn run_start(log: &Path, earlier: &LogStates) -> u64 {
    let Some(LogState {
        size,
        tail: Some(tail),
        ..
    }) = earlier.get(log)
    else {
        return 0;
    };
    let grew = std::fs::metadata(log).is_ok_and(|meta| meta.len() >= *size);
    if grew && read_tail(log, *size, tail.len() as u64).as_ref() == Some(tail) {
        *size
    } else {
        0
    }
}

/// `log`'s bytes from [`run_start`] to its end, decoded lossily so a legacy code-page
/// byte costs no line.
pub fn run_log_text(log: &Path, earlier: &LogStates) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(log)?;
    file.seek(SeekFrom::Start(run_start(log, earlier)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The RPT logs under `dir` this run wrote to: an `earlier` log (an older run's,
/// in a kept folder) is read only once it changed.
fn rpt_logs(dir: &Path, earlier: &LogStates) -> Vec<PathBuf> {
    find_logs(dir)
        .into_iter()
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rpt"))
        })
        .filter(|p| !unchanged(p, earlier.get(p)))
        .collect()
}

/// Whether `line` is `marker` alone, after the time the engine may put first: a
/// mod's print into the same log carries its own channel and text.
fn is_marker_line(line: &str, marker: &str) -> bool {
    let line = line.trim();
    line == marker
        || line
            .split_once(char::is_whitespace)
            .is_some_and(|(time, rest)| time.contains(':') && rest.trim() == marker)
}

/// Incremental reads of one ready marker: each log keeps how far it was read and
/// its unfinished last line, so a poll reads only bytes the log gained.
#[derive(Default)]
struct MarkerScan {
    read: HashMap<PathBuf, (u64, Vec<u8>)>,
}

impl MarkerScan {
    /// Whether `path` gained a line since the last read that `sees_line` accepts; a
    /// log that shrank is read from 0.
    fn sees_new_line(
        &mut self,
        path: &Path,
        earlier: &LogStates,
        sees_line: &mut impl FnMut(&str) -> bool,
    ) -> bool {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(path) else {
            return false;
        };
        let Ok(len) = file.metadata().map(|m| m.len()) else {
            return false;
        };
        // An older run's log counts only for what it gained since this run began.
        let (offset, last_line) = self
            .read
            .entry(path.to_path_buf())
            .or_insert_with(|| (run_start(path, earlier), Vec::new()));
        if len < *offset {
            *offset = 0;
            last_line.clear();
        }
        if len == *offset {
            return false;
        }
        if file.seek(SeekFrom::Start(*offset)).is_err() {
            return false;
        }
        let mut gained = Vec::new();
        if file.take(len - *offset).read_to_end(&mut gained).is_err() {
            return false;
        }
        *offset = len;
        // The unfinished line stays raw bytes, so a character split across reads decodes whole.
        let mut bytes = std::mem::take(last_line);
        bytes.extend_from_slice(&gained);
        let finished_len = bytes
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |end| end + 1);
        last_line.extend_from_slice(&bytes[finished_len..]);
        String::from_utf8_lossy(&bytes[..finished_len])
            .lines()
            .any(sees_line)
    }
}

/// Wait up to `timeout` for a line in an RPT log under one of `watched` to satisfy
/// `sees_line`, polling with `sleep` between checks and reading only what each log
/// gained. An existing line wins; a stop wins over the deadline. A `timeout` too
/// long to add to the clock never expires: the wait ends on a line or a stop.
/// Pure over the files.
pub fn wait_for_log_lines(
    watched: &[(&Path, &LogStates)],
    timeout: Duration,
    stop: &StopSignal,
    mut sees_line: impl FnMut(&str) -> bool,
    mut sleep: impl FnMut(),
) -> MarkerWait {
    let deadline = Instant::now().checked_add(timeout);
    let mut scans: Vec<MarkerScan> = watched.iter().map(|_| MarkerScan::default()).collect();
    loop {
        let found = watched
            .iter()
            .zip(&mut scans)
            .any(|((dir, earlier), scan)| {
                rpt_logs(dir, earlier)
                    .iter()
                    .any(|path| scan.sees_new_line(path, earlier, &mut sees_line))
            });
        if found {
            return MarkerWait::Found;
        }
        if stop.is_stopped() {
            return MarkerWait::Stopped;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return MarkerWait::TimedOut;
        }
        sleep();
    }
}

/// Wait up to `timeout` for `marker` to appear on a line in an RPT log under
/// `dir`, polling with `sleep` between checks and reading only what each log
/// gained. An already-present marker wins; a stop wins over the deadline. Pure
/// over the files.
pub fn wait_for_log_marker(
    dir: &Path,
    earlier: &LogStates,
    marker: &str,
    timeout: Duration,
    stop: &StopSignal,
    sleep: impl FnMut(),
) -> MarkerWait {
    wait_for_log_lines(
        &[(dir, earlier)],
        timeout,
        stop,
        |line| is_marker_line(line, marker),
        sleep,
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        capture_client, capture_command, capture_run, find_logs, log_states, run_start,
        wait_for_log_lines, wait_for_log_marker, ClientWait, LaunchCommand, LogStates, MarkerWait,
        StopSignal,
    };
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    /// Unique-per-call suffix so parallel tests in one binary never write the
    /// same path.
    fn unique() -> u64 {
        static C: AtomicU64 = AtomicU64::new(0);
        C.fetch_add(1, Ordering::Relaxed)
    }

    /// A fake "server": a shell script that writes a script.log + an RPT into
    /// its profiles dir (argv[1]) then sleeps — proving we capture what a
    /// long-running engine process writes. Uses only /bin/sh.
    fn fake_server_script(profiles: &Path) -> PathBuf {
        let sh = std::env::temp_dir().join(format!(
            "mf-diag-fake-{}-{}-{}.sh",
            std::process::id(),
            unique(),
            profiles.file_name().unwrap().to_string_lossy()
        ));
        std::fs::write(
            &sh,
            "#!/bin/sh\nprintf ' SCRIPT    (E): Virtual Machine Exception\\r\\n' > \"$1/script.log\"\nprintf '15:00:00.000  ENGINE    (E): boom\\r\\n' > \"$1/DayZServer_x64.RPT\"\nsleep 2\n",
        )
        .unwrap();
        sh
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn capture_run_collects_logs_written_by_child() {
        let profiles = std::env::temp_dir().join(format!(
            "mf-diag-profiles-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&profiles).unwrap();
        let script = fake_server_script(&profiles);
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let argv = vec![
            script.to_string_lossy().into_owned(),
            profiles.to_string_lossy().into_owned(),
        ];
        let report = capture_run(
            &PathBuf::from("/bin/sh"),
            &argv,
            &profiles,
            5,
            &StopSignal::new(),
        )
        .unwrap();
        assert_eq!(report.exit_code, Some(0));
        assert!(report.logs.iter().any(|l| l.ends_with("script.log")));
        assert!(report
            .logs
            .iter()
            .any(|l| l.ends_with("DayZServer_x64.RPT")));
        let _ = std::fs::remove_dir_all(&profiles);
        let _ = std::fs::remove_file(&script);
    }

    #[test]
    fn a_run_reports_only_the_logs_it_wrote() {
        let profiles =
            std::env::temp_dir().join(format!("mf-diag-kept-{}-{}", std::process::id(), unique()));
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(profiles.join("script_old.log"), b"stale\r\n").unwrap();
        std::fs::write(profiles.join("MyMod.log"), b"earlier run\r\n").unwrap();
        let script = std::env::temp_dir().join(format!(
            "mf-diag-kept-{}-{}.sh",
            std::process::id(),
            unique()
        ));
        write_exec(
            &script,
            "#!/bin/sh\nprintf 'fresh\\r\\n' > \"$1/script_new.log\"\nprintf 'this run\\r\\n' >> \"$1/MyMod.log\"\n",
        );
        let argv = vec![
            script.to_string_lossy().into_owned(),
            profiles.to_string_lossy().into_owned(),
        ];
        let report = capture_run(
            &PathBuf::from("/bin/sh"),
            &argv,
            &profiles,
            5,
            &StopSignal::new(),
        )
        .unwrap();
        let names: Vec<String> = report
            .logs
            .iter()
            .filter_map(|l| l.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["MyMod.log", "script_new.log"]);
        let _ = std::fs::remove_dir_all(&profiles);
        let _ = std::fs::remove_file(&script);
    }

    /// A fake Steam launcher: a script that ignores its args and writes a client
    /// RPT + script.log into the log dir (passed as $1), like a client that
    /// failed on a broken mod script then exited.
    fn fake_launcher(log_dir: &Path) -> PathBuf {
        let sh = std::env::temp_dir().join(format!(
            "mf-diag-fakeclient-{}-{}.sh",
            std::process::id(),
            unique()
        ));
        std::fs::write(
            &sh,
            format!(
                "#!/bin/sh\nprintf ' 1:17:37.261 ENGINE    (E): boom\\r\\n' > \"{d}/DayZ_x64_x.RPT\"\nprintf ' SCRIPT    (E): @\"MyMod/scripts/3_Game/Broken.c,5\": boom\\r\\n' > \"{d}/script_x.log\"\n",
                d = log_dir.display()
            ),
        )
        .unwrap();
        sh
    }

    /// A launcher [`LaunchCommand`] running `sh_script` with `/bin/sh`, no cwd
    /// override and an empty env.
    fn sh_command(script: &Path) -> LaunchCommand {
        LaunchCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec![script.as_os_str().to_os_string()],
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        }
    }

    fn write_exec(script: &Path, body: &str) {
        std::fs::write(script, body).unwrap();
        std::fs::set_permissions(script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn capture_client_returns_only_new_logs_from_log_dir() {
        let log_dir = std::env::temp_dir().join(format!(
            "mf-diag-clientlogs-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&log_dir).unwrap();
        // A pre-existing log must NOT be reported (new files only).
        std::fs::write(log_dir.join("old.RPT"), b"stale\r\n").unwrap();
        let launcher = fake_launcher(&log_dir);
        std::fs::set_permissions(
            &launcher,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        // Short settle keeps the test fast; the product default is much larger.
        let report = capture_client(
            &sh_command(&launcher),
            &log_dir,
            &ClientWait::new(15, 1),
            &StopSignal::new(),
        )
        .unwrap();
        // The fake launcher exits immediately; the new logs settle -> no timeout.
        assert!(!report.timed_out);
        assert!(report.logs.iter().any(|l| l.ends_with("DayZ_x64_x.RPT")));
        assert!(report.logs.iter().any(|l| l.ends_with("script_x.log")));
        assert!(
            !report.logs.iter().any(|l| l.ends_with("old.RPT")),
            "pre-existing logs must not be reported, got {:?}",
            report.logs
        );
        let _ = std::fs::remove_dir_all(&log_dir);
        let _ = std::fs::remove_file(&launcher);
    }

    /// A launcher script that records the cwd, an env var it was given and a var
    /// that must have been removed, then writes a client log into `$MF_DIAG_LOG_DIR`.
    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn capture_client_applies_cwd_env_and_env_remove() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-launchcmd-{}-{}",
            std::process::id(),
            unique()
        ));
        let cwd = dir.join("workdir");
        let log_dir = dir.join("logs");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&log_dir).unwrap();
        let record = dir.join("record.txt");
        let script = dir.join("launcher.sh");
        let leaked = format!("MF_DIAG_LEAK_{}_{}", std::process::id(), unique());
        std::env::set_var(&leaked, "leaked");
        write_exec(
            &script,
            &format!(
                "#!/bin/sh\n{{\n\
                 printf 'cwd=%s\\n' \"$(pwd)\"\n\
                 printf 'tag=%s\\n' \"$MF_DIAG_TAG\"\n\
                 printf 'leak=%s\\n' \"${{{leaked}-absent}}\"\n\
                 }} > \"$MF_DIAG_RECORD\"\n\
                 printf ' 1:00:00.000 ENGINE    (E): boot\\r\\n' > \"$MF_DIAG_LOG_DIR/DayZ_x64_x.RPT\"\n"
            ),
        );
        let cmd = LaunchCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec![script.as_os_str().to_os_string()],
            cwd: Some(cwd.clone()),
            env: vec![
                ("MF_DIAG_TAG".to_string(), OsString::from("hello")),
                (
                    "MF_DIAG_RECORD".to_string(),
                    record.as_os_str().to_os_string(),
                ),
                (
                    "MF_DIAG_LOG_DIR".to_string(),
                    log_dir.as_os_str().to_os_string(),
                ),
            ],
            env_remove: vec![leaked.clone()],
        };
        let report =
            capture_client(&cmd, &log_dir, &ClientWait::new(10, 1), &StopSignal::new()).unwrap();
        let text = std::fs::read_to_string(&record).unwrap();
        let canonical = std::fs::canonicalize(&cwd).unwrap();
        assert!(
            text.contains(&format!("cwd={}", canonical.display())),
            "child cwd not applied: {text}"
        );
        assert!(text.contains("tag=hello"), "child env not applied: {text}");
        assert!(
            text.contains("leak=absent"),
            "env_remove not applied: {text}"
        );
        assert!(report.logs.iter().any(|l| l.ends_with("DayZ_x64_x.RPT")));
        std::env::remove_var(&leaked);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn capture_client_leaves_a_still_running_launcher_alive() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-liveclient-{}-{}",
            std::process::id(),
            unique()
        ));
        let log_dir = dir.join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        let record = dir.join("pid.txt");
        let script = dir.join("launcher.sh");
        write_exec(
            &script,
            "#!/bin/sh\necho $$ > \"$MF_DIAG_RECORD\"\n\
             printf ' 1:00:00.000 ENGINE    (E): boot\\r\\n' > \"$MF_DIAG_LOG_DIR/DayZ_x64_y.RPT\"\n\
             exec sleep 10\n",
        );
        let cmd = LaunchCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec![script.as_os_str().to_os_string()],
            cwd: None,
            env: vec![
                (
                    "MF_DIAG_RECORD".to_string(),
                    record.as_os_str().to_os_string(),
                ),
                (
                    "MF_DIAG_LOG_DIR".to_string(),
                    log_dir.as_os_str().to_os_string(),
                ),
            ],
            env_remove: Vec::new(),
        };
        let start = Instant::now();
        let report =
            capture_client(&cmd, &log_dir, &ClientWait::new(30, 1), &StopSignal::new()).unwrap();
        assert!(!report.timed_out, "the settled logs must end the capture");
        assert_eq!(
            report.exit_code, None,
            "a client still running must report no exit code"
        );
        assert!(
            start.elapsed() < Duration::from_secs(9),
            "capture waited for the child instead of settling: {:?}",
            start.elapsed()
        );
        assert!(report.logs.iter().any(|l| l.ends_with("DayZ_x64_y.RPT")));

        let pid = std::fs::read_to_string(&record).unwrap().trim().to_string();
        let alive = Command::new("sh")
            .args(["-c", &format!("kill -0 {pid}")])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(alive, "capture must not kill a still-running child");
        let _ = Command::new("sh")
            .args(["-c", &format!("kill {pid}")])
            .status();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn client_scratch(label: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-{label}-{}-{}",
            std::process::id(),
            unique()
        ));
        let log_dir = dir.join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        (dir, log_dir)
    }

    #[test]
    fn a_launcher_that_fails_without_a_log_reports_its_code_and_output() {
        let cases = [
            (
                "#!/bin/sh\nprintf 'wine: cannot find DayZ_x64.exe\\n' >&2\nexit 3\n",
                [
                    "exited with code 3 before writing a log",
                    "wine: cannot find",
                ],
            ),
            (
                "#!/bin/sh\nexit 3\n",
                ["exited with code 3 before writing a log", "no output"],
            ),
        ];
        for (body, want) in cases {
            let (dir, log_dir) = client_scratch("failing-launcher");
            let script = dir.join("launcher.sh");
            write_exec(&script, body);
            let start = Instant::now();

            let err = capture_client(
                &sh_command(&script),
                &log_dir,
                &ClientWait::new(5, 1),
                &StopSignal::new(),
            )
            .unwrap_err()
            .to_string();

            assert!(
                start.elapsed() < Duration::from_secs(2),
                "a dead launcher must not wait out the window: {:?}",
                start.elapsed()
            );
            for want in want {
                assert!(err.contains(want), "{want:?} missing from {err}");
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn an_empty_log_counts_as_no_log() {
        let (dir, log_dir) = client_scratch("empty-log");
        let script = dir.join("launcher.sh");
        write_exec(
            &script,
            &format!(
                "#!/bin/sh\n: > '{}/script_1.log'\nexit 3\n",
                log_dir.display()
            ),
        );

        let err = capture_client(
            &sh_command(&script),
            &log_dir,
            &ClientWait::new(5, 1),
            &StopSignal::new(),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("exited with code 3"), "{err}");
    }

    #[test]
    fn a_launcher_that_exits_clean_without_a_log_gives_up_after_the_grace() {
        let (dir, log_dir) = client_scratch("clean-launcher");
        let script = dir.join("launcher.sh");
        write_exec(&script, "#!/bin/sh\nexit 0\n");
        let wait = ClientWait {
            exit_grace: Duration::from_millis(200),
            ..ClientWait::new(5, 1)
        };
        let start = Instant::now();

        let err = capture_client(&sh_command(&script), &log_dir, &wait, &StopSignal::new())
            .unwrap_err()
            .to_string();

        assert!(
            start.elapsed() < Duration::from_secs(2),
            "a clean exit with no log must give up after the grace: {:?}",
            start.elapsed()
        );
        assert!(err.contains("exited without writing a log"), "{err}");
        assert!(err.contains("s after it exited"), "{err}");
        assert!(err.contains(&log_dir.display().to_string()), "{err}");
        assert!(err.contains("no output"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_launcher_that_hands_off_and_writes_a_log_later_is_a_normal_report() {
        let (dir, log_dir) = client_scratch("handoff");
        let script = dir.join("launcher.sh");
        write_exec(
            &script,
            &format!(
                "#!/bin/sh\n( sleep 0.1; printf ' 1:00:00.000 ENGINE    (E): boot\\r\\n' > '{}/DayZ_x64_z.RPT' ) &\nexit 0\n",
                log_dir.display()
            ),
        );
        let wait = ClientWait {
            exit_grace: Duration::from_secs(2),
            ..ClientWait::new(5, 0)
        };

        let report = capture_client(&sh_command(&script), &log_dir, &wait, &StopSignal::new())
            .expect("a log written after the hand-off is still a capture");

        assert!(!report.timed_out);
        assert!(report.logs.iter().any(|l| l.ends_with("DayZ_x64_z.RPT")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_launcher_that_never_writes_a_log_is_killed_after_the_first_log_window() {
        let (dir, log_dir) = client_scratch("silent-launcher");
        let record = dir.join("pid.txt");
        let script = dir.join("launcher.sh");
        write_exec(
            &script,
            &format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
                record.display()
            ),
        );
        let wait = ClientWait {
            first_log: Duration::from_millis(500),
            ..ClientWait::new(5, 1)
        };
        let start = Instant::now();

        let err = capture_client(&sh_command(&script), &log_dir, &wait, &StopSignal::new())
            .unwrap_err()
            .to_string();

        assert!(
            start.elapsed() < Duration::from_secs(3),
            "a silent launcher must not wait out the window: {:?}",
            start.elapsed()
        );
        assert!(err.contains("wrote no log in"), "{err}");
        assert!(err.contains(&log_dir.display().to_string()), "{err}");
        assert!(err.contains("and was killed"), "{err}");
        let pid = std::fs::read_to_string(&record).unwrap().trim().to_owned();
        let alive = Command::new("sh")
            .args(["-c", &format!("kill -0 {pid}")])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        assert!(!alive, "the client that wrote no log must be killed: {pid}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fake server that writes nothing and sleeps far past the test's window,
    /// so only a stop can end the capture early.
    fn sleeping_server() -> PathBuf {
        let sh = std::env::temp_dir().join(format!(
            "mf-diag-sleep-{}-{}.sh",
            std::process::id(),
            unique()
        ));
        std::fs::write(&sh, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&sh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        sh
    }

    fn stop_after(signal: &StopSignal, delay: Duration) {
        let signal = signal.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            signal.stop();
        });
    }

    #[test]
    fn capture_run_stops_early_on_a_stop_signal() {
        let profiles =
            std::env::temp_dir().join(format!("mf-diag-stop-{}-{}", std::process::id(), unique()));
        std::fs::create_dir_all(&profiles).unwrap();
        let script = sleeping_server();
        let argv = vec![
            script.to_string_lossy().into_owned(),
            profiles.to_string_lossy().into_owned(),
        ];
        let stop = StopSignal::new();
        stop_after(&stop, Duration::from_millis(300));

        let start = Instant::now();
        let report = capture_run(&PathBuf::from("/bin/sh"), &argv, &profiles, 30, &stop).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "stop did not end the run: {:?}",
            start.elapsed()
        );
        assert!(report.stopped);
        assert!(!report.timed_out);
        assert_eq!(report.exit_code, None);
        let _ = std::fs::remove_dir_all(&profiles);
        let _ = std::fs::remove_file(&script);
    }

    #[test]
    fn capture_client_stops_early_on_a_stop_signal() {
        let log_dir =
            std::env::temp_dir().join(format!("mf-diag-cstop-{}-{}", std::process::id(), unique()));
        std::fs::create_dir_all(&log_dir).unwrap();
        let stop = StopSignal::new();
        stop_after(&stop, Duration::from_millis(300));

        let start = Instant::now();
        let cmd = LaunchCommand {
            program: PathBuf::from("/bin/true"),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        };
        let report = capture_client(&cmd, &log_dir, &ClientWait::new(30, 30), &stop).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "stop did not end the capture: {:?}",
            start.elapsed()
        );
        assert!(report.stopped);
        assert!(!report.timed_out);
        let _ = std::fs::remove_dir_all(&log_dir);
    }

    /// A stub that records its cwd and two env vars (one set by the command,
    /// one that must have been removed) and writes an RPT into the profiles dir.
    #[test]
    fn capture_command_applies_cwd_env_and_env_remove() {
        let dir =
            std::env::temp_dir().join(format!("mf-diag-cmd-{}-{}", std::process::id(), unique()));
        let cwd = dir.join("workdir");
        let profiles = dir.join("profiles");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        let record = dir.join("record.txt");
        let script = dir.join("stub.sh");
        let leaked = format!("MF_DIAG_LEAKCMD_{}_{}", std::process::id(), unique());
        std::env::set_var(&leaked, "leaked");
        write_exec(
            &script,
            &format!(
                "#!/bin/sh\n{{\n\
                 printf 'cwd=%s\\n' \"$(pwd)\"\n\
                 printf 'tag=%s\\n' \"$MF_DIAG_TAG\"\n\
                 printf 'leak=%s\\n' \"${{{leaked}-absent}}\"\n\
                 }} > \"$MF_DIAG_RECORD\"\n\
                 printf '0:00:00.000  ENGINE    (E): boot\\r\\n' > \"$MF_DIAG_PROFILES/DayZServer_x64.RPT\"\n"
            ),
        );
        let cmd = LaunchCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec![script.as_os_str().to_os_string()],
            cwd: Some(cwd.clone()),
            env: vec![
                ("MF_DIAG_TAG".to_string(), OsString::from("hello")),
                (
                    "MF_DIAG_RECORD".to_string(),
                    record.as_os_str().to_os_string(),
                ),
                (
                    "MF_DIAG_PROFILES".to_string(),
                    profiles.as_os_str().to_os_string(),
                ),
            ],
            env_remove: vec![leaked.clone()],
        };
        let report = capture_command(&cmd, &profiles, 10, &StopSignal::new()).unwrap();
        let text = std::fs::read_to_string(&record).unwrap();
        let canonical = std::fs::canonicalize(&cwd).unwrap();
        assert!(
            text.contains(&format!("cwd={}", canonical.display())),
            "child cwd not applied: {text}"
        );
        assert!(text.contains("tag=hello"), "child env not applied: {text}");
        assert!(
            text.contains("leak=absent"),
            "env_remove not applied: {text}"
        );
        assert_eq!(report.exit_code, Some(0));
        assert!(report
            .logs
            .iter()
            .any(|l| l.ends_with("DayZServer_x64.RPT")));
        std::env::remove_var(&leaked);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whether `pid` still has a live, non-zombie process entry.
    #[cfg(target_os = "linux")]
    fn process_running(pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z")
    }

    #[cfg(target_os = "linux")]
    fn wait_until_gone(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while process_running(pid) {
            assert!(
                Instant::now() < deadline,
                "process {pid} outlived the run by more than 2 s"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A launcher that leaves a `sleep` behind and writes its pid to `record`.
    #[cfg(target_os = "linux")]
    fn launcher_with_grandchild(script: &Path, record: &Path) {
        write_exec(
            script,
            &format!(
                "#!/bin/sh\n(n=0; while [ -e \"$0\" ] && [ $n -lt 150 ]; do sleep 0.2; n=$((n+1)); done) &\necho $! > '{}'\nn=0; while [ -e \"$0\" ] && [ $n -lt 150 ]; do sleep 0.2; n=$((n+1)); done\n",
                record.display()
            ),
        );
    }

    #[cfg(target_os = "linux")]
    fn grandchild_pid(record: &Path) -> u32 {
        std::fs::read_to_string(record)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_silent_client_launcher_has_its_grandchild_killed_too() {
        let (dir, log_dir) = client_scratch("silent-tree");
        let record = dir.join("grandchild.pid");
        let script = dir.join("launcher.sh");
        launcher_with_grandchild(&script, &record);
        let wait = ClientWait {
            first_log: Duration::from_millis(500),
            ..ClientWait::new(5, 1)
        };

        let err = capture_client(&sh_command(&script), &log_dir, &wait, &StopSignal::new())
            .unwrap_err()
            .to_string();

        assert!(err.contains("and was killed"), "{err}");
        wait_until_gone(grandchild_pid(&record));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_stopped_command_has_its_grandchild_killed_too() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-stop-tree-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let record = dir.join("grandchild.pid");
        let script = dir.join("server.sh");
        launcher_with_grandchild(&script, &record);
        let cmd = LaunchCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec![script.as_os_str().to_os_string()],
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        };
        let stop = StopSignal::new();
        stop_after(&stop, Duration::from_millis(300));

        let report = capture_command(&cmd, &dir, 30, &stop).unwrap();

        assert!(report.stopped);
        wait_until_gone(grandchild_pid(&record));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_command_stops_early_on_a_stop_signal() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-cmdstop-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = sleeping_server();
        let cmd = LaunchCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec![script.as_os_str().to_os_string()],
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        };
        let stop = StopSignal::new();
        stop_after(&stop, Duration::from_millis(300));

        let start = Instant::now();
        let report = capture_command(&cmd, &dir, 30, &stop).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "stop did not end the command: {:?}",
            start.elapsed()
        );
        assert!(report.stopped);
        assert!(!report.timed_out);
        assert_eq!(report.exit_code, None);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&script);
    }

    #[test]
    #[ignore = "real process, over a second: cargo test -- --ignored"]
    fn capture_command_reports_timed_out_after_the_window() {
        let dir =
            std::env::temp_dir().join(format!("mf-diag-cmdto-{}-{}", std::process::id(), unique()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = sleeping_server();
        let cmd = LaunchCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec![script.as_os_str().to_os_string()],
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        };
        let report = capture_command(&cmd, &dir, 1, &StopSignal::new()).unwrap();
        assert!(report.timed_out);
        assert!(!report.stopped);
        assert_eq!(report.exit_code, None);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&script);
    }

    #[test]
    fn capture_command_refuses_a_window_too_long_to_time_before_spawning() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-cmdhuge-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // A spawn of a missing program fails with another message, so a check moved after it shows.
        let cmd = LaunchCommand {
            program: dir.join("missing"),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        };

        let err = capture_command(&cmd, &dir, u64::MAX, &StopSignal::new())
            .unwrap_err()
            .to_string();

        assert!(err.starts_with("a run length of"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_client_refuses_a_window_too_long_to_time_before_launching() {
        let (dir, log_dir) = client_scratch("huge-window");
        let marker = dir.join("launched.txt");
        let script = dir.join("launcher.sh");
        write_exec(&script, &format!("#!/bin/sh\n: > '{}'\n", marker.display()));

        let err = capture_client(
            &sh_command(&script),
            &log_dir,
            &ClientWait::new(u64::MAX, 1),
            &StopSignal::new(),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("too long"), "{err}");
        assert!(!marker.exists(), "the client must not be launched: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_client_accepts_a_log_folder_the_client_creates() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-clientmkdir-{}-{}",
            std::process::id(),
            unique()
        ));
        let log_dir = dir.join("AppData/Local/DayZ");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("launcher.sh");
        write_exec(
            &script,
            &format!(
                "#!/bin/sh\nmkdir -p '{log_dir}'\n\
                 printf ' 1:00:00.000 ENGINE    (E): boot\\r\\n' > '{log_dir}/DayZ_x64_new.RPT'\n",
                log_dir = log_dir.display()
            ),
        );
        let wait = ClientWait {
            exit_grace: Duration::from_secs(2),
            ..ClientWait::new(5, 0)
        };

        let report = capture_client(&sh_command(&script), &log_dir, &wait, &StopSignal::new())
            .expect("a log folder the client creates is not a failed capture");

        assert!(
            report.logs.iter().any(|l| l.ends_with("DayZ_x64_new.RPT")),
            "{:?}",
            report.logs
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_capture_folder_is_refused_before_launching() {
        let dir = std::env::temp_dir().join(format!("mf-diag-absent-{}", unique()));
        let cmd = LaunchCommand {
            program: PathBuf::from("/nonexistent/launcher"),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        };
        let err = capture_command(&cmd, &dir, 1, &StopSignal::new()).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn wait_for_log_lines_matches_a_line_in_either_folder() {
        let (server, client) = (
            std::env::temp_dir().join(format!(
                "mf-diag-lines-server-{}-{}",
                std::process::id(),
                unique()
            )),
            std::env::temp_dir().join(format!(
                "mf-diag-lines-client-{}-{}",
                std::process::id(),
                unique()
            )),
        );
        for (dir, text) in [(&server, "boot\r\n"), (&client, "DONE side=server\r\n")] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("DayZ_x64.RPT"), text).unwrap();
        }

        let found = wait_for_log_lines(
            &[(&server, &LogStates::new()), (&client, &LogStates::new())],
            Duration::from_secs(5),
            &StopSignal::new(),
            |line| line.contains("DONE"),
            || panic!("an existing line must not poll"),
        );

        assert_eq!(found, MarkerWait::Found);
        let _ = std::fs::remove_dir_all(&server);
        let _ = std::fs::remove_dir_all(&client);
    }

    #[test]
    fn wait_for_log_lines_with_a_huge_timeout_waits_for_a_stop_instead_of_panicking() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-hugetimeout-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let stop = StopSignal::new();
        let mut polls = 0u32;

        let ended = wait_for_log_lines(
            &[(&dir, &LogStates::new())],
            Duration::MAX,
            &stop,
            |_| false,
            || {
                polls += 1;
                if polls == 3 {
                    stop.stop();
                }
            },
        );

        assert_eq!(ended, MarkerWait::Stopped);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_lines_with_a_huge_timeout_still_finds_an_existing_line() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-hugetimeout2-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("DayZ_x64.RPT"), "boot\r\nDONE\r\n").unwrap();

        let found = wait_for_log_lines(
            &[(&dir, &LogStates::new())],
            Duration::MAX,
            &StopSignal::new(),
            |line| line.contains("DONE"),
            || panic!("an existing line must not poll"),
        );

        assert_eq!(found, MarkerWait::Found);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_marker_finds_an_existing_marker() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-marker-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("DayZServer_x64.RPT"), "boot\r\nWorld loaded\r\n").unwrap();
        let mut polls = 0;
        let found = wait_for_log_marker(
            &dir,
            &LogStates::new(),
            "World loaded",
            Duration::from_secs(5),
            &StopSignal::new(),
            || polls += 1,
        );
        assert_eq!(found, MarkerWait::Found);
        assert_eq!(polls, 0, "an existing marker must not poll");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_marker_finds_a_marker_written_between_polls() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-marker2-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut polls = 0;
        let found = wait_for_log_marker(
            &dir,
            &LogStates::new(),
            "World loaded",
            Duration::from_secs(5),
            &StopSignal::new(),
            || {
                polls += 1;
                if polls == 2 {
                    std::fs::write(dir.join("DayZ_x64.RPT"), "line\nWorld loaded\n").unwrap();
                }
            },
        );
        assert_eq!(found, MarkerWait::Found);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_logs_leaves_out_crash_dumps() {
        let dir =
            std::env::temp_dir().join(format!("mf-diag-dumps-{}-{}", std::process::id(), unique()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("DayZServer_x64.RPT"), "boot\r\n").unwrap();
        std::fs::write(dir.join("ErrorMessage_DayZ_x64.mdmp"), [0xFFu8, 0x00, 0x9C]).unwrap();
        let logs = find_logs(&dir);
        assert_eq!(logs, vec![dir.join("DayZServer_x64.RPT")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_marker_finds_a_marker_in_a_log_with_non_utf8_bytes() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-marker5-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("DayZServer_x64.RPT"),
            b"player \xE9\r\nWorld loaded\r\n",
        )
        .unwrap();
        let found = wait_for_log_marker(
            &dir,
            &LogStates::new(),
            "World loaded",
            Duration::from_millis(20),
            &StopSignal::new(),
            || std::thread::sleep(Duration::from_millis(5)),
        );
        assert_eq!(found, MarkerWait::Found);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_marker_times_out_without_the_marker() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-marker3-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let timed = wait_for_log_marker(
            &dir,
            &LogStates::new(),
            "never",
            Duration::from_millis(20),
            &StopSignal::new(),
            || std::thread::sleep(Duration::from_millis(5)),
        );
        assert_eq!(timed, MarkerWait::TimedOut);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_marker_judges_new_bytes_and_whole_lines() {
        let m = "Player connect enabled";
        let cases = [
            (
                "an old log's marker",
                format!("{m}\r\n"),
                "ENTITY (W): more\r\n",
                false,
                false,
            ),
            (
                "a mod's print",
                String::new(),
                "18:20:27.392  SCRIPT       : [MyMod] Player connect enabled = false\r\n",
                false,
                false,
            ),
            (
                "the engine's timed line",
                String::new(),
                "18:20:42.796 Player connect enabled\r\n",
                false,
                true,
            ),
            (
                "a bare line",
                String::new(),
                "Player connect enabled\n",
                false,
                true,
            ),
            (
                "an unfinished line",
                String::new(),
                "Player connect enabled",
                false,
                false,
            ),
            (
                "a rewritten longer log",
                String::from("x\r\n"),
                "Player connect enabled\r\n",
                true,
                true,
            ),
        ];
        for (label, old, gained, replace, want) in cases {
            let dir = std::env::temp_dir().join(format!(
                "mf-diag-marker8-{}-{}",
                std::process::id(),
                unique()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let log = dir.join("DayZServer_1.RPT");
            std::fs::write(&log, &old).unwrap();
            let earlier = log_states(&dir);
            let now = if replace {
                gained.to_owned()
            } else {
                format!("{old}{gained}")
            };
            std::fs::write(&log, now).unwrap();

            let got = wait_for_log_marker(
                &dir,
                &earlier,
                m,
                Duration::from_millis(20),
                &StopSignal::new(),
                || std::thread::sleep(Duration::from_millis(1)),
            );

            assert_eq!(got == MarkerWait::Found, want, "{label}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn wait_for_log_marker_ignores_the_marker_in_a_script_log() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-marker7-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("script_1.log"), "Player connect enabled\r\n").unwrap();
        let timed = wait_for_log_marker(
            &dir,
            &LogStates::new(),
            "Player connect enabled",
            Duration::from_millis(20),
            &StopSignal::new(),
            || std::thread::sleep(Duration::from_millis(5)),
        );
        assert_eq!(timed, MarkerWait::TimedOut);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_marker_finds_a_marker_split_across_reads() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-marker8-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("DayZ_x64.RPT");
        std::fs::write(&log, "boot\r\nPlayer conn").unwrap();
        let mut polls = 0;
        let found = wait_for_log_marker(
            &dir,
            &LogStates::new(),
            "Player connect enabled",
            Duration::from_secs(5),
            &StopSignal::new(),
            || {
                polls += 1;
                if polls == 1 {
                    use std::io::Write;
                    std::fs::OpenOptions::new()
                        .append(true)
                        .open(&log)
                        .unwrap()
                        .write_all(b"ect enabled\r\n")
                        .unwrap();
                }
            },
        );
        assert_eq!(found, MarkerWait::Found);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_marker_reads_a_shrunk_log_from_the_start() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-marker9-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("DayZ_x64.RPT");
        std::fs::write(&log, "boot\r\nengine started and then some more text\r\n").unwrap();
        let mut polls = 0;
        let found = wait_for_log_marker(
            &dir,
            &LogStates::new(),
            "Player connect enabled",
            Duration::from_secs(5),
            &StopSignal::new(),
            || {
                polls += 1;
                if polls == 1 {
                    std::fs::write(&log, "Player connect enabled\r\n").unwrap();
                }
            },
        );
        assert_eq!(found, MarkerWait::Found);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_marker_stop_beats_the_deadline() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-marker4-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let stop = StopSignal::new();
        stop.stop();
        let res = wait_for_log_marker(
            &dir,
            &LogStates::new(),
            "never",
            Duration::ZERO,
            &stop,
            || panic!("a stopped wait must not poll"),
        );
        assert_eq!(res, MarkerWait::Stopped);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_marker_ignores_logs_from_before_the_run() {
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-marker6-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("DayZServer_old.RPT");
        std::fs::write(&old, "World loaded\n").unwrap();
        let earlier = log_states(&dir);
        let timed = wait_for_log_marker(
            &dir,
            &earlier,
            "World loaded",
            Duration::from_millis(20),
            &StopSignal::new(),
            || std::thread::sleep(Duration::from_millis(5)),
        );
        assert_eq!(timed, MarkerWait::TimedOut);

        std::fs::write(&old, "World loaded\nWorld loaded\n").unwrap();
        let found = wait_for_log_marker(
            &dir,
            &earlier,
            "World loaded",
            Duration::from_millis(20),
            &StopSignal::new(),
            || std::thread::sleep(Duration::from_millis(5)),
        );
        assert_eq!(
            found,
            MarkerWait::Found,
            "a kept log this run wrote to is read"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_for_log_lines_joins_a_character_split_across_reads() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-split-char-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("DayZ_x64.RPT");
        std::fs::write(&log, b"caf\xC3").unwrap();
        let mut appended = false;

        let found = wait_for_log_lines(
            &[(&dir, &LogStates::new())],
            Duration::from_secs(5),
            &StopSignal::new(),
            |line| line.trim_end() == "caf\u{e9}",
            || {
                if !appended {
                    appended = true;
                    let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
                    file.write_all(b"\xA9\r\n").unwrap();
                }
                std::thread::sleep(Duration::from_millis(5));
            },
        );

        assert_eq!(found, MarkerWait::Found);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_log_that_cannot_be_read_still_counts_as_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "mf-diag-unreadable-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("DayZServer_1.RPT");
        std::fs::write(&log, "old\r\n").unwrap();
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&log).is_ok() {
            // Root, or a filesystem without mode bits: nothing to check here.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let earlier = log_states(&dir);

        assert!(super::rpt_logs(&dir, &earlier).is_empty());
        assert_eq!(run_start(&log, &earlier), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_start_resumes_an_appended_log_and_rereads_a_rewritten_one() {
        let cases = [
            ("an appended log", Some("old\r\n"), "old\r\nnew\r\n", 5),
            (
                "a rewritten longer log",
                Some("old\r\n"),
                "OLD\r\nmore text\r\n",
                0,
            ),
            ("a shrunk log", Some("old and then some\r\n"), "old\r\n", 0),
            ("a log new to the run", None, "fresh\r\n", 0),
        ];
        for (label, old, now, want) in cases {
            let dir = std::env::temp_dir().join(format!(
                "mf-diag-run-start-{}-{}",
                std::process::id(),
                unique()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let log = dir.join("DayZServer_1.RPT");
            if let Some(old) = old {
                std::fs::write(&log, old).unwrap();
            }
            let earlier = log_states(&dir);
            std::fs::write(&log, now).unwrap();

            assert_eq!(run_start(&log, &earlier), want, "{label}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
