//! The wire between `mf` and the GPU embedding helper: little-endian integers and floats in frames over a pipe.

use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use modfoundry_platform::program::prepend_library_dir;

use crate::embed::Embed;

const MAGIC: &[u8; 4] = b"MFE1";

/// How long a helper gets to send its ready frame, to answer a request and to stop.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub ready: Duration,
    pub reply: Duration,
    pub stop: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            ready: Duration::from_secs(60),
            reply: Duration::from_secs(60),
            stop: Duration::from_secs(5),
        }
    }
}

/// Writes the ready frame, then answers embedding requests from `input` until it ends.
pub fn serve(embed: &dyn Embed, input: impl Read, output: impl Write) -> Result<(), String> {
    serve_frames(embed, input, output).map_err(|err| err.to_string())
}

fn serve_frames(embed: &dyn Embed, mut input: impl Read, mut output: impl Write) -> io::Result<()> {
    output.write_all(MAGIC)?;
    output.write_all(&(embed.dims() as u32).to_le_bytes())?;
    flush(&mut output)?;
    while let Some(texts) = read_request(&mut input)? {
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        output.write_all(&reply_frame(&embed.embed(&texts)))?;
        flush(&mut output)?;
    }
    Ok(())
}

fn flush(output: &mut impl Write) -> io::Result<()> {
    loop {
        match output.flush() {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            other => return other,
        }
    }
}

fn read_request(input: &mut impl Read) -> io::Result<Option<Vec<String>>> {
    let Some(count) = read_count(input)? else {
        return Ok(None);
    };
    (0..count)
        .map(|_| read_text(input))
        .collect::<io::Result<_>>()
        .map(Some)
}

fn read_count(input: &mut impl Read) -> io::Result<Option<u32>> {
    let mut bytes = [0; 4];
    loop {
        match input.read(&mut bytes[..1]) {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(err) => return Err(err),
        }
    }
    input.read_exact(&mut bytes[1..])?;
    Ok(Some(u32::from_le_bytes(bytes)))
}

fn read_u32(input: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_text(input: &mut impl Read) -> io::Result<String> {
    let len = u64::from(read_u32(input)?);
    // Grows with what arrives, so a broken length cannot claim gigabytes up front.
    let mut bytes = Vec::new();
    if input.take(len).read_to_end(&mut bytes)? as u64 != len {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    String::from_utf8(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn request_frame(texts: &[&str]) -> Vec<u8> {
    let mut frame = (texts.len() as u32).to_le_bytes().to_vec();
    for text in texts {
        frame.extend_from_slice(&(text.len() as u32).to_le_bytes());
        frame.extend_from_slice(text.as_bytes());
    }
    frame
}

fn reply_frame(reply: &Result<Vec<Vec<f32>>, String>) -> Vec<u8> {
    let mut frame = Vec::new();
    match reply {
        Ok(vectors) => {
            frame.push(0);
            frame.extend_from_slice(&(vectors.len() as u32).to_le_bytes());
            for value in vectors.iter().flatten() {
                frame.extend_from_slice(&value.to_le_bytes());
            }
        }
        Err(message) => {
            frame.push(1);
            frame.extend_from_slice(&(message.len() as u32).to_le_bytes());
            frame.extend_from_slice(message.as_bytes());
        }
    }
    frame
}

fn read_ready(input: &mut impl Read) -> io::Result<usize> {
    let mut magic = [0; 4];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a GPU helper",
        ));
    }
    match read_u32(input)? {
        0 => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GPU helper announced no dimensions",
        )),
        dims => Ok(dims as usize),
    }
}

fn read_reply(input: &mut impl Read, dims: usize) -> io::Result<Result<Vec<Vec<f32>>, String>> {
    let mut status = [0];
    input.read_exact(&mut status)?;
    match status[0] {
        0 => {
            let rows = read_u32(input)? as usize;
            (0..rows)
                .map(|_| read_vector(input, dims))
                .collect::<io::Result<_>>()
                .map(Ok)
        }
        1 => read_text(input).map(Err),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown reply status {other}"),
        )),
    }
}

fn read_vector(input: &mut impl Read, dims: usize) -> io::Result<Vec<f32>> {
    let mut bytes = vec![0; dims * 4];
    input.read_exact(&mut bytes)?;
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|value| f32::from_le_bytes(*value))
        .collect())
}

/// The GPU helper as an embedder: one connection, one request at a time.
pub struct HelperEmbed {
    dims: usize,
    link: Mutex<Link>,
}

struct Link {
    input: BufReader<Box<dyn Read + Send>>,
    output: Option<Box<dyn Write + Send>>,
    child: Option<Arc<Mutex<Child>>>,
    stderr: Option<JoinHandle<String>>,
    limits: Option<Limits>,
    expired: Option<Duration>,
}

impl HelperEmbed {
    /// Connects to a helper that writes to `input` and reads requests from `output`.
    pub fn over(
        input: Box<dyn Read + Send>,
        output: Box<dyn Write + Send>,
    ) -> Result<Self, String> {
        let mut link = Link::new(input, output, None);
        let dims = read_ready(&mut link.input)
            .map_err(|err| format!("GPU helper did not start: {err}"))?;
        Ok(Self::with_link(dims, link))
    }

    /// Starts `program` with the model folder `model` and connects to it.
    pub fn spawn(program: &Path, model: &Path, lib_dir: Option<&Path>) -> Result<Self, String> {
        Self::spawn_with(program, model, lib_dir, Limits::default())
    }

    /// Starts `program` with the model folder `model` and connects to it, giving it `limits` to answer.
    pub fn spawn_with(
        program: &Path,
        model: &Path,
        lib_dir: Option<&Path>,
        limits: Limits,
    ) -> Result<Self, String> {
        let mut child = command(program, lib_dir)
            .arg(model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("{}: {err}", program.display()))?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let mut stderr = child.stderr.take().expect("stderr was piped");
        let stderr_reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        });
        let child = Arc::new(Mutex::new(child));
        let mut link = Link::new(Box::new(stdout), Box::new(stdin), Some(limits));
        link.child = Some(Arc::clone(&child));
        link.stderr = Some(stderr_reader);
        let (ready, expired) = watch(&child, limits.ready, || read_ready(&mut link.input));
        match ready {
            _ if expired => {
                link.output = None;
                link.kill();
                let _ = link.take_stderr();
                Err(format!(
                    "{}: GPU helper did not answer within {:?}; stopped it",
                    program.display(),
                    limits.ready
                ))
            }
            Ok(dims) => Ok(Self::with_link(dims, link)),
            Err(err) if err.kind() == io::ErrorKind::InvalidData => {
                link.kill();
                let _ = link.take_stderr();
                Err(format!("{}: {err}", program.display()))
            }
            Err(_) => Err(link.start_failure(program)),
        }
    }

    fn with_link(dims: usize, link: Link) -> Self {
        Self {
            dims,
            link: Mutex::new(link),
        }
    }
}

/// Runs `read`, killing `child` when it takes longer than `limit`; returns whether the limit ran out.
fn watch<T>(
    child: &Arc<Mutex<Child>>,
    limit: Duration,
    read: impl FnOnce() -> io::Result<T>,
) -> (io::Result<T>, bool) {
    let (cancel, wait) = mpsc::channel::<()>();
    let expired = Arc::new(AtomicBool::new(false));
    let fired = Arc::clone(&expired);
    let child = Arc::clone(child);
    let watchdog = thread::spawn(move || {
        if matches!(wait.recv_timeout(limit), Err(RecvTimeoutError::Timeout)) {
            fired.store(true, Ordering::SeqCst);
            let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = child.kill();
        }
    });
    let result = read();
    drop(cancel);
    let _ = watchdog.join();
    (result, expired.load(Ordering::SeqCst))
}

impl Link {
    fn new(
        input: Box<dyn Read + Send>,
        output: Box<dyn Write + Send>,
        limits: Option<Limits>,
    ) -> Self {
        Self {
            input: BufReader::new(input),
            output: Some(output),
            child: None,
            stderr: None,
            limits,
            expired: None,
        }
    }

    fn exchange(
        &mut self,
        texts: &[&str],
        dims: usize,
    ) -> io::Result<Result<Vec<Vec<f32>>, String>> {
        let output = self.output.as_mut().ok_or(io::ErrorKind::BrokenPipe)?;
        let input = &mut self.input;
        let mut send = || {
            output.write_all(&request_frame(texts))?;
            output.flush()?;
            read_reply(input, dims)
        };
        let (Some(limits), Some(child)) = (self.limits, self.child.as_ref()) else {
            return send();
        };
        let (result, expired) = watch(child, limits.reply, send);
        if expired {
            self.expired = Some(limits.reply);
            return Err(io::ErrorKind::TimedOut.into());
        }
        result
    }

    fn start_failure(&mut self, program: &Path) -> String {
        self.output = None;
        let status = self.child.as_ref().and_then(|child| {
            let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
            child.wait().ok()
        });
        let stderr = self.take_stderr();
        last_line(&stderr).unwrap_or_else(|| match status {
            Some(status) => format!("{} exited with {status}", program.display()),
            None => format!("{} did not start", program.display()),
        })
    }

    fn kill(&mut self) {
        if let Some(child) = self.child.as_ref() {
            let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn stopped(&mut self, err: io::Error) -> String {
        self.output = None;
        self.kill();
        if let Some(limit) = self.expired {
            return format!("GPU helper did not answer within {limit:?}; stopped it");
        }
        let stderr = self.take_stderr();
        match last_line(&stderr) {
            Some(line) => format!("GPU helper stopped: {err}: {line}"),
            None => format!("GPU helper stopped: {err}"),
        }
    }

    fn take_stderr(&mut self) -> String {
        self.stderr
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default()
    }

    fn stop(&mut self) {
        let stop = self.limits.map_or(Duration::ZERO, |limits| limits.stop);
        let Some(child) = self.child.take() else {
            return;
        };
        let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
        if wait_within(&mut child, stop).is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Waits up to `limit` for `child` to exit; past it, kills and reaps it and returns `None`.
fn wait_within(child: &mut Child, limit: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(Some(status)),
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
        }
    }
}

fn last_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim_start)
        .find(|line| !line.trim().is_empty() && !line.starts_with("note:"))
        .map(|line| line.trim_end().to_owned())
}

fn command(program: &Path, lib_dir: Option<&Path>) -> Command {
    let mut command = Command::new(program);
    if let Some(dir) = lib_dir {
        prepend_library_dir(&mut command, dir);
    }
    command
}

/// Runs `program --check` and reports why it finds no usable CUDA device.
pub fn check_helper(program: &Path, lib_dir: Option<&Path>, limit: Duration) -> Result<(), String> {
    let mut child = command(program, lib_dir)
        .arg("--check")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{}: {err}", program.display()))?;
    let mut stderr = child.stderr.take().expect("stderr was piped");
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let status = match wait_within(&mut child, limit) {
        Ok(Some(status)) => status,
        Ok(None) => {
            return Err(format!(
                "{} did not finish within {limit:?}",
                program.display()
            ))
        }
        Err(err) => return Err(format!("{}: {err}", program.display())),
    };
    let stderr = reader.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    Err(last_line(&stderr).unwrap_or_else(|| format!("{} exited with {status}", program.display())))
}

impl Embed for HelperEmbed {
    fn dims(&self) -> usize {
        self.dims
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        let mut link = self.link.lock().unwrap_or_else(PoisonError::into_inner);
        match link.exchange(texts, self.dims) {
            Ok(Ok(vectors)) if vectors.len() == texts.len() => Ok(vectors),
            Ok(Ok(vectors)) => Err(format!(
                "GPU helper returned {} vectors for {} texts",
                vectors.len(),
                texts.len()
            )),
            Ok(Err(message)) => Err(message),
            Err(err) => Err(link.stopped(err)),
        }
    }
}

impl Drop for HelperEmbed {
    fn drop(&mut self) {
        let link = self.link.get_mut().unwrap_or_else(PoisonError::into_inner);
        link.output = None;
        link.stop();
    }
}
