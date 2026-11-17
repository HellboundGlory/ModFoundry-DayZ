//! Checking for and installing newer ModFoundry releases: `host.update_check` and `host.update`.

use modfoundry_platform::archive::{make_executable, unpack_tar_gz};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAY_SECS: u64 = 86_400;
const INSTALL_MARKER: &str = ".modfoundry-install";
const SKILL_PATH: &str = "skill/modfoundry/SKILL.md";
const PROGRAMS: [&str; 4] = [
    "mf",
    "modfoundry-mcp-server",
    "modfoundry-steam",
    "libsteam_api.so",
];
const ARCHIVE_LIMIT: u64 = 300 * 1024 * 1024;
const CHECKSUM_LIMIT: u64 = 4 * 1024;
const VERSION_WAIT: Duration = Duration::from_secs(10);
/// How long `host.update_check` gives GitHub.
pub const CHECK_WAIT: Duration = Duration::from_secs(10);
/// How long `mf update` gives each request, the archive download included.
pub const DOWNLOAD_WAIT: Duration = Duration::from_secs(600);

/// The last check: when it ran and the newest version it found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckState {
    pub checked_at: u64,
    pub latest: Option<String>,
}

/// A check is due a day after the last one, or when the saved time lies in the future.
fn is_due(state: Option<&CheckState>, now: u64) -> bool {
    state.is_none_or(|state| {
        now.saturating_sub(state.checked_at) >= DAY_SECS || state.checked_at > now
    })
}

/// A published release: its version and the links to its tarball and checksum.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    pub tarball_url: String,
    pub sha256_url: String,
}

fn parse_latest(json: &str) -> Result<Release, String> {
    let latest: Latest =
        serde_json::from_str(json).map_err(|e| format!("unreadable release answer: {e}"))?;
    let version = Version::parse(&latest.tag_name)
        .ok_or_else(|| format!("unreadable tag {}", latest.tag_name))?;
    let tarball = tarball_name(&version);
    let sha256 = format!("{tarball}.sha256");
    Ok(Release {
        version,
        tarball_url: asset_url(&latest.assets, &tarball)?,
        sha256_url: asset_url(&latest.assets, &sha256)?,
    })
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn asset_url(assets: &[Asset], name: &str) -> Result<String, String> {
    let asset = assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| format!("release has no {name} asset"))?;
    let releases = format!("https://github.com/{RELEASES_REPO}/releases/download/");
    if !asset.browser_download_url.starts_with(&releases) {
        return Err(format!(
            "release asset {name} is not served from {releases}: {}",
            asset.browser_download_url
        ));
    }
    Ok(asset.browser_download_url.clone())
}

/// A release version, compared by major, minor, then patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    /// Reads `X.Y.Z` or `vX.Y.Z`; anything else is `None`.
    pub fn parse(text: &str) -> Option<Version> {
        let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
        let version = Version {
            major: number(parts.next()?)?,
            minor: number(parts.next()?)?,
            patch: number(parts.next()?)?,
        };
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn notice(current: &Version, latest: &str) -> Option<String> {
    let latest = Version::parse(latest)?;
    (latest > *current)
        .then(|| format!("ModFoundry {latest} is out (this is {current}): run mf update"))
}

/// The folder of `exe` when it holds the marker the installer writes next to `mf`.
fn install_dir_of(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    dir.join(INSTALL_MARKER)
        .is_file()
        .then(|| dir.to_path_buf())
}

/// False when `MF_NO_UPDATE_CHECK` is set to anything but an empty value or `0`.
fn checks_enabled(env: impl Fn(&str) -> Option<String>) -> bool {
    env("MF_NO_UPDATE_CHECK").is_none_or(|value| value.is_empty() || value == "0")
}

/// The install folder an update may use, or the reason this `mf` may not update.
pub fn update_target(
    lib: Option<PathBuf>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<PathBuf, &'static str> {
    let lib = lib.ok_or("this mf was not installed from a release package")?;
    if checks_enabled(env) {
        Ok(lib)
    } else {
        Err("MF_NO_UPDATE_CHECK is set")
    }
}

fn load_state(path: &Path) -> Option<CheckState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_state(path: &Path, state: &CheckState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string(state).map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}

/// Where the newest release comes from; `Ok(None)` when none is published yet.
pub trait ReleaseSource {
    fn latest(&self) -> Result<Option<Release>, String>;
    /// Writes the bytes at `url` into `out`, refused when they run past `max_bytes`; on an error `out` may hold part of them.
    fn fetch(&self, url: &str, max_bytes: u64, out: &mut dyn Write) -> Result<(), String>;
}

/// The answer `host.update_check` returns.
#[derive(Debug, PartialEq, Serialize)]
pub struct UpdateCheck {
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub notice: Option<String>,
}

fn check(
    source: &dyn ReleaseSource,
    current: &Version,
    now: u64,
    state: &Path,
) -> Result<UpdateCheck, String> {
    let release = source.latest()?;
    let latest = release.map(|release| release.version.to_string());
    let notice = latest.as_deref().and_then(|latest| notice(current, latest));
    // The answer stands even when the cache folder cannot be written.
    let _ = save_state(
        state,
        &CheckState {
            checked_at: now,
            latest: latest.clone(),
        },
    );
    Ok(UpdateCheck {
        current: current.to_string(),
        latest,
        update_available: notice.is_some(),
        notice,
    })
}

/// Parses the version text `current`, then runs `check`.
pub fn run_check(
    source: &dyn ReleaseSource,
    current: &str,
    now: u64,
    state: &Path,
) -> Result<UpdateCheck, String> {
    let current = parse_current(current)?;
    check(source, &current, now, state)
}

fn parse_current(current: &str) -> Result<Version, String> {
    Version::parse(current).ok_or_else(|| format!("unreadable version {current}"))
}

/// The notice saved by the last check, against `current`; no request is made.
fn stored_notice(current: &Version, state_path: &Path) -> Option<String> {
    let latest = load_state(state_path)?.latest?;
    notice(current, &latest)
}

/// How long a due check may take, and how long the CLI waits for it before exiting.
pub const NOTICE_WAIT: Duration = Duration::from_secs(2);

/// The update notice for one run: the saved one to print, and the check started for it when one is due.
pub struct NoticeStart {
    pub saved: Option<String>,
    pub check: Option<Receiver<Option<String>>>,
}

/// Starts the notice for this run; both parts are empty when update notices are off.
pub fn start_notice() -> NoticeStart {
    let (Some(state), Some(current)) = (
        update_notices_enabled(),
        Version::parse(env!("CARGO_PKG_VERSION")),
    ) else {
        return NoticeStart {
            saved: None,
            check: None,
        };
    };
    let source = GitHubReleases {
        timeout: NOTICE_WAIT,
    };
    notice_start(current, state, now_secs(), source)
}

fn notice_start(
    current: Version,
    state_path: PathBuf,
    now: u64,
    source: impl ReleaseSource + Send + 'static,
) -> NoticeStart {
    let saved = stored_notice(&current, &state_path);
    let check = check_in_background(source, current, state_path, now);
    NoticeStart { saved, check }
}

/// The attempt is saved before the request, so a failed or cut-short check still waits a day.
fn check_in_background(
    source: impl ReleaseSource + Send + 'static,
    current: Version,
    state_path: PathBuf,
    now: u64,
) -> Option<Receiver<Option<String>>> {
    let saved = load_state(&state_path);
    if !is_due(saved.as_ref(), now) {
        return None;
    }
    let attempt = CheckState {
        checked_at: now,
        latest: saved.and_then(|state| state.latest),
    };
    let _ = save_state(&state_path, &attempt);
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let notice = check(&source, &current, now, &state_path)
                .ok()
                .and_then(|answer| answer.notice);
            let _ = sender.send(notice);
        })
        .ok()
        .map(|_| receiver)
}

/// The public repository whose GitHub Releases carry the installable packages.
pub const RELEASES_REPO: &str = "HellboundGlory/ModFoundry-DayZ";

/// The GitHub Releases answer for this repository, over HTTPS.
pub struct GitHubReleases {
    pub timeout: Duration,
}

impl GitHubReleases {
    fn agent(&self) -> ureq::Agent {
        ureq::Agent::config_builder()
            .https_only(true)
            .timeout_global(Some(self.timeout))
            .user_agent(concat!("modfoundry/", env!("CARGO_PKG_VERSION")))
            .http_status_as_error(false)
            .build()
            .new_agent()
    }
}

impl ReleaseSource for GitHubReleases {
    fn fetch(&self, url: &str, max_bytes: u64, out: &mut dyn Write) -> Result<(), String> {
        let mut response = self.agent().get(url).call().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(format!("GitHub answered {status} for {url}"));
        }
        std::io::copy(
            &mut response.body_mut().with_config().limit(max_bytes).reader(),
            out,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    fn latest(&self) -> Result<Option<Release>, String> {
        let agent = self.agent();
        let url = format!("https://api.github.com/repos/{RELEASES_REPO}/releases/latest");
        let mut response = agent
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .call()
            .map_err(|e| e.to_string())?;
        match response.status().as_u16() {
            200 => {
                let body = response
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| e.to_string())?;
                parse_latest(&body).map(Some)
            }
            404 => Ok(None),
            status => Err(format!("GitHub answered {status} for the newest release")),
        }
    }
}

/// Where the check remembers its last answer.
pub fn state_path() -> Option<PathBuf> {
    modfoundry_platform::app_cache_dir().map(|dir| dir.join("modfoundry").join("update-check.json"))
}

/// Where an install keeps its download and unpacked copy.
pub fn work_dir() -> Option<PathBuf> {
    modfoundry_platform::app_cache_dir().map(|dir| dir.join("modfoundry").join("update"))
}

/// The agents copy of the ModFoundry skill, which an install keeps in step with the program.
pub fn agents_skill_path() -> Option<PathBuf> {
    modfoundry_platform::agents_dir().map(|dir| dir.join("skills/modfoundry/SKILL.md"))
}

/// The running `mf`'s folder, when it is a release install.
pub fn release_install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    install_dir_of(&exe)
}

/// The state path when update notices are on: a release install with checks not switched off.
fn update_notices_enabled() -> Option<PathBuf> {
    let enabled = checks_enabled(|key| std::env::var(key).ok());
    if !enabled || release_install_dir().is_none() {
        return None;
    }
    state_path()
}

/// The notice saved by the last check, for this build, when update notices are on.
pub fn saved_notice() -> Option<String> {
    let state = update_notices_enabled()?;
    stored_notice(&Version::parse(env!("CARGO_PKG_VERSION"))?, &state)
}

/// Seconds since the Unix epoch; 0 when the clock is set before it.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// What `install_release` replaced: the version that was running and the one now in `lib`.
#[derive(Debug, PartialEq)]
pub struct Installed {
    pub from: String,
    pub to: String,
}

/// Downloads `release`, checks it, and puts it in `lib` once the new `mf` runs; the old programs stay until then.
/// `agents_skill` is the agents copy of the skill, replaced only while it still matches the old one.
fn install_release(
    lib: &Path,
    release: &Release,
    source: &dyn ReleaseSource,
    work: &Path,
    current: &Version,
    agents_skill: Option<&Path>,
) -> Result<Installed, String> {
    fs::create_dir_all(work).map_err(|e| io_error(work, e))?;
    let _lock = lock_work(work)?;
    let archive_path = download_archive(release, source, work)?;
    let placed = place(lib, release, &archive_path, work, agents_skill);
    let _ = fs::remove_file(&archive_path);
    let _ = remove_tree(&work.join("unpacked"));
    placed.map(|()| Installed {
        from: current.to_string(),
        to: release.version.to_string(),
    })
}

/// What `mf update` did: installed a newer release, or found none newer than this one.
#[derive(Debug, PartialEq)]
pub enum Updated {
    Installed(Installed),
    /// `latest` is `None` when no release is published yet.
    NotNewer {
        latest: Option<String>,
    },
}

/// Installs the newest release when it is newer than `current`.
pub fn run_update(
    source: &dyn ReleaseSource,
    current: &str,
    lib: &Path,
    work: &Path,
    agents_skill: Option<&Path>,
) -> Result<Updated, String> {
    let current = parse_current(current)?;
    match source.latest()? {
        Some(release) if release.version > current => {
            install_release(lib, &release, source, work, &current, agents_skill)
                .map(Updated::Installed)
        }
        latest => Ok(Updated::NotNewer {
            latest: latest.map(|release| release.version.to_string()),
        }),
    }
}

fn lock_work(work: &Path) -> Result<fs::File, String> {
    let path = work.join(".lock");
    let file = fs::File::create(&path).map_err(|e| io_error(&path, e))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err("another mf update is running".into()),
        Err(fs::TryLockError::Error(e)) => Err(io_error(&path, e)),
    }
}

/// Unpacks the checked archive beside `lib` and swaps it in; `.old` is removed only once the new copy is ready.
fn place(
    lib: &Path,
    release: &Release,
    archive_path: &Path,
    work: &Path,
    agents_skill: Option<&Path>,
) -> Result<(), String> {
    let name = tarball_name(&release.version);
    let top = unpack(archive_path, work, &name)?;
    let new = sibling(lib, ".new");
    let old = sibling(lib, ".old");
    remove_tree(&new)?;
    if let Err(error) = build_new(&top, &new) {
        let _ = fs::remove_dir_all(&new);
        return Err(error);
    }
    remove_tree(&old)?;
    if let Err(error) = swap(lib, &new, &old) {
        let _ = fs::remove_dir_all(&new);
        return Err(error);
    }
    if let Err(reason) = check_version(&lib.join("mf"), &release.version, VERSION_WAIT) {
        return Err(restore(lib, &old, &release.version, &reason));
    }
    if let Some(agents) = agents_skill {
        refresh_agents_skill(agents, &old.join(SKILL_PATH), &lib.join(SKILL_PATH));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

fn tarball_name(version: &Version) -> String {
    format!("ModFoundry-{version}-linux-x86_64.tar.gz")
}

fn download_archive(
    release: &Release,
    source: &dyn ReleaseSource,
    work: &Path,
) -> Result<PathBuf, String> {
    let expected = expected_checksum(release, source)?;
    let name = tarball_name(&release.version);
    let part = work.join(format!("{name}.part"));
    let archive = work.join(&name);
    let placed = fetch_part(release, source, &expected, &part)
        .and_then(|()| fs::rename(&part, &archive).map_err(|e| io_error(&part, e)));
    if placed.is_err() {
        let _ = fs::remove_file(&part);
    }
    placed.map(|()| archive)
}

fn expected_checksum(release: &Release, source: &dyn ReleaseSource) -> Result<String, String> {
    let mut checksum = Vec::new();
    source.fetch(&release.sha256_url, CHECKSUM_LIMIT, &mut checksum)?;
    let text = String::from_utf8_lossy(&checksum);
    let mut words = text.split_whitespace();
    let expected = words.next().unwrap_or("");
    let named = words.next().unwrap_or("").trim_start_matches('*');
    let tarball = tarball_name(&release.version);
    if named != tarball {
        return Err(format!("the checksum file names {named}, not {tarball}"));
    }
    Ok(expected.to_owned())
}

fn fetch_part(
    release: &Release,
    source: &dyn ReleaseSource,
    expected: &str,
    part: &Path,
) -> Result<(), String> {
    let file = fs::File::create(part).map_err(|e| io_error(part, e))?;
    let mut sink = HashingFile {
        file,
        hash: Sha256::new(),
        part,
    };
    source.fetch(&release.tarball_url, ARCHIVE_LIMIT, &mut sink)?;
    let actual = format!("{:x}", sink.hash.finalize());
    if !expected.eq_ignore_ascii_case(&actual) {
        return Err(format!(
            "checksum of {} is {actual}, the release says {expected}",
            tarball_name(&release.version)
        ));
    }
    Ok(())
}

struct HashingFile<'a> {
    file: fs::File,
    hash: Sha256,
    part: &'a Path,
}

impl Write for HashingFile<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self
            .file
            .write(buf)
            .map_err(|e| std::io::Error::other(io_error(self.part, e)))?;
        self.hash.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn unpack(archive: &Path, work: &Path, name: &str) -> Result<PathBuf, String> {
    let into = work.join("unpacked");
    remove_tree(&into)?;
    let top = name.strip_suffix(".tar.gz").unwrap_or(name);
    let folder = unpack_tar_gz(archive, &into, top)?;
    for program in PROGRAMS {
        if !folder.join(program).is_file() {
            return Err(format!("the release has no {program}"));
        }
    }
    Ok(folder)
}

fn build_new(top: &Path, new: &Path) -> Result<(), String> {
    fs::create_dir_all(new).map_err(|e| io_error(new, e))?;
    for program in PROGRAMS {
        let to = new.join(program);
        fs::copy(top.join(program), &to).map_err(|e| io_error(&to, e))?;
        make_executable(&to).map_err(|e| io_error(&to, e))?;
    }
    let skill = top.join(SKILL_PATH);
    if skill.is_file() {
        let to = new.join(SKILL_PATH);
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
        }
        fs::copy(&skill, &to).map_err(|e| io_error(&to, e))?;
    }
    let marker = new.join(INSTALL_MARKER);
    fs::write(&marker, "").map_err(|e| io_error(&marker, e))
}

fn swap(lib: &Path, new: &Path, old: &Path) -> Result<(), String> {
    fs::rename(lib, old).map_err(|e| io_error(lib, e))?;
    if let Err(e) = fs::rename(new, lib) {
        return Err(match fs::rename(old, lib) {
            Ok(()) => io_error(new, e),
            Err(back) => format!(
                "{}; the old version could not be put back ({back}) and is in {}",
                io_error(new, e),
                old.display()
            ),
        });
    }
    Ok(())
}

fn check_version(mf: &Path, version: &Version, wait: Duration) -> Result<(), String> {
    let mut child = spawn_version(mf)?;
    let deadline = std::time::Instant::now() + wait;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "it did not answer --version within {} s",
                    wait.as_secs_f32()
                ));
            }
            Err(e) => return Err(format!("{}: {e}", mf.display())),
        }
    };
    let mut printed = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut out, &mut printed);
    }
    let expected = version.to_string();
    if status.success() && printed.split_whitespace().last() == Some(expected.as_str()) {
        return Ok(());
    }
    Err(format!(
        "it exited with {status} and printed {:?}",
        printed.trim()
    ))
}

/// Starts `mf --version`, waiting out a short "text file busy" while another thread's child still holds the file.
fn spawn_version(mf: &Path) -> Result<std::process::Child, String> {
    let mut tries = 0;
    loop {
        let started = Command::new(mf)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        match started {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 40 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            other => return other.map_err(|e| format!("{}: {e}", mf.display())),
        }
    }
}

fn restore(lib: &Path, old: &Path, version: &Version, reason: &str) -> String {
    let failed = sibling(lib, ".failed");
    let _ = remove_tree(&failed);
    if fs::rename(lib, &failed).is_err() {
        let _ = fs::remove_dir_all(lib);
    }
    match fs::rename(old, lib) {
        Ok(()) => {
            let _ = fs::remove_dir_all(&failed);
            format!("ModFoundry {version} did not start ({reason}); the old version was kept")
        }
        Err(e) => format!(
            "ModFoundry {version} did not start ({reason}), and the old version could not be put back ({e}); it is in {}",
            old.display()
        ),
    }
}

/// Replaces the agents copy with the new skill while it still matches the old one. A link is never followed.
fn refresh_agents_skill(agents: &Path, old_skill: &Path, new_skill: &Path) {
    let is_link = fs::symlink_metadata(agents).is_ok_and(|meta| meta.file_type().is_symlink())
        || agents
            .parent()
            .is_none_or(|dir| dir.canonicalize().ok().as_deref() != Some(dir));
    let same = matches!((fs::read(agents), fs::read(old_skill)), (Ok(a), Ok(b)) if a == b);
    if same && !is_link {
        let temp = sibling(agents, ".new");
        let _ = fs::remove_file(&temp);
        let placed = fs::copy(new_skill, &temp).and_then(|_| fs::rename(&temp, agents));
        if placed.is_err() {
            let _ = fs::remove_file(&temp);
        }
    }
}

fn sibling(lib: &Path, suffix: &str) -> PathBuf {
    let mut name = lib.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    lib.with_file_name(name)
}

fn remove_tree(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_error(path, e)),
        _ => Ok(()),
    }
}

fn io_error(path: &Path, error: std::io::Error) -> String {
    format!("{}: {error}", path.display())
}

fn number(part: &str) -> Option<u64> {
    let leading_zero = part.len() > 1 && part.starts_with('0');
    if leading_zero || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use modfoundry_core::{Actor, Outcome};
    use serde_json::json;

    const LATEST: &str = r#"{
        "tag_name": "v2.2.0",
        "draft": false,
        "prerelease": false,
        "assets": [
            {
                "name": "ModFoundry-2.2.0-linux-x86_64.tar.gz",
                "browser_download_url": "https://github.com/HellboundGlory/ModFoundry-DayZ/releases/download/v2.2.0/ModFoundry-2.2.0-linux-x86_64.tar.gz"
            },
            {
                "name": "ModFoundry-2.2.0-linux-x86_64.tar.gz.sha256",
                "browser_download_url": "https://github.com/HellboundGlory/ModFoundry-DayZ/releases/download/v2.2.0/ModFoundry-2.2.0-linux-x86_64.tar.gz.sha256"
            }
        ]
    }"#;

    #[test]
    fn reads_the_tag_and_the_two_assets_of_the_release() {
        let release = parse_latest(LATEST).unwrap();
        assert_eq!(release.version.to_string(), "2.2.0");
        assert_eq!(
            release.tarball_url,
            "https://github.com/HellboundGlory/ModFoundry-DayZ/releases/download/v2.2.0/ModFoundry-2.2.0-linux-x86_64.tar.gz"
        );
        assert!(release
            .sha256_url
            .ends_with("ModFoundry-2.2.0-linux-x86_64.tar.gz.sha256"));
    }

    #[test]
    fn a_check_is_due_a_day_after_the_last_one_or_when_the_time_is_ahead() {
        let at = |checked_at| CheckState {
            checked_at,
            latest: None,
        };
        let now = 1_000_000;
        let cases = [
            (None, true),
            (Some(at(now)), false),
            (Some(at(now - DAY_SECS + 1)), false),
            (Some(at(now - DAY_SECS)), true),
            (Some(at(now + 60)), true),
        ];
        for (state, want) in cases {
            assert_eq!(is_due(state.as_ref(), now), want, "{state:?}");
        }
    }

    #[test]
    fn notices_only_a_newer_release() {
        let current = Version::parse("2.2.0").unwrap();
        let cases = [
            ("2.1.0", None),
            ("2.2.0", None),
            ("junk", None),
            (
                "v2.3.0",
                Some("ModFoundry 2.3.0 is out (this is 2.2.0): run mf update"),
            ),
        ];
        for (latest, want) in cases {
            assert_eq!(notice(&current, latest).as_deref(), want, "{latest}");
        }
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mf-update-{name}-{}", std::process::id()))
    }

    #[test]
    fn the_install_folder_is_the_one_holding_the_marker() {
        let dir = scratch("marker");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("mf");
        assert_eq!(install_dir_of(&exe), None);
        std::fs::write(dir.join(".modfoundry-install"), "").unwrap();
        assert_eq!(install_dir_of(&exe), Some(dir.clone()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    struct FakeSource(Result<Option<&'static str>, &'static str>);

    impl ReleaseSource for FakeSource {
        fn latest(&self) -> Result<Option<Release>, String> {
            match self.0 {
                Ok(None) => Ok(None),
                Ok(Some(tag)) => Ok(Some(Release {
                    version: Version::parse(tag).unwrap(),
                    tarball_url: String::new(),
                    sha256_url: String::new(),
                })),
                Err(message) => Err(message.to_owned()),
            }
        }

        fn fetch(&self, url: &str, _max_bytes: u64, _out: &mut dyn Write) -> Result<(), String> {
            Err(format!("no download in this test: {url}"))
        }
    }

    #[test]
    fn a_newer_release_is_saved_and_noticed() {
        let dir = scratch("check-newer");
        let path = dir.join("update-check.json");
        let current = Version::parse("2.2.0").unwrap();
        let source = FakeSource(Ok(Some("v2.3.0")));
        let answer = check(&source, &current, 1_000, &path).unwrap();
        assert_eq!(
            answer,
            UpdateCheck {
                current: "2.2.0".into(),
                latest: Some("2.3.0".into()),
                update_available: true,
                notice: Some("ModFoundry 2.3.0 is out (this is 2.2.0): run mf update".into()),
            }
        );
        assert_eq!(
            load_state(&path),
            Some(CheckState {
                checked_at: 1_000,
                latest: Some("2.3.0".into()),
            })
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_published_release_is_saved_as_no_latest_version() {
        let dir = scratch("check-none");
        let path = dir.join("update-check.json");
        let current = Version::parse("2.2.0").unwrap();
        let answer = check(&FakeSource(Ok(None)), &current, 1_000, &path).unwrap();
        assert_eq!(answer.latest, None);
        assert!(!answer.update_available);
        assert_eq!(answer.notice, None);
        assert_eq!(
            load_state(&path),
            Some(CheckState {
                checked_at: 1_000,
                latest: None,
            })
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_request_is_an_error_and_saves_nothing() {
        let dir = scratch("check-failed");
        let path = dir.join("update-check.json");
        let current = Version::parse("2.2.0").unwrap();
        let error =
            check(&FakeSource(Err("connection reset")), &current, 1_000, &path).unwrap_err();
        assert_eq!(error, "connection reset");
        assert_eq!(load_state(&path), None);
    }

    #[test]
    fn update_check_answers_without_a_request_outside_a_release_install() {
        let outcome = crate::registry::core()
            .dispatch(Actor::CliSession, "host.update_check", json!({}))
            .unwrap();
        let Outcome::Data(answer) = outcome else {
            panic!("expected Data, got {outcome:?}");
        };
        assert_eq!(answer["checked"], json!(false));
        assert_eq!(answer["current"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            answer["reason"],
            "this mf was not installed from a release package"
        );
    }

    #[test]
    fn saved_state_reads_back_and_a_missing_file_reads_as_none() {
        let dir = scratch("state");
        let path = dir.join("nested").join("update-check.json");
        assert_eq!(load_state(&path), None);
        let state = CheckState {
            checked_at: 42,
            latest: Some("2.3.0".into()),
        };
        save_state(&path, &state).unwrap();
        assert_eq!(load_state(&path), Some(state));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_switch_turns_checks_off_for_any_value_but_empty_or_zero() {
        let cases = [
            (None, true),
            (Some("1"), false),
            (Some("0"), true),
            (Some(""), true),
        ];
        for (value, want) in cases {
            let env = |key: &str| {
                assert_eq!(key, "MF_NO_UPDATE_CHECK");
                value.map(String::from)
            };
            assert_eq!(checks_enabled(env), want, "{value:?}");
        }
    }

    #[test]
    fn a_malformed_answer_says_what_could_not_be_read() {
        let Err(error) = parse_latest(r#"{"tag_name": "v2.2.0"}"#) else {
            panic!("a release without assets parsed");
        };
        assert!(error.starts_with("unreadable release answer"), "{error}");
    }

    #[test]
    fn a_missing_checksum_asset_is_an_error_naming_it() {
        let without_checksum =
            LATEST.replace("linux-x86_64.tar.gz.sha256\"", "linux-x86_64.zip.sha256\"");
        let error = parse_latest(&without_checksum).err().unwrap();
        assert!(
            error.contains("ModFoundry-2.2.0-linux-x86_64.tar.gz.sha256"),
            "{error}"
        );
    }

    #[test]
    fn parses_three_numbers_with_an_optional_v() {
        let cases = [
            ("2.2.0", Some((2, 2, 0))),
            ("v2.2.0", Some((2, 2, 0))),
            ("2.2", None),
            ("v2.2.0-rc1", None),
            ("v02.2.0", None),
            ("x.y.z", None),
        ];
        for (text, want) in cases {
            let got = Version::parse(text).map(|v| (v.major, v.minor, v.patch));
            assert_eq!(got, want, "{text}");
        }
    }

    #[test]
    fn displays_as_three_numbers() {
        let version = Version::parse("v10.2.7").unwrap();
        assert_eq!(version.to_string(), "10.2.7");
    }

    #[test]
    fn the_stored_notice_comes_from_the_saved_state_alone() {
        let current = Version::parse("2.2.0").unwrap();
        let path = scratch("stored-notice").join("update-check.json");
        let saved = |latest: Option<&str>| CheckState {
            checked_at: 1_000,
            latest: latest.map(String::from),
        };
        let cases = [
            (
                Some(saved(Some("2.3.0"))),
                Some("ModFoundry 2.3.0 is out (this is 2.2.0): run mf update"),
            ),
            (Some(saved(Some("2.2.0"))), None),
            (Some(saved(None)), None),
            (None, None),
        ];
        for (state, want) in cases {
            let _ = std::fs::remove_file(&path);
            if let Some(state) = &state {
                save_state(&path, state).unwrap();
            }
            assert_eq!(stored_notice(&current, &path).as_deref(), want, "{state:?}");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_failed_background_check_counts_as_the_days_check_and_keeps_the_saved_version() {
        let dir = scratch("background-failed");
        let path = dir.join("update-check.json");
        let saved = |checked_at| CheckState {
            checked_at,
            latest: Some("2.3.0".into()),
        };
        save_state(&path, &saved(0)).unwrap();
        let current = Version::parse("2.2.0").unwrap();
        let now = 2 * DAY_SECS;

        let answer = check_in_background(FakeSource(Err("offline")), current, path.clone(), now)
            .expect("a check is due")
            .recv()
            .unwrap();

        assert_eq!(answer, None);
        assert_eq!(load_state(&path), Some(saved(now)));
        assert!(!is_due(load_state(&path).as_ref(), now + 60));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_saved_notice_is_returned_and_no_check_starts_when_the_saved_one_is_not_due() {
        let dir = scratch("notice-not-due");
        let path = dir.join("update-check.json");
        save_state(
            &path,
            &CheckState {
                checked_at: 1_000,
                latest: Some("2.3.0".into()),
            },
        )
        .unwrap();
        let current = Version::parse("2.2.0").unwrap();

        let start = notice_start(current, path, 1_060, NeverAsked);

        assert!(start.saved.unwrap().contains("2.3.0"));
        assert!(start.check.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn with_no_saved_state_a_check_starts_and_its_answer_arrives_on_it() {
        let dir = scratch("notice-no-state");
        let path = dir.join("update-check.json");
        let current = Version::parse("2.2.0").unwrap();

        let start = notice_start(current, path.clone(), 5_000, FakeSource(Ok(Some("v2.3.0"))));
        let answer = start.check.expect("a check is due").recv().unwrap();

        assert_eq!(start.saved, None);
        assert!(answer.unwrap().contains("2.3.0"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    struct WaitingSource(mpsc::Receiver<()>);

    impl ReleaseSource for WaitingSource {
        fn latest(&self) -> Result<Option<Release>, String> {
            let _ = self.0.recv();
            Ok(None)
        }

        fn fetch(&self, url: &str, _max_bytes: u64, _out: &mut dyn Write) -> Result<(), String> {
            Err(format!("no download in this test: {url}"))
        }
    }

    #[test]
    fn a_background_check_saves_its_attempt_before_the_answer_comes() {
        let dir = scratch("background-attempt");
        let path = dir.join("update-check.json");
        let current = Version::parse("2.2.0").unwrap();
        let (release, waiting) = mpsc::channel();

        let receiver =
            check_in_background(WaitingSource(waiting), current, path.clone(), 5_000).unwrap();

        assert_eq!(
            load_state(&path),
            Some(CheckState {
                checked_at: 5_000,
                latest: None,
            })
        );
        release.send(()).unwrap();
        assert_eq!(receiver.recv().unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_newer_release_found_in_the_background_is_sent_and_saved() {
        let dir = scratch("background-newer");
        let path = dir.join("update-check.json");
        let current = Version::parse("2.2.0").unwrap();

        let answer = check_in_background(FakeSource(Ok(Some("v2.3.0"))), current, path.clone(), 7)
            .unwrap()
            .recv()
            .unwrap();

        assert!(answer.unwrap().contains("2.3.0"));
        assert_eq!(
            load_state(&path).and_then(|state| state.latest).as_deref(),
            Some("2.3.0")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_package_version_parses_so_notices_stay_on() {
        assert!(Version::parse(env!("CARGO_PKG_VERSION")).is_some());
    }

    use std::collections::HashMap;

    const NAME: &str = "ModFoundry-2.3.0-linux-x86_64";
    const NEW_MF: &str = "echo mf 2.3.0";
    const OLD_MF: &str = "echo mf 2.2.0";

    fn write_install(dir: &Path, mf_body: &str, skill: &str) {
        std::fs::create_dir_all(dir.join("skill").join("modfoundry")).unwrap();
        for program in PROGRAMS {
            let body = if program == "mf" { mf_body } else { "true" };
            write_script(&dir.join(program), body);
        }
        std::fs::write(dir.join("skill").join("modfoundry").join("SKILL.md"), skill).unwrap();
    }

    fn write_script(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        make_executable(path).unwrap();
    }

    /// A release whose `mf` runs `mf_body`, served with its checksum.
    fn served_release(root: &Path, mf_body: &str) -> (Release, Served) {
        served_release_without(root, mf_body, "")
    }

    fn served_release_without(root: &Path, mf_body: &str, missing: &str) -> (Release, Served) {
        let stage = root.join("stage");
        write_install(&stage.join(NAME), mf_body, "new skill");
        if !missing.is_empty() {
            std::fs::remove_file(stage.join(NAME).join(missing)).unwrap();
        }
        let archive = root.join(format!("{NAME}.tar.gz"));
        let status = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg(NAME)
            .current_dir(&stage)
            .status()
            .unwrap();
        assert!(status.success(), "tar could not pack the release");
        let bytes = std::fs::read(&archive).unwrap();
        let release = Release {
            version: Version::parse("2.3.0").unwrap(),
            tarball_url: format!("https://releases.test/{NAME}.tar.gz"),
            sha256_url: format!("https://releases.test/{NAME}.tar.gz.sha256"),
        };
        let checksum = format!("{}  {NAME}.tar.gz\n", modfoundry_vfs::sha256_hex(&bytes));
        let mut served = Served::default();
        served.0.insert(release.tarball_url.clone(), bytes);
        served
            .0
            .insert(release.sha256_url.clone(), checksum.into_bytes());
        (release, served)
    }

    #[derive(Default)]
    struct Served(HashMap<String, Vec<u8>>);

    impl ReleaseSource for Served {
        fn latest(&self) -> Result<Option<Release>, String> {
            Ok(None)
        }

        fn fetch(&self, url: &str, _max_bytes: u64, out: &mut dyn Write) -> Result<(), String> {
            let bytes = self
                .0
                .get(url)
                .ok_or_else(|| format!("no fixture for {url}"))?;
            out.write_all(bytes).map_err(|e| e.to_string())
        }
    }

    #[test]
    fn a_release_with_the_right_hash_replaces_the_install() {
        let root = scratch("install-good");
        let (release, source) = served_release(&root, NEW_MF);
        let lib = root.join("lib");
        write_install(&lib, OLD_MF, "old skill");
        std::fs::write(lib.join(".modfoundry-install"), "").unwrap();
        std::fs::create_dir_all(sibling(&lib, ".new")).unwrap();
        std::fs::create_dir_all(sibling(&lib, ".old")).unwrap();
        let current = Version::parse("2.2.0").unwrap();

        let installed =
            install_release(&lib, &release, &source, &root.join("work"), &current, None).unwrap();

        assert_eq!(
            installed,
            Installed {
                from: "2.2.0".into(),
                to: "2.3.0".into(),
            }
        );
        assert!(std::fs::read_to_string(lib.join("mf"))
            .unwrap()
            .contains(NEW_MF));
        assert!(lib.join(".modfoundry-install").is_file());
        assert_eq!(
            std::fs::read_to_string(lib.join("skill/modfoundry/SKILL.md")).unwrap(),
            "new skill"
        );
        assert!(!sibling(&lib, ".new").exists());
        assert!(!sibling(&lib, ".old").exists());
        assert!(!root.join("work").join(format!("{NAME}.tar.gz")).exists());
        assert!(!root.join("work").join("unpacked").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    struct TwoChunks {
        served: Served,
        archive_url: String,
        part: PathBuf,
    }

    impl ReleaseSource for TwoChunks {
        fn latest(&self) -> Result<Option<Release>, String> {
            Ok(None)
        }

        fn fetch(&self, url: &str, max_bytes: u64, out: &mut dyn Write) -> Result<(), String> {
            if url != self.archive_url {
                return self.served.fetch(url, max_bytes, out);
            }
            let bytes = &self.served.0[url];
            let (first, rest) = bytes.split_at(bytes.len() / 2);
            out.write_all(first).map_err(|e| e.to_string())?;
            let on_disk = std::fs::metadata(&self.part).unwrap().len();
            assert_eq!(on_disk, first.len() as u64);
            out.write_all(rest).map_err(|e| e.to_string())
        }
    }

    #[test]
    fn the_archive_downloads_into_a_part_file() {
        let root = scratch("install-part-file");
        let (release, served) = served_release(&root, NEW_MF);
        let work = root.join("work");
        let source = TwoChunks {
            served,
            archive_url: release.tarball_url.clone(),
            part: work.join(format!("{NAME}.tar.gz.part")),
        };
        let lib = root.join("lib");
        write_install(&lib, OLD_MF, "old skill");
        let current = Version::parse("2.2.0").unwrap();

        install_release(&lib, &release, &source, &work, &current, None).unwrap();

        assert!(std::fs::read_to_string(lib.join("mf"))
            .unwrap()
            .contains(NEW_MF));
        assert!(!work.join(format!("{NAME}.tar.gz.part")).exists());
        assert!(!work.join(format!("{NAME}.tar.gz")).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    struct DropsMidway {
        served: Served,
        archive_url: String,
    }

    impl ReleaseSource for DropsMidway {
        fn latest(&self) -> Result<Option<Release>, String> {
            Ok(None)
        }

        fn fetch(&self, url: &str, max_bytes: u64, out: &mut dyn Write) -> Result<(), String> {
            if url != self.archive_url {
                return self.served.fetch(url, max_bytes, out);
            }
            let bytes = &self.served.0[url];
            out.write_all(&bytes[..bytes.len() / 2])
                .map_err(|e| e.to_string())?;
            Err("connection dropped".into())
        }
    }

    #[test]
    fn a_download_that_fails_midway_leaves_nothing() {
        let root = scratch("install-dropped");
        let (release, served) = served_release(&root, NEW_MF);
        let source = DropsMidway {
            served,
            archive_url: release.tarball_url.clone(),
        };
        let lib = root.join("lib");
        write_install(&lib, OLD_MF, "old skill");
        let work = root.join("work");
        let current = Version::parse("2.2.0").unwrap();

        let error = install_release(&lib, &release, &source, &work, &current, None).unwrap_err();

        assert_eq!(error, "connection dropped");
        assert!(std::fs::read_to_string(lib.join("mf"))
            .unwrap()
            .contains(OLD_MF));
        assert!(!work.join(format!("{NAME}.tar.gz.part")).exists());
        assert!(!work.join(format!("{NAME}.tar.gz")).exists());
        assert!(!work.join("unpacked").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_checksum_mismatch_touches_nothing() {
        let root = scratch("install-wrong-hash");
        let (release, mut source) = served_release(&root, NEW_MF);
        source.0.insert(
            release.sha256_url.clone(),
            format!("{}  {NAME}.tar.gz\n", "0".repeat(64)).into_bytes(),
        );
        let lib = root.join("lib");
        write_install(&lib, OLD_MF, "old skill");
        let work = root.join("work");
        let current = Version::parse("2.2.0").unwrap();

        let error = install_release(&lib, &release, &source, &work, &current, None).unwrap_err();

        assert!(error.starts_with("checksum of"), "{error}");
        assert!(std::fs::read_to_string(lib.join("mf"))
            .unwrap()
            .contains(OLD_MF));
        assert!(!work.join(format!("{NAME}.tar.gz")).exists());
        assert!(!work.join(format!("{NAME}.tar.gz.part")).exists());
        assert!(!work.join("unpacked").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_new_mf_that_exits_with_an_error_puts_the_old_install_back() {
        let root = scratch("install-broken");
        let (release, source) = served_release(&root, "exit 1");
        let lib = root.join("lib");
        write_install(&lib, OLD_MF, "old skill");
        let current = Version::parse("2.2.0").unwrap();

        let error = install_release(&lib, &release, &source, &root.join("work"), &current, None)
            .unwrap_err();

        assert!(error.contains("the old version was kept"), "{error}");
        assert!(std::fs::read_to_string(lib.join("mf"))
            .unwrap()
            .contains(OLD_MF));
        assert!(!sibling(&lib, ".new").exists());
        assert!(!sibling(&lib, ".old").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_agents_skill_follows_only_while_it_is_the_old_copy() {
        let root = scratch("agents-skill");
        let (release, source) = served_release(&root, NEW_MF);
        let current = Version::parse("2.2.0").unwrap();
        let agents = root.join("agents").join("SKILL.md");
        std::fs::create_dir_all(agents.parent().unwrap()).unwrap();
        let cases = [
            ("old skill", "new skill"),
            ("edited by hand", "edited by hand"),
        ];
        for (index, (before, after)) in cases.into_iter().enumerate() {
            let lib = root.join(format!("lib-{index}"));
            write_install(&lib, OLD_MF, "old skill");
            std::fs::write(&agents, before).unwrap();

            install_release(
                &lib,
                &release,
                &source,
                &root.join("work"),
                &current,
                Some(&agents),
            )
            .unwrap();

            assert_eq!(std::fs::read_to_string(&agents).unwrap(), after, "{before}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_agents_skill_that_is_a_link_is_left_alone() {
        let root = scratch("agents-link");
        let (release, source) = served_release(&root, NEW_MF);
        let lib = root.join("lib");
        write_install(&lib, OLD_MF, "old skill");
        let target = root.join("repo").join("SKILL.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "old skill").unwrap();
        let agents = root.join("agents").join("SKILL.md");
        std::fs::create_dir_all(agents.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &agents).unwrap();
        let current = Version::parse("2.2.0").unwrap();

        install_release(
            &lib,
            &release,
            &source,
            &root.join("work"),
            &current,
            Some(&agents),
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old skill");
        assert!(agents.symlink_metadata().unwrap().file_type().is_symlink());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_asset_served_from_another_place_is_refused() {
        let foreign = LATEST.replace(
            "https://github.com/HellboundGlory/ModFoundry-DayZ/releases/download/",
            "https://example.com/",
        );
        let Err(error) = parse_latest(&foreign) else {
            panic!("a foreign download link must be refused");
        };
        assert!(
            error.starts_with("release asset ModFoundry-2.2.0-linux-x86_64.tar.gz"),
            "{error}"
        );
    }

    #[test]
    fn a_checksum_that_names_another_file_is_refused() {
        let root = scratch("install-checksum-name");
        let (release, mut source) = served_release(&root, NEW_MF);
        let named = String::from_utf8(source.0[&release.sha256_url].clone())
            .unwrap()
            .replace(NAME, "Other");
        source
            .0
            .insert(release.sha256_url.clone(), named.into_bytes());
        let lib = root.join("lib");
        write_install(&lib, OLD_MF, "old skill");
        let current = Version::parse("2.2.0").unwrap();

        let error = install_release(&lib, &release, &source, &root.join("work"), &current, None)
            .unwrap_err();

        assert!(
            error.starts_with("the checksum file names Other.tar.gz"),
            "{error}"
        );
        assert!(std::fs::read_to_string(lib.join("mf"))
            .unwrap()
            .contains(OLD_MF));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_refused_release_leaves_no_download_behind() {
        let root = scratch("install-leftovers");
        let (release, source) = served_release_without(&root, NEW_MF, "modfoundry-mcp-server");
        let lib = root.join("lib");
        write_install(&lib, OLD_MF, "old skill");
        let work = root.join("work");
        let current = Version::parse("2.2.0").unwrap();

        let error = install_release(&lib, &release, &source, &work, &current, None).unwrap_err();

        assert_eq!(error, "the release has no modfoundry-mcp-server");
        assert!(!work.join(format!("{NAME}.tar.gz")).exists());
        assert!(!work.join("unpacked").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_second_update_at_the_same_time_is_refused() {
        let root = scratch("install-locked");
        let (release, source) = served_release(&root, NEW_MF);
        let lib = root.join("lib");
        write_install(&lib, OLD_MF, "old skill");
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let held = std::fs::File::create(work.join(".lock")).unwrap();
        held.lock().unwrap();
        let current = Version::parse("2.2.0").unwrap();

        let error = install_release(&lib, &release, &source, &work, &current, None).unwrap_err();

        assert_eq!(error, "another mf update is running");
        assert!(std::fs::read_to_string(lib.join("mf"))
            .unwrap()
            .contains(OLD_MF));
        drop(held);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_agents_skill_in_a_linked_folder_is_left_alone() {
        let root = scratch("skill-linked-folder");
        let real = root.join("checkout/modfoundry");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("SKILL.md"), "old skill").unwrap();
        let skills = root.join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        std::os::unix::fs::symlink(&real, skills.join("modfoundry")).unwrap();
        let old = root.join("old.md");
        let new = root.join("new.md");
        std::fs::write(&old, "old skill").unwrap();
        std::fs::write(&new, "new skill").unwrap();

        refresh_agents_skill(&skills.join("modfoundry/SKILL.md"), &old, &new);

        assert_eq!(
            std::fs::read_to_string(real.join("SKILL.md")).unwrap(),
            "old skill"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_new_mf_that_hangs_is_given_up_on() {
        let root = scratch("version-hang");
        std::fs::create_dir_all(&root).unwrap();
        let mf = root.join("mf");
        write_script(&mf, "exec sleep 30");
        let started = std::time::Instant::now();

        let error = check_version(
            &mf,
            &Version::parse("2.3.0").unwrap(),
            Duration::from_millis(300),
        )
        .unwrap_err();

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(error.starts_with("it did not answer"), "{error}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_update_target_is_refused_outside_a_release_install_or_when_switched_off() {
        let lib = PathBuf::from("/opt/mf");
        let cases = [
            (
                None,
                Some("1"),
                Err("this mf was not installed from a release package"),
            ),
            (
                Some(lib.clone()),
                Some("1"),
                Err("MF_NO_UPDATE_CHECK is set"),
            ),
            (Some(lib.clone()), None, Ok(lib.clone())),
        ];
        for (install, switch, want) in cases {
            let env = |key: &str| {
                assert_eq!(key, "MF_NO_UPDATE_CHECK");
                switch.map(String::from)
            };
            assert_eq!(update_target(install, env), want, "{switch:?}");
        }
    }

    struct NeverAsked;

    impl ReleaseSource for NeverAsked {
        fn latest(&self) -> Result<Option<Release>, String> {
            panic!("the release source was asked")
        }

        fn fetch(&self, _url: &str, _max_bytes: u64, _out: &mut dyn Write) -> Result<(), String> {
            panic!("the release source was asked")
        }
    }

    #[test]
    fn run_check_refuses_an_unreadable_version_before_asking_the_source() {
        let path = scratch("check-unreadable").join("update-check.json");
        let error = run_check(&NeverAsked, "junk", 1_000, &path).unwrap_err();
        assert_eq!(error, "unreadable version junk");
    }

    #[test]
    fn run_update_keeps_the_install_when_no_newer_release_exists() {
        let dir = scratch("update-not-newer");
        let lib = dir.join("lib");
        let work = dir.join("work");
        std::fs::create_dir_all(&lib).unwrap();
        let source = FakeSource(Ok(Some("v2.2.0")));
        let updated = run_update(&source, "2.2.0", &lib, &work, None).unwrap();
        assert_eq!(
            updated,
            Updated::NotNewer {
                latest: Some("2.2.0".into())
            }
        );
        assert_eq!(std::fs::read_dir(&lib).unwrap().count(), 0);
        assert!(!work.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_update_says_when_no_release_is_published() {
        let dir = scratch("update-no-release");
        let updated = run_update(
            &FakeSource(Ok(None)),
            "2.2.0",
            &dir.join("lib"),
            &dir.join("work"),
            None,
        )
        .unwrap();
        assert_eq!(updated, Updated::NotNewer { latest: None });
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct Offered(Release, Served);

    impl ReleaseSource for Offered {
        fn latest(&self) -> Result<Option<Release>, String> {
            Ok(Some(self.0.clone()))
        }

        fn fetch(&self, url: &str, max_bytes: u64, out: &mut dyn Write) -> Result<(), String> {
            self.1.fetch(url, max_bytes, out)
        }
    }

    #[test]
    fn run_update_installs_a_newer_release() {
        let root = scratch("update-newer");
        let (release, served) = served_release(&root, NEW_MF);
        let lib = root.join("lib");
        write_install(&lib, OLD_MF, "old skill");
        std::fs::write(lib.join(".modfoundry-install"), "").unwrap();

        let installed = run_update(
            &Offered(release, served),
            "2.2.0",
            &lib,
            &root.join("work"),
            None,
        )
        .unwrap();

        assert_eq!(
            installed,
            Updated::Installed(Installed {
                from: "2.2.0".into(),
                to: "2.3.0".into(),
            })
        );
        assert!(std::fs::read_to_string(lib.join("mf"))
            .unwrap()
            .contains(NEW_MF));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn run_update_refuses_an_unreadable_version_before_asking_the_source() {
        let dir = scratch("update-unreadable");
        let error = run_update(
            &NeverAsked,
            "junk",
            &dir.join("lib"),
            &dir.join("work"),
            None,
        )
        .unwrap_err();
        assert_eq!(error, "unreadable version junk");
    }

    const BUILD_RELEASE: &str = include_str!("../../../packaging/linux/build-release.sh");
    const INSTALL: &str = include_str!("../../../packaging/linux/install.sh");

    fn words_between<'a>(text: &'a str, start: &str, end: &str) -> Option<Vec<&'a str>> {
        let (_, rest) = text.split_once(start)?;
        let (words, _) = rest.split_once(end)?;
        Some(words.split_whitespace().collect())
    }

    #[test]
    fn the_release_scripts_list_the_programs_the_updater_installs() {
        let build = words_between(BUILD_RELEASE, "for program in ", "; do")
            .expect("build-release.sh has no `for program in ...; do` loop");
        let install = words_between(INSTALL, "programs=(", ")")
            .expect("install.sh has no `programs=(...)` list");
        assert_eq!(build, PROGRAMS, "build-release.sh");
        assert_eq!(install, PROGRAMS, "install.sh");
    }
}
