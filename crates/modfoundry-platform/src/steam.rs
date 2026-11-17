//! The Steam helper's wire types and the runner that talks to it.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// Who can see a Workshop item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    FriendsOnly,
    #[default]
    Private,
    Unlisted,
}

/// One Steam key-value tag on a Workshop item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValueTag {
    pub key: String,
    pub value: String,
}

/// One request to the Steam helper, read as one JSON line on its stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum SteamRequest {
    Account,
    Create,
    Update(ItemUpdate),
    Delete { id: u64 },
    MyItems,
}

/// The fields of a Workshop item to change; a `None` field is left unchanged on Steam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemUpdate {
    pub id: u64,
    pub content_dir: PathBuf,
    pub preview: Option<PathBuf>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<Visibility>,
    pub tags: Option<Vec<String>>,
    pub remove_keys: Vec<String>,
    pub kv_tags: Vec<KeyValueTag>,
    pub change_note: Option<String>,
}

/// The Steam account the helper signed in with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteamAccount {
    pub name: String,
    pub steam_id: u64,
}

/// A Workshop item the account published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedItem {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub visibility: Visibility,
    pub tags: Vec<String>,
    pub kv_tags: Vec<KeyValueTag>,
    pub updated: u32,
}

/// The part of an upload the helper is working on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadStage {
    PreparingConfig,
    PreparingContent,
    UploadingContent,
    UploadingPreview,
    Committing,
}

/// One event line the Steam helper prints on its stdout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SteamEvent {
    Account(SteamAccount),
    Created {
        id: u64,
        needs_legal_agreement: bool,
    },
    Progress {
        stage: UploadStage,
        done: u64,
        total: u64,
    },
    Submitted {
        id: u64,
        needs_legal_agreement: bool,
    },
    Deleted {
        id: u64,
    },
    Items {
        account: SteamAccount,
        items: Vec<PublishedItem>,
    },
    Failed {
        message: String,
    },
}

/// Why a request to the Steam helper produced no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteamError {
    HelperMissing(PathBuf),
    Spawn(String),
    Steam(String),
    Protocol(String),
    /// The helper exited without an answer, as it does when Steam closes or hangs mid-request.
    Ended(String),
    Cancelled,
}

impl std::fmt::Display for SteamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SteamError::HelperMissing(path) => write!(
                f,
                "the Steam helper is missing: {}; build it with cargo build -p modfoundry-steam",
                path.display()
            ),
            SteamError::Spawn(message) => write!(f, "could not start the Steam helper: {message}"),
            SteamError::Steam(message) => write!(f, "{message}"),
            SteamError::Protocol(message) => {
                write!(f, "the Steam helper answered badly: {message}")
            }
            SteamError::Ended(message) => write!(
                f,
                "the Steam helper ended without an answer ({message}); Steam may have closed or stopped responding"
            ),
            SteamError::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for SteamError {}

/// A cooperative cancel flag shared with a running request. `Clone` shares the
/// same flag, so a caller can stop the helper from another thread.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// Asks a running request to stop.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether [`CancelToken::cancel`] has been called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Runs one request against Steam and returns its final event.
pub trait SteamRun: Send + Sync {
    fn run(
        &self,
        request: &SteamRequest,
        on_progress: &mut dyn FnMut(UploadStage, u64, u64),
    ) -> Result<SteamEvent, SteamError>;

    /// [`SteamRun::run`], stopped when `cancel` is set.
    fn run_cancellable(
        &self,
        request: &SteamRequest,
        on_progress: &mut dyn FnMut(UploadStage, u64, u64),
        _cancel: &CancelToken,
    ) -> Result<SteamEvent, SteamError> {
        self.run(request, on_progress)
    }
}

/// The file name of the Steam helper binary, without the platform suffix.
pub const HELPER_NAME: &str = "modfoundry-steam";

/// Runs the Steam helper binary, one process per request.
pub struct SteamHelper {
    path: PathBuf,
}

impl SteamHelper {
    /// Runs the helper at `path`.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The helper next to the running binary.
    pub fn beside_current_exe() -> Self {
        Self::new(
            crate::program::sibling_program(HELPER_NAME)
                .unwrap_or_else(|| PathBuf::from(HELPER_NAME)),
        )
    }

    fn spawn(&self) -> Result<Child, SteamError> {
        let mut command = Command::new(&self.path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            crate::program::prepend_library_dir(&mut command, dir);
        }
        // A terminal's Ctrl+C then reaches only us; the upload is stopped through `publish.cancel`.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command
            .spawn()
            .map_err(|e| SteamError::Spawn(e.to_string()))
    }
}

impl SteamRun for SteamHelper {
    fn run(
        &self,
        request: &SteamRequest,
        on_progress: &mut dyn FnMut(UploadStage, u64, u64),
    ) -> Result<SteamEvent, SteamError> {
        self.run_cancellable(request, on_progress, &CancelToken::default())
    }

    fn run_cancellable(
        &self,
        request: &SteamRequest,
        on_progress: &mut dyn FnMut(UploadStage, u64, u64),
        cancel: &CancelToken,
    ) -> Result<SteamEvent, SteamError> {
        if !crate::host_discovery_on() {
            return Err(SteamError::Spawn("Steam is off in this session".to_owned()));
        }
        if !self.path.is_file() {
            return Err(SteamError::HelperMissing(self.path.clone()));
        }
        if cancel.is_cancelled() {
            return Err(SteamError::Cancelled);
        }
        let mut child = self.spawn()?;
        let mut stderr = child.stderr.take().expect("stderr was piped");
        let stderr_reader = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let request_line = serde_json::to_string(request).expect("a request always serializes");
        if let Some(mut stdin) = child.stdin.take() {
            // A helper that exits before reading its request reports why on stdout or stderr.
            let _ = writeln!(stdin, "{request_line}");
        }
        let stdout = child.stdout.take().expect("stdout was piped");
        let child = Arc::new(Mutex::new(child));
        // Reading its stdout blocks, so a cancel is delivered by killing the helper from here.
        let (stop_watching, watching) = mpsc::channel::<()>();
        let cancelled = Arc::new(AtomicBool::new(false));
        let watcher = {
            let child = Arc::clone(&child);
            let cancel = cancel.clone();
            let fired = Arc::clone(&cancelled);
            std::thread::spawn(move || {
                while let Err(RecvTimeoutError::Timeout) =
                    watching.recv_timeout(Duration::from_millis(50))
                {
                    if !cancel.is_cancelled() {
                        continue;
                    }
                    let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
                    // A helper that answered and exited keeps its answer.
                    if !matches!(child.try_wait(), Ok(None)) {
                        return;
                    }
                    let _ = child.kill();
                    fired.store(true, Ordering::SeqCst);
                    return;
                }
            })
        };
        let answer = match read_answer(BufReader::new(stdout), on_progress) {
            Ok(answer) => answer,
            Err(error) => {
                let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
                let _ = child.kill();
                let _ = child.wait();
                drop(child);
                drop(stop_watching);
                let _ = watcher.join();
                return if cancelled.load(Ordering::SeqCst) {
                    Err(SteamError::Cancelled)
                } else {
                    Err(error)
                };
            }
        };
        // Polled, not waited: a blocking wait would hold the lock the watcher needs to kill.
        let status = wait_for_exit(&child);
        drop(stop_watching);
        let _ = watcher.join();
        let stderr_text = stderr_reader.join().unwrap_or_default();
        // A kill after the helper reported its answer keeps that answer: the work went through.
        if cancelled.load(Ordering::SeqCst) && answer.is_none() {
            return Err(SteamError::Cancelled);
        }
        let status = status.map_err(|e| SteamError::Protocol(e.to_string()))?;
        answer.ok_or_else(|| no_answer(status, &stderr_text))
    }
}

/// Waits for `child` to exit, leaving the lock free between polls so it can still be killed.
fn wait_for_exit(child: &Arc<Mutex<Child>>) -> std::io::Result<ExitStatus> {
    loop {
        let waited = child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_wait();
        match waited? {
            Some(status) => return Ok(status),
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// Reads event lines until stdout ends or the helper reports a failure. Lines that are not JSON
/// objects are skipped, since the Steam library prints its own text to stdout.
fn read_answer(
    mut stdout: impl BufRead,
    on_progress: &mut dyn FnMut(UploadStage, u64, u64),
) -> Result<Option<SteamEvent>, SteamError> {
    let mut answer = None;
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        let read = stdout
            .read_until(b'\n', &mut bytes)
            .map_err(|e| SteamError::Protocol(format!("reading its answer: {e}")))?;
        if read == 0 {
            return Ok(answer);
        }
        let text = String::from_utf8_lossy(&bytes);
        let line = text.trim();
        if !line.starts_with('{') {
            continue;
        }
        match serde_json::from_str::<SteamEvent>(line) {
            Ok(SteamEvent::Progress { stage, done, total }) => on_progress(stage, done, total),
            Ok(SteamEvent::Failed { message }) => return Err(SteamError::Steam(message)),
            Ok(event) => answer = Some(event),
            Err(e) => return Err(SteamError::Protocol(format!("{line}: {e}"))),
        }
    }
}

fn no_answer(status: ExitStatus, stderr: &str) -> SteamError {
    let detail = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| format!(": {line}"))
        .unwrap_or_default();
    SteamError::Ended(format!("exited with {status}{detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_serialize_to_the_helper_wire_format() {
        assert_eq!(
            serde_json::to_value(&SteamRequest::Delete { id: 9 }).unwrap(),
            serde_json::json!({"request": "delete", "id": 9})
        );
        assert_eq!(
            serde_json::to_value(&SteamRequest::Account).unwrap(),
            serde_json::json!({"request": "account"})
        );
    }

    #[test]
    fn every_visibility_round_trips_through_its_wire_word() {
        let cases = [
            (Visibility::Public, "public"),
            (Visibility::FriendsOnly, "friends_only"),
            (Visibility::Unlisted, "unlisted"),
            (Visibility::Private, "private"),
        ];
        for (visibility, word) in cases {
            let json = serde_json::to_string(&visibility).unwrap();
            assert_eq!(json, format!("\"{word}\""));
            assert_eq!(
                serde_json::from_str::<Visibility>(&json).unwrap(),
                visibility
            );
        }
        assert_eq!(Visibility::default(), Visibility::Private);
    }

    #[test]
    fn an_item_update_keeps_an_unlisted_visibility_on_the_wire() {
        let update = ItemUpdate {
            id: 7,
            content_dir: PathBuf::from("/mods/addons"),
            preview: None,
            title: None,
            description: None,
            visibility: Some(Visibility::Unlisted),
            tags: None,
            remove_keys: Vec::new(),
            kv_tags: Vec::new(),
            change_note: None,
        };
        let text = serde_json::to_string(&update).unwrap();
        assert!(text.contains(r#""visibility":"unlisted""#), "{text}");
        assert_eq!(serde_json::from_str::<ItemUpdate>(&text).unwrap(), update);
    }

    #[test]
    fn a_progress_line_parses_into_a_progress_event() {
        let line = r#"{"event":"progress","stage":"uploading_content","done":5,"total":10}"#;
        assert_eq!(
            serde_json::from_str::<SteamEvent>(line).unwrap(),
            SteamEvent::Progress {
                stage: UploadStage::UploadingContent,
                done: 5,
                total: 10,
            }
        );
    }

    #[test]
    fn steam_errors_display_the_helper_messages() {
        let cases = [
            (
                SteamError::HelperMissing(PathBuf::from("/bin/modfoundry-steam")),
                "the Steam helper is missing: /bin/modfoundry-steam; build it with cargo build -p modfoundry-steam",
            ),
            (
                SteamError::Spawn("Permission denied".to_owned()),
                "could not start the Steam helper: Permission denied",
            ),
            (
                SteamError::Steam("Steam is not running".to_owned()),
                "Steam is not running",
            ),
            (
                SteamError::Protocol("garbage".to_owned()),
                "the Steam helper answered badly: garbage",
            ),
        ];
        for (error, text) in cases {
            assert_eq!(error.to_string(), text);
        }
    }

    #[test]
    fn a_missing_helper_is_reported_by_its_path() {
        let _spawning = crate::spawn_lock();
        let path = PathBuf::from("/nonexistent/modfoundry-steam");
        let result = SteamHelper::new(path.clone()).run(&SteamRequest::Account, &mut |_, _, _| {});
        assert_eq!(result, Err(SteamError::HelperMissing(path)));
    }

    #[cfg(unix)]
    struct FakeHelper {
        dir: PathBuf,
        _spawning: std::sync::MutexGuard<'static, ()>,
    }

    #[cfg(unix)]
    impl FakeHelper {
        fn new(body: &str) -> Self {
            let spawning = crate::spawn_lock();
            use std::os::unix::fs::PermissionsExt;
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "mf-steam-helper-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(HELPER_NAME);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self {
                dir,
                _spawning: spawning,
            }
        }

        fn path(&self) -> PathBuf {
            self.dir.join(HELPER_NAME)
        }
    }

    #[cfg(unix)]
    impl Drop for FakeHelper {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[cfg(unix)]
    type RunOutcome = (Result<SteamEvent, SteamError>, Vec<(UploadStage, u64, u64)>);

    #[cfg(unix)]
    fn run_fake(body: &str) -> RunOutcome {
        let helper = FakeHelper::new(body);
        let mut progress = Vec::new();
        let result = SteamHelper::new(helper.path()).run(
            &SteamRequest::Account,
            &mut |stage, done, total| {
                progress.push((stage, done, total));
            },
        );
        (result, progress)
    }

    #[cfg(unix)]
    enum Want {
        Answer(SteamEvent),
        Steam(&'static str),
        ProtocolStarting(&'static str),
        EndedContaining(&'static str),
    }

    #[cfg(unix)]
    #[test]
    fn the_helper_answer_is_read_from_its_stdout() {
        let deleted = || Want::Answer(SteamEvent::Deleted { id: 9 });
        let cases = [
            (
                r#"echo '{"event":"progress","stage":"uploading_content","done":5,"total":10}'
echo '{"event":"submitted","id":7,"needs_legal_agreement":false}'"#,
                Want::Answer(SteamEvent::Submitted {
                    id: 7,
                    needs_legal_agreement: false,
                }),
                vec![(UploadStage::UploadingContent, 5, 10)],
            ),
            (
                r#"echo '{"event":"failed","message":"Steam is not running"}'"#,
                Want::Steam("Steam is not running"),
                vec![],
            ),
            (
                r#"echo '{"event":"failed","message":"disk full"}'
echo '{"event":"submitted","id":7,"needs_legal_agreement":false}'"#,
                Want::Steam("disk full"),
                vec![],
            ),
            (
                "echo oops >&2\nexit 3",
                Want::EndedContaining("oops"),
                vec![],
            ),
            (
                r#"echo '{"x":1}'"#,
                Want::ProtocolStarting(r#"{"x":1}: "#),
                vec![],
            ),
            (
                r#"echo 'Setting breakpad minidump AppID = 221100'
echo '{"event":"deleted","id":9}'"#,
                deleted(),
                vec![],
            ),
            (
                r#"printf '\377\n'
echo '{"event":"deleted","id":9}'"#,
                deleted(),
                vec![],
            ),
            (
                "echo ''\necho '{\"event\":\"deleted\",\"id\":9}'\necho ''",
                deleted(),
                vec![],
            ),
        ];
        for (body, want, progress_seen) in cases {
            let (result, progress) = run_fake(&format!("cat > /dev/null\n{body}"));
            let matches = match (&want, &result) {
                (Want::Answer(event), Ok(got)) => event == got,
                (Want::Steam(text), Err(SteamError::Steam(got))) => text == got,
                (Want::ProtocolStarting(text), Err(SteamError::Protocol(got))) => {
                    got.starts_with(text)
                }
                (Want::EndedContaining(text), Err(SteamError::Ended(got))) => got.contains(text),
                _ => false,
            };
            assert!(matches, "{body}: {result:?}");
            assert_eq!(progress, progress_seen, "{body}");
        }
    }

    #[test]
    fn a_helper_that_ends_without_answering_points_at_steam() {
        let text = SteamError::Ended("exited with exit status: 1 and no answer".into()).to_string();
        assert!(
            text.contains("Steam may have closed or stopped responding"),
            "{text}"
        );
        assert!(!text.contains("answered badly"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn the_request_reaches_the_helper_on_stdin() {
        let helper = FakeHelper::new(
            r#"cat > "$(dirname "$0")/request.json"
echo '{"event":"deleted","id":9}'"#,
        );
        let result =
            SteamHelper::new(helper.path()).run(&SteamRequest::Delete { id: 9 }, &mut |_, _, _| {});
        let received = std::fs::read_to_string(helper.dir.join("request.json")).unwrap();
        assert_eq!(
            serde_json::from_str::<SteamRequest>(&received).unwrap(),
            SteamRequest::Delete { id: 9 }
        );
        assert_eq!(result, Ok(SteamEvent::Deleted { id: 9 }));
    }

    #[cfg(all(unix, target_os = "linux"))]
    const STALLED_HELPER: &str = r#"cat > /dev/null
echo $$ > "$(dirname "$0")/pid"
echo '{"event":"progress","stage":"uploading_content","done":1,"total":10}'
exec sleep 30"#;

    /// Runs a stalled helper, cancelling on its first progress line.
    #[cfg(all(unix, target_os = "linux"))]
    fn cancel_on_first_progress(helper: &FakeHelper) -> (RunOutcome, Duration) {
        let token = CancelToken::default();
        let mut progress = Vec::new();
        let started = std::time::Instant::now();
        let result = SteamHelper::new(helper.path()).run_cancellable(
            &SteamRequest::Account,
            &mut |stage, done, total| {
                progress.push((stage, done, total));
                token.cancel();
            },
            &token,
        );
        ((result, progress), started.elapsed())
    }

    #[cfg(unix)]
    #[test]
    fn a_token_set_before_the_run_starts_no_helper() {
        let helper = FakeHelper::new(r#"echo started > "$(dirname "$0")/started""#);
        let token = CancelToken::default();
        token.cancel();
        let result = SteamHelper::new(helper.path()).run_cancellable(
            &SteamRequest::Account,
            &mut |_, _, _| {},
            &token,
        );
        assert_eq!(result, Err(SteamError::Cancelled));
        assert!(!helper.dir.join("started").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_token_that_is_never_set_changes_nothing() {
        let helper = FakeHelper::new(
            r#"cat > /dev/null
echo '{"event":"progress","stage":"uploading_content","done":5,"total":10}'
echo '{"event":"submitted","id":7,"needs_legal_agreement":false}'"#,
        );
        let mut progress = Vec::new();
        let result = SteamHelper::new(helper.path()).run_cancellable(
            &SteamRequest::Account,
            &mut |stage, done, total| progress.push((stage, done, total)),
            &CancelToken::default(),
        );
        assert_eq!(
            result,
            Ok(SteamEvent::Submitted {
                id: 7,
                needs_legal_agreement: false,
            })
        );
        assert_eq!(progress, [(UploadStage::UploadingContent, 5, 10)]);
        assert_eq!(SteamError::Cancelled.to_string(), "cancelled");
    }

    #[cfg(all(unix, target_os = "linux"))]
    #[test]
    fn cancelling_stops_a_stalled_helper_and_returns_cancelled() {
        let helper = FakeHelper::new(STALLED_HELPER);
        let ((result, progress), elapsed) = cancel_on_first_progress(&helper);
        let pid: u32 = std::fs::read_to_string(helper.dir.join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        assert_eq!(result, Err(SteamError::Cancelled));
        assert_eq!(progress, [(UploadStage::UploadingContent, 1, 10)]);
        assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "the helper {pid} is still running"
        );
    }

    #[cfg(all(unix, target_os = "linux"))]
    #[test]
    fn cancelling_kills_only_the_helper_it_started() {
        struct Bystander(Child);
        impl Drop for Bystander {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut bystander = {
            let _lock = crate::spawn_lock();
            Bystander(
                Command::new("sleep")
                    .arg("29")
                    .spawn()
                    .expect("a sleep starts"),
            )
        };
        let helper = FakeHelper::new(STALLED_HELPER);
        let ((result, _), _) = cancel_on_first_progress(&helper);

        assert_eq!(result, Err(SteamError::Cancelled));
        // A kill lands a moment later, so give a wrong kill time to show.
        let deadline = std::time::Instant::now() + Duration::from_millis(200);
        while std::time::Instant::now() < deadline {
            let state = bystander.0.try_wait().expect("the bystander is readable");
            assert!(state.is_none(), "the bystander was killed");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_that_answers_and_then_hangs_keeps_its_answer_when_cancelled() {
        let helper = FakeHelper::new(
            r#"cat > /dev/null
echo '{"event":"submitted","id":7,"needs_legal_agreement":false}'
exec sleep 30"#,
        );
        let token = CancelToken::default();
        let later = token.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            later.cancel();
        });
        let started = std::time::Instant::now();
        let result = SteamHelper::new(helper.path()).run_cancellable(
            &SteamRequest::Account,
            &mut |_, _, _| {},
            &token,
        );
        canceller.join().unwrap();

        assert_eq!(
            result,
            Ok(SteamEvent::Submitted {
                id: 7,
                needs_legal_agreement: false,
            })
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(all(unix, target_os = "linux"))]
    #[test]
    fn the_helper_folder_leads_its_library_path() {
        let helper = FakeHelper::new(
            r#"cat > /dev/null
echo "{\"event\":\"failed\",\"message\":\"$LD_LIBRARY_PATH\"}""#,
        );
        let dir = helper.dir.display().to_string();
        let expected = match std::env::var("LD_LIBRARY_PATH") {
            Ok(existing) => format!("{dir}:{existing}"),
            Err(_) => dir,
        };
        let result = SteamHelper::new(helper.path()).run(&SteamRequest::Account, &mut |_, _, _| {});
        assert_eq!(result, Err(SteamError::Steam(expected)));
    }

    #[cfg(all(unix, target_os = "linux"))]
    #[test]
    fn the_helper_runs_in_a_process_group_of_its_own() {
        let helper = FakeHelper::new(
            r#"cat > /dev/null
ps -o pgid= -p $$ > "$(dirname "$0")/pgid"
echo '{"event":"deleted","id":9}'"#,
        );
        let result = SteamHelper::new(helper.path()).run(&SteamRequest::Account, &mut |_, _, _| {});
        let own_group = process_group(std::process::id());
        let helper_group = std::fs::read_to_string(helper.dir.join("pgid")).unwrap();
        let helper_group = helper_group.trim();

        assert_eq!(result, Ok(SteamEvent::Deleted { id: 9 }));
        assert!(!helper_group.is_empty(), "the helper group was not read");
        assert_ne!(
            helper_group, own_group,
            "the helper was left in the caller's process group"
        );
    }

    #[cfg(all(unix, target_os = "linux"))]
    fn process_group(pid: u32) -> String {
        let output = Command::new("ps")
            .args(["-o", "pgid=", "-p", &pid.to_string()])
            .output()
            .expect("ps runs");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
}
