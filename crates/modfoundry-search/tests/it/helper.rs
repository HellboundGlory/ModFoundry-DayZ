use std::fs;
use std::io::pipe;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use modfoundry_search::embed::{Embed, FakeEmbed};
use modfoundry_search::helper::{check_helper, serve, HelperEmbed, Limits};

const TEXTS: [&str; 3] = ["hide the toast", "héllo wörld, ünïcode", "apple pie"];

fn connect(
    embed: impl Embed + Send + 'static,
) -> (HelperEmbed, thread::JoinHandle<Result<(), String>>) {
    let (helper_reader, helper_writer) = pipe().unwrap();
    let (request_reader, request_writer) = pipe().unwrap();
    let server = thread::spawn(move || serve(&embed, request_reader, helper_writer));
    let helper = HelperEmbed::over(Box::new(helper_reader), Box::new(request_writer)).unwrap();
    (helper, server)
}

#[test]
fn helper_vectors_match_the_embedder_it_serves() {
    let (helper, server) = connect(FakeEmbed { dims: 8 });
    assert_eq!(helper.dims(), 8);
    let vectors = helper.embed(&TEXTS).unwrap();
    assert_eq!(vectors, FakeEmbed { dims: 8 }.embed(&TEXTS).unwrap());
    drop(helper);
    server.join().unwrap().unwrap();
}

struct Broken;

impl Embed for Broken {
    fn dims(&self) -> usize {
        8
    }

    fn embed(&self, _: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        Err("device lost".to_owned())
    }
}

#[test]
fn a_failed_embed_is_reported_and_serving_goes_on() {
    let (helper, server) = connect(Broken);
    assert_eq!(helper.embed(&TEXTS), Err("device lost".to_owned()));
    assert_eq!(helper.embed(&TEXTS[..1]), Err("device lost".to_owned()));
    drop(helper);
    server.join().unwrap().unwrap();
}

// A sibling test forking while a script is still open for writing makes its exec fail with "text file busy".
static SPAWNS: Mutex<()> = Mutex::new(());

fn spawn_lock() -> MutexGuard<'static, ()> {
    SPAWNS.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(unix)]
fn script(name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("mf-helper-{name}-{}", std::process::id()));
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn a_helper_that_cannot_load_its_libraries_is_reported() {
    let _spawn = spawn_lock();
    let program = script(
        "libs",
        "#!/bin/sh\necho 'error while loading shared libraries: libcublasLt.so.13: cannot open shared object file' >&2\nexit 127\n",
    );
    let err = HelperEmbed::spawn(&program, Path::new("model"), None)
        .err()
        .expect("a helper that exits at once cannot start");
    let _ = fs::remove_file(&program);
    assert!(err.contains("libcublasLt.so.13"), "{err}");
}

#[test]
fn a_missing_helper_is_named_in_the_error() {
    let _spawn = spawn_lock();
    let program = std::env::temp_dir().join("mf-helper-not-installed");
    let err = HelperEmbed::spawn(&program, Path::new("model"), None)
        .err()
        .expect("a missing program cannot start");
    assert!(err.contains(&program.display().to_string()), "{err}");
}

fn ready_then(dims: u32, rest: &[u8]) -> Box<dyn std::io::Read + Send> {
    let mut bytes = b"MFE1".to_vec();
    bytes.extend_from_slice(&dims.to_le_bytes());
    bytes.extend_from_slice(rest);
    Box::new(std::io::Cursor::new(bytes))
}

#[test]
fn a_reply_with_an_unknown_status_is_an_error() {
    let helper =
        HelperEmbed::over(ready_then(2, b"[{\"x\":1}]"), Box::new(std::io::sink())).unwrap();
    let err = helper.embed(&TEXTS[..1]).unwrap_err();
    assert!(err.contains("status"), "{err}");
}

#[test]
fn a_helper_announcing_zero_dimensions_is_refused() {
    let result = HelperEmbed::over(ready_then(0, &[]), Box::new(std::io::sink()));
    assert!(result.is_err());
}

#[cfg(unix)]
#[test]
fn a_program_that_is_not_a_helper_is_stopped_rather_than_waited_for() {
    let _spawn = spawn_lock();
    let program = script(
        "wrong",
        "#!/bin/sh\nprintf 'Usage: something else\\n'\nexec sleep 30\n",
    );
    let started = Instant::now();
    let result = HelperEmbed::spawn(&program, Path::new("model"), None);
    let _ = fs::remove_file(&program);
    let err = result
        .err()
        .expect("a program that prints no frame is no helper");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(err.contains("not a GPU helper"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_helper_that_never_sends_the_ready_frame_is_stopped() {
    let _spawn = spawn_lock();
    let program = script("silent", "#!/bin/sh\nexec sleep 30\n");
    let started = Instant::now();
    let err = HelperEmbed::spawn_with(
        &program,
        Path::new("model"),
        None,
        Limits {
            ready: Duration::from_millis(300),
            ..Limits::default()
        },
    )
    .err()
    .expect("a helper that never answers cannot start");
    let _ = fs::remove_file(&program);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(err.contains("did not answer within"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_reply_that_never_comes_stops_the_helper() {
    let _spawn = spawn_lock();
    let program = script(
        "mute",
        "#!/bin/sh\nprintf 'MFE1\\002\\000\\000\\000'\nexec sleep 30\n",
    );
    let helper = HelperEmbed::spawn_with(
        &program,
        Path::new("model"),
        None,
        Limits {
            reply: Duration::from_millis(300),
            ..Limits::default()
        },
    )
    .expect("the ready frame arrives");
    let _ = fs::remove_file(&program);
    let started = Instant::now();
    let err = helper.embed(&TEXTS[..1]).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10), "{err}");
    assert!(err.contains("did not answer within"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_helper_that_stops_reading_cannot_block_a_large_request() {
    let _spawn = spawn_lock();
    let program = script(
        "deaf",
        "#!/bin/sh\nprintf 'MFE1\\002\\000\\000\\000'\nexec sleep 30\n",
    );
    let helper = HelperEmbed::spawn_with(
        &program,
        Path::new("model"),
        None,
        Limits {
            reply: Duration::from_millis(300),
            ..Limits::default()
        },
    )
    .expect("the ready frame arrives");
    let _ = fs::remove_file(&program);
    let big = "x".repeat(1024 * 1024);
    let started = Instant::now();
    let err = helper.embed(&[big.as_str()]).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10), "{err}");
    assert!(err.contains("did not answer within"), "{err}");
}

#[cfg(unix)]
#[test]
fn dropping_a_stuck_helper_waits_only_the_stop_limit() {
    let _spawn = spawn_lock();
    let pid_file = std::env::temp_dir().join(format!("mf-helper-pid-{}", std::process::id()));
    let program = script(
        "stuck",
        &format!(
            "#!/bin/sh\necho $$ > '{}'\nprintf 'MFE1\\002\\000\\000\\000'\nexec sleep 30\n",
            pid_file.display()
        ),
    );
    let helper = HelperEmbed::spawn_with(
        &program,
        Path::new("model"),
        None,
        Limits {
            stop: Duration::from_millis(300),
            ..Limits::default()
        },
    )
    .expect("the ready frame arrives");
    let _ = fs::remove_file(&program);
    let pid = fs::read_to_string(&pid_file).unwrap();
    let started = Instant::now();
    drop(helper);
    let _ = fs::remove_file(&pid_file);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        !std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success(),
        "the helper is gone"
    );
}

#[cfg(unix)]
#[test]
fn note_lines_do_not_hide_the_real_error() {
    let _spawn = spawn_lock();
    let program = script(
        "noted",
        "#!/bin/sh\necho 'error: libfoo.so missing' >&2\necho 'note: run with RUST_BACKTRACE=1' >&2\nexit 1\n",
    );
    let err = HelperEmbed::spawn(&program, Path::new("model"), None)
        .err()
        .expect("a helper that exits at once cannot start");
    let _ = fs::remove_file(&program);
    assert!(err.contains("libfoo.so"), "{err}");
    assert!(!err.contains("RUST_BACKTRACE"), "{err}");
}

struct InterruptedOnce<R> {
    inner: R,
    pending: bool,
}

impl<R: std::io::Read> std::io::Read for InterruptedOnce<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.pending {
            self.pending = true;
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        self.pending = false;
        self.inner.read(buf)
    }
}

#[test]
fn serve_goes_on_after_an_interrupted_read() {
    let text = TEXTS[0];
    let mut request = 1_u32.to_le_bytes().to_vec();
    request.extend_from_slice(&(text.len() as u32).to_le_bytes());
    request.extend_from_slice(text.as_bytes());
    let input = InterruptedOnce {
        inner: std::io::Cursor::new(request),
        pending: false,
    };
    let mut output = Vec::new();
    serve(&FakeEmbed { dims: 8 }, input, &mut output).expect("the request is answered");
    assert!(output.len() > 8, "the reply follows the ready frame");
}

struct FlushInterruptedOnce {
    written: Vec<u8>,
    pending: bool,
}

impl std::io::Write for FlushInterruptedOnce {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.pending = !self.pending;
        if self.pending {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        Ok(())
    }
}

#[test]
fn serve_goes_on_after_an_interrupted_flush() {
    let text = TEXTS[0];
    let mut request = 1_u32.to_le_bytes().to_vec();
    request.extend_from_slice(&(text.len() as u32).to_le_bytes());
    request.extend_from_slice(text.as_bytes());
    let mut output = FlushInterruptedOnce {
        written: Vec::new(),
        pending: false,
    };
    serve(&FakeEmbed { dims: 8 }, request.as_slice(), &mut output)
        .expect("the request is answered");
    assert!(
        output.written.len() > 8,
        "the reply follows the ready frame"
    );
}

#[cfg(unix)]
#[test]
fn check_helper_reports_exit_status_and_reason() {
    let _spawn = spawn_lock();
    let cases = [
        (
            "ok",
            "#!/bin/sh\n[ \"$1\" = --check ] || exit 3\nexit 0\n",
            None,
        ),
        (
            "nodevice",
            "#!/bin/sh\n[ \"$1\" = --check ] || exit 3\necho 'error: no CUDA device' >&2\necho 'note: run with RUST_BACKTRACE=1' >&2\nexit 1\n",
            Some("no CUDA device"),
        ),
    ];
    for (name, body, expected) in cases {
        let program = script(name, body);
        let result = check_helper(&program, None, Duration::from_secs(10));
        let _ = fs::remove_file(&program);
        match expected {
            Some(text) => {
                let err = result.expect_err("the check fails");
                assert!(err.contains(text), "{err}");
            }
            None => assert!(result.is_ok(), "{result:?}"),
        }
    }

    let missing = std::env::temp_dir().join("mf-helper-check-not-installed");
    let err = check_helper(&missing, None, Duration::from_secs(10)).expect_err("no program");
    assert!(err.contains(&missing.display().to_string()), "{err}");
}

#[cfg(unix)]
#[test]
fn check_helper_kills_a_program_that_hangs() {
    let _spawn = spawn_lock();
    let program = script("hang", "#!/bin/sh\nexec sleep 30\n");
    let err = check_helper(&program, None, Duration::from_millis(300))
        .expect_err("a hanging check fails");
    let _ = fs::remove_file(&program);
    assert!(err.contains("did not finish within"), "{err}");
}
