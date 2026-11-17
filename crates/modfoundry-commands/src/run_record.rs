use crate::diag_run::{DiagOutcome, MARKER_POLL, SERVER_READY_MARKER, SERVER_READY_TIMEOUT_SECS};
use crate::run_ops::{ClientRunOutcome, RunControl, RunOutcome};
use modfoundry_diag::{log_states, wait_for_log_marker, LogStates, MarkerWait, StopSignal};
use modfoundry_store::run::NewRun;
use modfoundry_store::{Store, StoreError};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TOUCH_EVERY: Duration = Duration::from_secs(3);

/// A running row not touched for this long has lost its launcher.
pub const STALE_AFTER_SECS: i64 = modfoundry_store::run::STALE_AFTER_SECS;

/// A launch's row in the agent database, kept current until the launch ends.
pub struct Recorder {
    store: Arc<Mutex<Store>>,
    id: String,
    started_millis: i64,
    ticker: Option<(Sender<()>, JoinHandle<()>)>,
}

impl Recorder {
    /// Record `run` as running and keep it current until the recorder ends.
    pub fn start(db: &Path, run: NewRun, stop: StopSignal) -> anyhow::Result<Self> {
        Self::start_every(db, run, stop, TOUCH_EVERY)
    }

    fn start_every(
        db: &Path,
        run: NewRun,
        stop: StopSignal,
        every: Duration,
    ) -> anyhow::Result<Self> {
        let store = Store::open(db)?;
        store.start_run(&run)?;
        let store = Arc::new(Mutex::new(store));
        let ticker = spawn_ticker(
            Arc::clone(&store),
            run.id.clone(),
            run.at_millis,
            stop,
            every,
        );
        Ok(Self {
            store,
            id: run.id,
            started_millis: run.at_millis,
            ticker: Some(ticker),
        })
    }

    /// End the row with `outcome` and, when it produced one, its result.
    pub fn finish(mut self, outcome: &str, result: Option<serde_json::Value>) {
        self.end(outcome, result.as_ref());
    }

    fn end(&mut self, outcome: &str, result: Option<&serde_json::Value>) {
        let Some((done, ticker)) = self.ticker.take() else {
            return;
        };
        drop(done);
        let _ = ticker.join();
        let store = lock_store(&self.store);
        if !owns_row(&store, &self.id, self.started_millis) {
            return;
        }
        if let Some(result) = result {
            let _ = store.set_run_result(&self.id, result);
        }
        let _ = store.end_run(&self.id, now_unix(), outcome);
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.end("failed", None);
    }
}

fn lock_store(store: &Mutex<Store>) -> MutexGuard<'_, Store> {
    store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether the row under `id` is still the one this launcher started: a launcher
/// that stalled past the staleness window may have had it replaced by a new run.
fn owns_row(store: &Store, id: &str, started_millis: i64) -> bool {
    matches!(store.run(id), Ok(Some(row)) if row.started_millis == started_millis)
}

fn spawn_ticker(
    store: Arc<Mutex<Store>>,
    id: String,
    started_millis: i64,
    stop: StopSignal,
    every: Duration,
) -> (Sender<()>, JoinHandle<()>) {
    let (done, finished) = mpsc::channel();
    let ticker = std::thread::spawn(move || {
        while let Err(RecvTimeoutError::Timeout) = finished.recv_timeout(every) {
            let requested = {
                let store = lock_store(&store);
                if !owns_row(&store, &id, started_millis) {
                    return;
                }
                let _ = store.touch_run(&id, now_unix());
                store.run_stop_requested(&id).unwrap_or(false)
            };
            if requested {
                stop.stop();
            }
        }
    });
    (done, ticker)
}

pub(crate) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

pub(crate) struct Launch {
    pub db: Option<PathBuf>,
    pub actor: String,
    pub command: &'static str,
    pub run_id: Option<String>,
    pub source_root: Option<PathBuf>,
    pub log_dirs: Vec<PathBuf>,
}

pub(crate) trait Ending {
    fn ending(&self) -> &'static str;
}

impl Ending for RunOutcome {
    fn ending(&self) -> &'static str {
        report_ending(self.report.stopped, self.report.timed_out)
    }
}

impl Ending for DiagOutcome {
    fn ending(&self) -> &'static str {
        if self.ready_timed_out {
            return report_ending(false, true);
        }
        report_ending(self.server.stopped, self.server.timed_out)
    }
}

impl Ending for ClientRunOutcome {
    fn ending(&self) -> &'static str {
        report_ending(self.report.stopped, self.report.timed_out)
    }
}

fn report_ending(stopped: bool, timed_out: bool) -> &'static str {
    match (stopped, timed_out) {
        (true, _) => "stopped",
        (false, true) => "timed out",
        (false, false) => "exited",
    }
}

/// Runs `body` under its stop signal, recording the launch when there is a database.
pub(crate) fn run_launch<T: Ending + serde::Serialize>(
    control: &RunControl,
    launch: Launch,
    body: impl FnOnce(&StopSignal) -> anyhow::Result<T>,
) -> (anyhow::Result<T>, Option<String>) {
    let Some(db) = launch.db.as_deref() else {
        return (control.scoped(launch.run_id.as_deref(), body), None);
    };
    let id = launch
        .run_id
        .clone()
        .unwrap_or_else(|| generated_id(&launch.actor));
    let result = control.scoped(Some(&id), |signal| {
        let recorder = match Recorder::start(db, new_run(&launch, &id), signal.clone()) {
            Ok(recorder) => Some(recorder),
            Err(e) if is_live_refusal(&e) => return Err(e),
            Err(_) => None,
        };
        let result = body(signal);
        if let Some(recorder) = recorder {
            let stored = result
                .as_ref()
                .ok()
                .and_then(|outcome| serde_json::to_value(outcome).ok());
            recorder.finish(&ending(&result), stored);
        }
        result
    });
    (result, Some(id))
}

fn is_live_refusal(e: &anyhow::Error) -> bool {
    matches!(e.downcast_ref::<StoreError>(), Some(StoreError::RunLive(_)))
}

/// A server launch running on its own thread, under the id it is recorded as.
pub(crate) struct Detached<T> {
    pub run_id: String,
    /// Whether the server's log showed the ready marker.
    pub ready: bool,
    run: JoinHandle<(anyhow::Result<T>, Option<String>)>,
}

impl<T> Detached<T> {
    /// The run's outcome, or `None` while its thread is still going.
    pub(crate) fn outcome(self) -> Option<anyhow::Result<T>> {
        if !self.run.is_finished() {
            return None;
        }
        Some(match self.run.join() {
            Ok((result, _)) => result,
            Err(_) => Err(anyhow::anyhow!("the launch thread panicked")),
        })
    }
}

/// Starts a server launch on its own thread, returning once its log shows the ready marker,
/// the run has ended, or the ready timeout passes.
pub(crate) fn spawn_server<T: Ending + serde::Serialize + Send + 'static>(
    control: &RunControl,
    mut launch: Launch,
    body: impl FnOnce(&StopSignal) -> anyhow::Result<T> + Send + 'static,
) -> Detached<T> {
    let run_id = launch
        .run_id
        .get_or_insert_with(|| generated_id(&launch.actor))
        .clone();
    let watched = launch
        .log_dirs
        .first()
        .map(|dir| (dir.clone(), log_states(dir)));
    let control = control.clone();
    let run = std::thread::spawn(move || {
        crate::run_ops::mark_detached();
        run_launch(&control, launch, body)
    });
    // Past the run's own wait, so a server that never gets ready reports its real ending.
    let patience = Duration::from_secs(SERVER_READY_TIMEOUT_SECS + 5);
    let ready = watched.is_some_and(|(dir, earlier)| await_ready(&run, &dir, &earlier, patience));
    Detached { run_id, ready, run }
}

fn await_ready<T>(
    run: &JoinHandle<T>,
    dir: &Path,
    earlier: &LogStates,
    patience: Duration,
) -> bool {
    let give_up = StopSignal::new();
    let wait = wait_for_log_marker(
        dir,
        earlier,
        SERVER_READY_MARKER,
        patience,
        &give_up,
        || {
            if run.is_finished() {
                give_up.stop();
            }
            std::thread::sleep(MARKER_POLL);
        },
    );
    matches!(wait, MarkerWait::Found)
}

fn new_run(launch: &Launch, id: &str) -> NewRun {
    NewRun {
        id: id.to_owned(),
        actor: launch.actor.clone(),
        command: launch.command.to_owned(),
        source_root: launch.source_root.clone(),
        profiles_dir: None,
        log_dirs: launch.log_dirs.clone(),
        at_millis: now_millis(),
    }
}

fn ending<T: Ending>(result: &anyhow::Result<T>) -> String {
    match result {
        Ok(outcome) => outcome.ending().to_owned(),
        Err(e) => format!("failed: {e:#}"),
    }
}

fn generated_id(actor: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("{actor}-{}-{millis}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::{Ending, Recorder};
    use modfoundry_diag::StopSignal;
    use modfoundry_store::run::{NewRun, RunState};
    use modfoundry_store::Store;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mf-run-record-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn new_run(id: &str) -> NewRun {
        NewRun {
            id: id.into(),
            actor: "cli".into(),
            command: "diag.run".into(),
            source_root: None,
            profiles_dir: None,
            log_dirs: Vec::new(),
            at_millis: 1,
        }
    }

    #[test]
    fn a_launch_starts_its_row_at_the_current_millisecond() {
        let launch = super::Launch {
            db: None,
            actor: "cli".to_owned(),
            command: "diag.run",
            run_id: None,
            source_root: None,
            log_dirs: Vec::new(),
        };
        let before = super::now_millis();
        let started = super::new_run(&launch, "r4");
        let after = super::now_millis();

        assert!((before..=after).contains(&started.at_millis));
    }

    fn wait_until(db: &Path, mut done: impl FnMut(&Store) -> bool) {
        let store = Store::open(db).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !done(&store) {
            assert!(Instant::now() < deadline, "condition not reached in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_ready_timeout_ends_as_timed_out_not_as_a_stop() {
        let outcome: crate::diag_run::DiagOutcome = serde_json::from_value(serde_json::json!({
            "profiles_dir": "/p",
            "server": {"exit_code": null, "logs": [], "timed_out": false, "stopped": true},
            "client": null,
            "errors": [],
            "ready_timed_out": true,
        }))
        .unwrap();

        assert_eq!(outcome.ending(), "timed out");
    }

    #[test]
    fn a_recorded_run_is_touched_while_live_and_a_stop_request_reaches_its_signal() {
        let dir = tmp("live");
        let db = dir.join("agents.db");
        let stop = StopSignal::new();
        let recorder =
            Recorder::start_every(&db, new_run("r1"), stop.clone(), Duration::from_millis(5))
                .unwrap();
        let store = Store::open(&db).unwrap();
        wait_until(&db, |store| {
            store
                .run("r1")
                .unwrap()
                .is_some_and(|row| row.seen_unix > 1)
        });
        assert_eq!(store.run("r1").unwrap().unwrap().state, RunState::Running);

        assert!(store.request_run_stop("r1").unwrap());
        wait_until(&db, |_| stop.is_stopped());

        recorder.finish("stopped", None);
        let row = store.run("r1").unwrap().unwrap();
        assert_eq!(row.state, RunState::Ended);
        assert_eq!(row.outcome.as_deref(), Some("stopped"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_launcher_whose_row_was_taken_over_leaves_the_new_run_alone() {
        let dir = tmp("taken-over");
        let db = dir.join("agents.db");
        let recorder = Recorder::start_every(
            &db,
            new_run("r3"),
            StopSignal::new(),
            Duration::from_millis(5),
        )
        .unwrap();
        // A later launch finds the row stale and replaces it.
        let store = Store::open(&db).unwrap();
        let later = NewRun {
            at_millis: (super::now_unix() + 60) * 1000,
            ..new_run("r3")
        };
        store.start_run(&later).unwrap();

        recorder.finish("exited", Some(serde_json::json!({"old": true})));

        let row = store.run("r3").unwrap().unwrap();
        assert_eq!(row.started_millis, later.at_millis);
        assert_eq!(row.state, RunState::Running);
        assert_eq!(store.run_result("r3").unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_dropped_recorder_ends_its_row_as_failed() {
        let dir = tmp("dropped");
        let db = dir.join("agents.db");
        let recorder = Recorder::start_every(
            &db,
            new_run("r2"),
            StopSignal::new(),
            Duration::from_millis(5),
        )
        .unwrap();
        drop(recorder);
        let row = Store::open(&db).unwrap().run("r2").unwrap().unwrap();
        assert_eq!(row.state, RunState::Ended);
        assert_eq!(row.outcome.as_deref(), Some("failed"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod ready_tests {
    use super::*;

    #[test]
    fn await_ready_says_whether_the_marker_appeared() {
        let dir = std::env::temp_dir().join(format!("mf-await-ready-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = std::thread::spawn(|| std::thread::sleep(Duration::from_secs(3)));
        let short = Duration::from_millis(50);
        let earlier = log_states(&dir);
        assert!(!await_ready(&run, &dir, &earlier, short));
        let log = dir.join("server.RPT");
        std::fs::write(&log, format!("{SERVER_READY_MARKER}\n")).unwrap();
        assert!(await_ready(&run, &dir, &earlier, Duration::from_secs(2)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
