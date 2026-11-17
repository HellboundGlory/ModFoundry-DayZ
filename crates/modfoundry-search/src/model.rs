//! Model files for the embedder: where they live and how they are fetched.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// The Hugging Face model the embedder loads.
pub const MODEL_ID: &str = "BAAI/bge-small-en-v1.5";
/// The commit of `MODEL_ID` whose files are fetched and checked.
pub const REVISION: &str = "5c38ec7c405ec4b44b94cc5a9bb96e735b38267a";

/// One file of the model: its name, exact size and SHA-256 (hex).
#[doc(hidden)]
pub struct ModelFile {
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

const FILES: [ModelFile; 3] = [
    ModelFile {
        name: "config.json",
        bytes: 743,
        sha256: "094f8e891b932f2000c92cfc663bac4c62069f5d8af5b5278c4306aef3084750",
    },
    ModelFile {
        name: "tokenizer.json",
        bytes: 711_396,
        sha256: "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
    },
    ModelFile {
        name: "model.safetensors",
        bytes: 133_466_304,
        sha256: "3c9f31665447c8911517620762200d2245a2518d6e7208acc78cd9db317e21ad",
    },
];

/// The folder a model is kept in under the host cache folder.
pub fn default_model_dir(cache: &Path) -> PathBuf {
    cache
        .join("modfoundry")
        .join("models")
        .join("bge-small-en-v1.5-5c38ec7")
}

/// Writes the bytes at `url` into `out`.
pub trait Fetch {
    fn fetch(&self, url: &str, out: &mut dyn Write) -> Result<(), String>;
}

/// The real client: HTTPS over rustls, following Hugging Face's redirects.
pub struct HttpFetch;

impl Fetch for HttpFetch {
    fn fetch(&self, url: &str, out: &mut dyn Write) -> Result<(), String> {
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            // ureq has no idle timeout; a total this long still fits the 133 MB file on a slow link.
            .timeout_recv_body(Some(Duration::from_secs(2 * 60 * 60)))
            .build()
            .new_agent();
        let mut response = agent.get(url).call().map_err(|err| err.to_string())?;
        io::copy(&mut response.body_mut().as_reader(), out)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }
}

/// Fetches any model file missing from `dir` or of the wrong size, checking each one.
pub fn ensure_model(
    dir: &Path,
    fetch: &dyn Fetch,
    progress: &mut dyn FnMut(&str),
) -> Result<(), ModelError> {
    ensure_files(dir, &FILES, fetch, progress)
}

/// Checks that every model file in `dir` exists with the right size, without fetching.
pub fn check_model(dir: &Path) -> Result<(), ModelError> {
    for file in &FILES {
        let len = fs::metadata(dir.join(file.name))
            .map_err(|err| match err.kind() {
                io::ErrorKind::NotFound => ModelError::Missing { file: file.name },
                _ => io_error(&dir.join(file.name), err),
            })?
            .len();
        if len != file.bytes {
            return Err(ModelError::Size {
                file: file.name,
                expected: file.bytes,
                actual: len,
            });
        }
    }
    Ok(())
}

/// The fetch-and-check step behind `ensure_model`, for a table of files.
#[doc(hidden)]
pub fn ensure_files(
    dir: &Path,
    files: &[ModelFile],
    fetch: &dyn Fetch,
    progress: &mut dyn FnMut(&str),
) -> Result<(), ModelError> {
    fs::create_dir_all(dir).map_err(|err| io_error(dir, err))?;
    for file in files {
        if fs::metadata(dir.join(file.name)).is_ok_and(|meta| meta.len() == file.bytes) {
            continue;
        }
        progress(&format!(
            "downloading {} ({})",
            file.name,
            size_label(file.bytes)
        ));
        download(dir, file, fetch)?;
    }
    Ok(())
}

fn download(dir: &Path, file: &ModelFile, fetch: &dyn Fetch) -> Result<(), ModelError> {
    let part = crate::store::part_path(&dir.join(file.name));
    let url = format!(
        "https://huggingface.co/{MODEL_ID}/resolve/{REVISION}/{}",
        file.name
    );
    let placed = fetch_into(&part, &url, file, fetch)
        .and_then(|()| verify(&part, file))
        .and_then(|()| fs::rename(&part, dir.join(file.name)).map_err(|err| io_error(&part, err)));
    if placed.is_err() {
        let _ = fs::remove_file(&part);
    }
    placed
}

fn fetch_into(
    part: &Path,
    url: &str,
    file: &ModelFile,
    fetch: &dyn Fetch,
) -> Result<(), ModelError> {
    let out = File::create(part).map_err(|err| io_error(part, err))?;
    let mut out = Capped {
        out,
        left: file.bytes,
    };
    fetch
        .fetch(url, &mut out)
        .map_err(|message| ModelError::Fetch {
            file: file.name,
            message,
        })
}

struct Capped {
    out: File,
    left: u64,
}

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() as u64 > self.left {
            return Err(io::Error::other(
                "the server sent more bytes than the file holds",
            ));
        }
        let written = self.out.write(buf)?;
        self.left -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

fn verify(part: &Path, file: &ModelFile) -> Result<(), ModelError> {
    let len = fs::metadata(part).map_err(|err| io_error(part, err))?.len();
    if len != file.bytes {
        return Err(ModelError::Size {
            file: file.name,
            expected: file.bytes,
            actual: len,
        });
    }
    let actual = sha256_hex(part)?;
    if actual != file.sha256 {
        return Err(ModelError::Hash {
            file: file.name,
            expected: file.sha256,
            actual,
        });
    }
    Ok(())
}

fn sha256_hex(path: &Path) -> Result<String, ModelError> {
    let mut hasher = Sha256::new();
    let mut input = File::open(path).map_err(|err| io_error(path, err))?;
    io::copy(&mut input, &mut hasher).map_err(|err| io_error(path, err))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn size_label(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{} MB", bytes / 1_000_000)
    } else if bytes >= 1_000 {
        format!("{} kB", bytes / 1_000)
    } else {
        format!("{bytes} B")
    }
}

fn io_error(path: &Path, err: io::Error) -> ModelError {
    ModelError::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    }
}

/// Why the model folder cannot be used.
#[derive(Debug)]
pub enum ModelError {
    Io {
        path: PathBuf,
        message: String,
    },
    Fetch {
        file: &'static str,
        message: String,
    },
    Missing {
        file: &'static str,
    },
    Size {
        file: &'static str,
        expected: u64,
        actual: u64,
    },
    Hash {
        file: &'static str,
        expected: &'static str,
        actual: String,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Fetch { file, message } => write!(f, "could not download {file}: {message}"),
            Self::Missing { file } => write!(f, "model file {file} is missing"),
            Self::Size {
                file,
                expected,
                actual,
            } => write!(
                f,
                "model file {file} is {actual} bytes, expected {expected}"
            ),
            Self::Hash {
                file,
                expected,
                actual,
            } => write!(
                f,
                "model file {file} has sha256 {actual}, expected {expected}"
            ),
        }
    }
}

impl std::error::Error for ModelError {}
