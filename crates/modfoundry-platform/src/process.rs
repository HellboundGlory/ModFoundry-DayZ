//! Host process control: detecting and shutting down the DayZ client a run
//! launched.
//!
//! Steam owns the client process, so a run that ends normally never touches it
//! (only an explicit stop closes the client).

/// Terminate running DayZ client processes, returning how many were signalled.
///
/// Linux scans the `/proc` process table; Windows shells out to `taskkill`. A
/// client that is already gone counts zero and is not an error.
pub fn stop_dayz_client() -> std::io::Result<usize> {
    platform_stop()
}

/// Which DayZ process a process is, when it is one at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayzRole {
    /// The client, retail or Diag.
    Client,
    /// The dedicated server, native, retail or Diag.
    Server,
}

/// Only the game itself counts: under Proton the Steam runtime and Wine wrappers carry the same
/// argv (`DayZ*_x64.exe ...`) and outlive the game by a while, so they must not look like it.
fn is_game_process_name(comm: &str) -> bool {
    comm.is_empty()
        || comm == "enfmain"
        || comm.starts_with("wine")
        || matches!(
            comm,
            "dayz_x64.exe" | "dayzdiag_x64.exe" | "dayzserver" | "dayzserver_x64.exe"
        )
}

/// Classify a process by its name and argv; `None` for anything that is neither
/// a DayZ client nor a server (Steam, the BattlEye stub, the empty process).
pub fn classify_dayz_process(comm: &str, args: &[&str]) -> Option<DayzRole> {
    classify(comm, args).map(|(role, _)| role)
}

/// The role, and whether the process is the Diag executable.
fn classify(comm: &str, args: &[&str]) -> Option<(DayzRole, bool)> {
    let comm = comm.to_ascii_lowercase();
    if !is_game_process_name(&comm) {
        return None;
    }
    classify_argv(&comm, args, true)
}

/// The names of the Proton wrappers that carry the game's argv without being it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const WRAPPER_NAMES: [&str; 4] = ["srt-bwrap", "pv-adverb", "python3", "steam.exe"];

/// The role and Diag flag a wrapper's argv names, for a process whose name is one of
/// [`WRAPPER_NAMES`].
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn classify_wrapper(comm: &str, args: &[&str]) -> Option<(DayzRole, bool)> {
    let comm = comm.to_ascii_lowercase();
    if !WRAPPER_NAMES.contains(&comm.as_str()) {
        return None;
    }
    classify_argv(&comm, args, false)
}

/// The role and Diag flag the argv and a lowercased process name give. `native_server` lets a
/// bare `DayZServer` path count; a wrapper's argv holds bind-mounted folders of that name.
fn classify_argv(comm: &str, args: &[&str], native_server: bool) -> Option<(DayzRole, bool)> {
    if comm.ends_with("dayzserver") || comm.ends_with("dayzserver_x64.exe") {
        return Some((DayzRole::Server, false));
    }
    let mut client = comm == "enfmain";
    let mut diag = false;
    let mut server_flag = false;
    for arg in args {
        let lower = arg.to_ascii_lowercase();
        if (native_server && lower.ends_with("dayzserver")) || lower.ends_with("dayzserver_x64.exe")
        {
            return Some((DayzRole::Server, false));
        }
        if lower.ends_with("dayz_be.exe") {
            return None;
        }
        diag |= lower.ends_with("dayzdiag_x64.exe");
        client |= lower.ends_with("dayz_x64.exe") || lower.ends_with("dayzdiag_x64.exe");
        server_flag |= lower == "-server";
    }
    if diag && server_flag {
        return Some((DayzRole::Server, true));
    }
    client.then_some((DayzRole::Client, diag))
}

/// Terminate running DayZ Diag client processes and their Proton wrappers, returning how many
/// games were signalled. Never touches a retail client or any server.
pub fn stop_diag_client() -> std::io::Result<usize> {
    platform_stop_diag_client()
}

/// Terminate running DayZ Diag server processes, returning how many were
/// signalled. Never touches the native retail server.
pub fn stop_diag_server() -> std::io::Result<usize> {
    platform_stop_diag()
}

/// Whether a DayZ Diag server process currently exists, without signalling it.
#[cfg(windows)]
pub fn diag_server_running() -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq DayZDiag_x64.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("DayZDiag_x64.exe"))
        .unwrap_or(false)
}

/// Whether a DayZ Diag server process currently exists, without signalling it.
#[cfg(not(windows))]
pub fn diag_server_running() -> bool {
    any_running(|p| p.role == DayzRole::Server && p.diag)
}

#[cfg(windows)]
fn platform_stop() -> std::io::Result<usize> {
    use std::process::Command;
    let status = Command::new("taskkill")
        .args(["/IM", "DayZ_x64.exe", "/T", "/F"])
        .status()?;
    if status.success() {
        Ok(1)
    } else {
        Ok(0)
    }
}

#[cfg(windows)]
fn platform_stop_diag() -> std::io::Result<usize> {
    use std::process::Command;
    let status = Command::new("taskkill")
        .args(["/IM", "DayZDiag_x64.exe", "/T", "/F"])
        .status()?;
    if status.success() {
        Ok(1)
    } else {
        Ok(0)
    }
}

#[cfg(not(windows))]
fn platform_stop() -> std::io::Result<usize> {
    let (games, pids) = games_and_pids(&dayz_processes()?, |p| p.role == DayzRole::Client);
    stop_pids(&pids);
    Ok(games)
}

#[cfg(not(windows))]
fn is_diag_client(p: &DayzProcess) -> bool {
    p.role == DayzRole::Client && p.diag
}

#[cfg(not(windows))]
fn platform_stop_diag_client() -> std::io::Result<usize> {
    let (games, pids) = games_and_pids(&dayz_processes()?, is_diag_client);
    stop_pids(&pids);
    Ok(games)
}

/// The Diag client and server share one image name here, so only the server stop can end it.
#[cfg(windows)]
fn platform_stop_diag_client() -> std::io::Result<usize> {
    Ok(0)
}

#[cfg(not(windows))]
fn platform_stop_diag() -> std::io::Result<usize> {
    let (games, pids) =
        games_and_pids(&dayz_processes()?, |p| p.role == DayzRole::Server && p.diag);
    stop_pids(&pids);
    Ok(games)
}

#[cfg(not(windows))]
fn stop_pids(pids: &[u32]) -> usize {
    for pid in pids {
        signal(*pid, Signal::Term);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline && pids.iter().any(|p| alive(*p)) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    for pid in pids.iter().filter(|p| alive(**p)) {
        signal(*pid, Signal::Kill);
    }
    pids.len()
}

/// End `pid` and every descendant a process table finds through their parent pids, returning
/// how many were signalled. A wrapper chain that started its own session is found this way, not
/// with a process-group signal; a descendant already orphaned by an earlier exit is not found.
pub fn stop_process_tree(pid: u32) -> usize {
    // 0 would signal our own process group.
    if pid == 0 {
        return 0;
    }
    platform_stop_tree(pid)
}

/// Make the child `command` spawns end when the thread that spawns it ends, so a run does not
/// outlive the process that started it. Only the direct child gets the signal; anything it
/// started is ended by the next explicit stop.
pub fn end_with_parent(command: &mut std::process::Command) {
    platform_end_with_parent(command)
}

#[cfg(target_os = "linux")]
fn platform_end_with_parent(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    let parent = std::process::id() as libc::pid_t;
    // The closure runs between fork and exec, so it makes only async-signal-safe calls.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            Ok(())
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn platform_end_with_parent(_command: &mut std::process::Command) {}

#[cfg(target_os = "linux")]
fn platform_stop_tree(pid: u32) -> usize {
    stop_pids(&descendants(pid, &process_table()))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn platform_stop_tree(pid: u32) -> usize {
    stop_pids(&[pid])
}

#[cfg(windows)]
fn platform_stop_tree(pid: u32) -> usize {
    let status = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
    matches!(status, Ok(status) if status.success()) as usize
}

/// Every `(pid, ppid)` pair in `/proc`.
#[cfg(target_os = "linux")]
fn process_table() -> Vec<(u32, u32)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            Some((pid, parent_of(&stat)?))
        })
        .collect()
}

/// The parent pid in a `/proc/<pid>/stat` line: the field after the state, and the name before it
/// may itself hold spaces and parentheses, so the name ends at the last `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parent_of(stat: &str) -> Option<u32> {
    field_after_name(stat, 1)?.parse().ok()
}

/// The `nth` whitespace-separated field after the process name of a `/proc/<pid>/stat` line.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn field_after_name(stat: &str, nth: usize) -> Option<&str> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(nth)
}

/// `root` and every pid below it in `table`, each once; a cycle in the table still ends.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn descendants(root: u32, table: &[(u32, u32)]) -> Vec<u32> {
    let mut found = vec![root];
    let mut next = 0;
    while next < found.len() {
        let parent = found[next];
        for (pid, ppid) in table {
            if *ppid == parent && !found.contains(pid) {
                found.push(*pid);
            }
        }
        next += 1;
    }
    found
}

#[cfg(not(windows))]
struct DayzProcess {
    pid: u32,
    role: DayzRole,
    diag: bool,
    /// A Proton wrapper carrying the game's argv, not the game.
    wrapper: bool,
}

#[cfg(not(windows))]
fn dayz_processes() -> std::io::Result<Vec<DayzProcess>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        let comm = comm.trim();
        let (role, diag, wrapper) = if let Some((role, diag)) = classify(comm, &refs) {
            (role, diag, false)
        } else if let Some((role, diag)) = classify_wrapper(comm, &refs) {
            (role, diag, true)
        } else {
            continue;
        };
        found.push(DayzProcess {
            pid,
            role,
            diag,
            wrapper,
        });
    }
    Ok(found)
}

/// The pids of a role's game processes and its wrappers, and how many of them are the game itself.
#[cfg(not(windows))]
fn games_and_pids(
    processes: &[DayzProcess],
    want: impl Fn(&DayzProcess) -> bool,
) -> (usize, Vec<u32>) {
    let matching: Vec<&DayzProcess> = processes.iter().filter(|p| want(p)).collect();
    let games = matching.iter().filter(|p| !p.wrapper).count();
    (games, matching.iter().map(|p| p.pid).collect())
}

/// The pid of a role's game process, never one of its wrappers.
#[cfg(not(windows))]
fn game_pid(processes: &[DayzProcess], role: DayzRole) -> Option<u32> {
    processes
        .iter()
        .find(|p| p.role == role && !p.wrapper)
        .map(|p| p.pid)
}

/// The pid of the running DayZ client game, if there is one.
#[cfg(not(windows))]
pub fn dayz_client_pid() -> Option<u32> {
    game_pid(&dayz_processes().ok()?, DayzRole::Client)
}

/// Windows cannot tell the game's pid here; callers fall back to their own.
#[cfg(windows)]
pub fn dayz_client_pid() -> Option<u32> {
    None
}

/// Whether a game process of a role exists; a wrapper never counts as one.
#[cfg(not(windows))]
fn any_running(want: impl Fn(&DayzProcess) -> bool) -> bool {
    dayz_processes().is_ok_and(|processes| games_and_pids(&processes, want).0 > 0)
}

/// Whether a DayZ client process currently exists, without signalling it.
#[cfg(windows)]
pub fn dayz_client_running() -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq DayZ_x64.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("DayZ_x64.exe"))
        .unwrap_or(false)
}

/// Whether a DayZ client process currently exists, without signalling it.
#[cfg(not(windows))]
pub fn dayz_client_running() -> bool {
    any_running(|p| p.role == DayzRole::Client)
}

/// The two signals a stop sends: a polite one, then one that cannot be ignored.
#[cfg(not(windows))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    Term,
    Kill,
}

/// The pid as `kill(2)` reads it, or `None` when it would not name one process: a value of 0 or
/// less means a process group, and one too large for `pid_t` does not fit at all.
#[cfg(not(windows))]
fn signal_pid(pid: u32) -> Option<libc::pid_t> {
    let pid = libc::pid_t::try_from(pid).ok()?;
    (pid > 0).then_some(pid)
}

#[cfg(not(windows))]
fn signal(pid: u32, sig: Signal) {
    let sig = match sig {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // A pid that has already gone is the goal, so the result says nothing and prints nothing.
    if let Some(pid) = signal_pid(pid) {
        unsafe {
            libc::kill(pid, sig);
        }
    }
}

/// A zombie is a child that exited but has not been reaped, so it reads as gone.
#[cfg(target_os = "linux")]
pub(crate) fn alive(pid: u32) -> bool {
    let stat = std::fs::read_to_string(
        std::path::Path::new("/proc")
            .join(pid.to_string())
            .join("stat"),
    );
    match stat {
        Ok(stat) => field_after_name(&stat, 0) != Some("Z"),
        Err(_) => false,
    }
}

#[cfg(windows)]
pub(crate) fn alive(pid: u32) -> bool {
    let Ok(output) = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
    else {
        return true;
    };
    // An unanswered check must read as alive, or a live run's staging is removed.
    if !output.status.success() {
        return true;
    }
    let wanted = pid.to_string();
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        line.split(|c: char| c == ',' || c.is_whitespace())
            .any(|field| field.trim_matches('"') == wanted)
    })
}

/// Elsewhere nothing can tell, so every run reads as alive and none of its staging is removed.
#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{
        classify, classify_dayz_process, classify_wrapper, descendants, parent_of, DayzRole,
    };

    #[test]
    fn a_bind_mounted_server_folder_does_not_make_a_client_wrapper_a_server() {
        let argv = [
            "srt-bwrap",
            "--bind",
            "/srv/DayZServer",
            "/srv/DayZServer",
            "/x/DayZ/DayZDiag_x64.exe",
            "-connect=127.0.0.1",
        ];
        assert_eq!(
            classify_wrapper("srt-bwrap", &argv),
            Some((DayzRole::Client, true))
        );
    }

    #[test]
    fn proton_wrapper_processes_are_never_the_game() {
        let argv = [
            "/x/DayZ/DayZDiag_x64.exe",
            "-mod=@X",
            "-connect=127.0.0.1:2302",
        ];
        for comm in ["srt-bwrap", "pv-adverb", "python3", "steam.exe", "bash"] {
            assert_eq!(classify_dayz_process(comm, &argv), None, "{comm}");
        }
        let server = ["/x/DayZ/DayZDiag_x64.exe", "-server", "-filePatching"];
        for comm in ["srt-bwrap", "pv-adverb", "python3", "steam.exe"] {
            assert_eq!(classify_dayz_process(comm, &server), None, "{comm}");
        }
    }

    #[test]
    fn a_stat_line_names_its_parent_after_the_last_parenthesis() {
        let cases: &[(&str, Option<u32>, &str)] = &[
            ("812 (sh) S 77 812 812 0 -1", Some(77), "a plain line"),
            (
                "812 (a) b) S 77 812 812 0 -1",
                Some(77),
                "a name with a space and a parenthesis",
            ),
            (
                "812 no closing parenthesis S 77",
                None,
                "no closing parenthesis",
            ),
        ];
        for (stat, expected, why) in cases {
            assert_eq!(parent_of(stat), *expected, "{why}");
        }
    }

    #[test]
    fn descendants_of_a_root_are_found_through_the_table() {
        type Case<'a> = (&'a str, u32, &'a [(u32, u32)], &'a [u32]);
        let chain = [(10, 1), (11, 10), (12, 11)];
        let fork = [(10, 1), (11, 10), (12, 10)];
        let with_stranger = [(10, 1), (11, 10), (99, 1)];
        let cycle = [(10, 12), (11, 10), (12, 11)];
        let cases: &[Case] = &[
            ("a chain", 10, &chain, &[10, 11, 12]),
            ("a fork", 10, &fork, &[10, 11, 12]),
            (
                "an unrelated process left out",
                10,
                &with_stranger,
                &[10, 11],
            ),
            ("a cycle still ends", 10, &cycle, &[10, 11, 12]),
        ];
        for (why, root, table, expected) in cases {
            let mut got = descendants(*root, table);
            got.sort_unstable();
            assert_eq!(got, *expected, "{why}");
        }
        assert!(
            !descendants(10, &[(99, 1), (98, 99)]).contains(&99),
            "a process off the root's tree is left out"
        );
        let mut repeated = descendants(10, &[(10, 1), (11, 10), (11, 10)]);
        repeated.sort_unstable();
        assert_eq!(repeated, [10, 11], "each pid appears once");
    }

    #[test]
    fn a_wrapper_is_classified_by_its_argv_and_only_for_its_own_names() {
        use DayzRole::{Client, Server};
        let client_argv = ["/x/DayZ/DayZDiag_x64.exe", "-filePatching"];
        let server_argv = ["/x/DayZ/DayZDiag_x64.exe", "-server", "-filePatching"];
        for name in [
            "srt-bwrap",
            "pv-adverb",
            "python3",
            "steam.exe",
            "SRT-BWRAP",
        ] {
            assert_eq!(
                classify_wrapper(name, &client_argv),
                Some((Client, true)),
                "{name}"
            );
            assert_eq!(
                classify_wrapper(name, &server_argv),
                Some((Server, true)),
                "{name}"
            );
        }
        for name in ["reaper", "bash", "enfMain", ""] {
            assert_eq!(classify_wrapper(name, &client_argv), None, "{name}");
            assert_eq!(classify_wrapper(name, &server_argv), None, "{name}");
        }
    }

    #[cfg(target_os = "linux")]
    fn grandchild_script(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let script = dir.join("launcher.sh");
        let record = dir.join("grandchild.pid");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n(n=0; while [ -e \"$0\" ] && [ $n -lt 150 ]; do sleep 0.2; n=$((n+1)); done) &\necho $! > '{}'\nn=0; while [ -e \"$0\" ] && [ $n -lt 150 ]; do sleep 0.2; n=$((n+1)); done\n",
                record.display()
            ),
        )
        .unwrap();
        (script, record)
    }

    #[cfg(target_os = "linux")]
    fn wait_for_pid_file(record: &std::path::Path) -> u32 {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match std::fs::read_to_string(record)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                Some(pid) => return pid,
                None => assert!(std::time::Instant::now() < deadline, "no grandchild pid"),
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stopping_a_process_tree_ends_its_grandchild_too() {
        let _lock = crate::spawn_lock();
        let dir = std::env::temp_dir().join(format!(
            "mf-platform-tree-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let (script, record) = grandchild_script(&dir);
        let mut child = std::process::Command::new("/bin/sh")
            .arg(&script)
            .spawn()
            .unwrap();
        let grandchild = wait_for_pid_file(&record);

        let signalled = super::stop_process_tree(child.id());
        assert!(signalled >= 2, "the child and its grandchild: {signalled}");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while super::alive(grandchild) {
            assert!(
                std::time::Instant::now() < deadline,
                "the grandchild {grandchild} outlived the tree stop"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_zombie_child_reads_as_gone() {
        let _lock = crate::spawn_lock();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        let stat_path = std::path::Path::new("/proc")
            .join(pid.to_string())
            .join("stat");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let stat = std::fs::read_to_string(&stat_path).unwrap_or_default();
            let state = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned));
            if state.as_deref() == Some("Z") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the child never became a zombie: {stat:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!super::alive(pid), "a zombie must read as gone");
        let _ = child.wait();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_child_marked_to_end_with_its_parent_dies_when_that_thread_returns() {
        let _lock = crate::spawn_lock();
        let spawn_in_thread = |with_parent: bool| {
            std::thread::spawn(move || {
                let mut command = std::process::Command::new("sleep");
                command.arg("30").stdin(std::process::Stdio::null());
                if with_parent {
                    super::end_with_parent(&mut command);
                }
                command.spawn().unwrap()
            })
            .join()
            .unwrap()
        };

        let mut marked = spawn_in_thread(true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while super::alive(marked.id()) {
            assert!(
                std::time::Instant::now() < deadline,
                "a child marked to end with its parent outlived the spawning thread"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = marked.wait();

        let mut plain = spawn_in_thread(false);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            super::alive(plain.id()),
            "a child not marked to end with its parent must keep running"
        );
        let _ = plain.kill();
        let _ = plain.wait();
    }

    #[cfg(not(windows))]
    #[test]
    fn a_pid_that_would_reach_a_group_is_never_signalled() {
        assert_eq!(super::signal_pid(0), None, "0 signals our own group");
        assert_eq!(
            super::signal_pid(u32::MAX),
            None,
            "a value too large for pid_t"
        );
        assert_eq!(super::signal_pid(1), Some(1));
        let above_group_range = u32::MAX / 2 + 1;
        assert_eq!(super::signal_pid(above_group_range), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_signal_ends_a_live_child_and_a_pid_that_has_gone_is_ignored() {
        use std::os::unix::process::ExitStatusExt;
        let _lock = crate::spawn_lock();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();

        super::signal(pid, super::Signal::Term);

        let status = child.wait().unwrap();
        assert_eq!(
            status.signal(),
            Some(libc::SIGTERM),
            "a live child must end on the term signal"
        );

        // A reaped pid can belong to another process, so the gone case asks about one no
        // process can have: the call is the goal, not an error.
        let beyond = impossible_pid();
        assert!(beyond > 1, "a pid no process can have: {beyond}");
        super::signal(beyond, super::Signal::Kill);
        assert!(
            !std::path::Path::new("/proc")
                .join(beyond.to_string())
                .exists(),
            "no process may take the impossible pid"
        );
    }

    #[cfg(target_os = "linux")]
    fn impossible_pid() -> u32 {
        let pid_max = std::fs::read_to_string("/proc/sys/kernel/pid_max")
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .unwrap_or(4_194_304);
        pid_max.saturating_add(1)
    }

    #[cfg(not(windows))]
    #[test]
    fn a_diag_client_stop_leaves_a_retail_client_and_every_server_alone() {
        let process = |pid, role, diag, wrapper| super::DayzProcess {
            pid,
            role,
            diag,
            wrapper,
        };
        let processes = [
            process(1, DayzRole::Client, true, false),
            process(2, DayzRole::Client, true, true),
            process(3, DayzRole::Client, false, false),
            process(4, DayzRole::Client, false, true),
            process(5, DayzRole::Server, true, false),
        ];

        let (games, pids) = super::games_and_pids(&processes, super::is_diag_client);

        assert_eq!(pids, [1, 2], "only the Diag client and its wrapper");
        assert_eq!(games, 1);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_stop_signals_a_roles_wrappers_but_counts_only_the_game() {
        let process = |pid, role, diag, wrapper| super::DayzProcess {
            pid,
            role,
            diag,
            wrapper,
        };
        let processes = [
            process(1, DayzRole::Client, true, false),
            process(2, DayzRole::Client, true, true),
            process(3, DayzRole::Server, false, false),
            process(4, DayzRole::Server, true, true),
        ];

        let (games, pids) = super::games_and_pids(&processes, |p| p.role == DayzRole::Client);
        assert_eq!(pids, [1, 2], "the game and its wrapper are both signalled");
        assert_eq!(games, 1, "a wrapper never counts as the game");

        let (games, pids) =
            super::games_and_pids(&processes, |p| p.role == DayzRole::Server && p.diag);
        assert_eq!(pids, [4]);
        assert_eq!(
            games, 0,
            "a Diag server wrapper is not a running Diag server"
        );

        assert_eq!(
            super::games_and_pids(&processes, |p| p.role == DayzRole::Server).0,
            1,
            "a wrapper is not the game whichever role it carries"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn a_roles_game_pid_skips_its_wrappers() {
        let process = |pid, role, wrapper| super::DayzProcess {
            pid,
            role,
            diag: false,
            wrapper,
        };
        let processes = [
            process(2, DayzRole::Client, true),
            process(3, DayzRole::Server, false),
            process(5, DayzRole::Client, false),
        ];

        assert_eq!(super::game_pid(&processes, DayzRole::Client), Some(5));
        assert_eq!(super::game_pid(&processes[..2], DayzRole::Client), None);
    }

    #[test]
    fn processes_classify_by_name_and_argv() {
        use DayzRole::{Client, Server};
        type Case<'a> = (&'a str, &'a [&'a str], Option<(DayzRole, bool)>, &'a str);
        let cases: &[Case] = &[
            (
                "enfMain",
                &["/home/u/.steam/steamapps/common/DayZServer/DayZServer"],
                Some((Server, false)),
                "a native server",
            ),
            (
                "enfMain",
                &["Z:\\DayZ\\DayZServer_x64.exe"],
                Some((Server, false)),
                "a native server under Wine",
            ),
            (
                "enfMain",
                &["Z:\\DayZ\\DayZDiag_x64.exe", "-server", "-filePatching"],
                Some((Server, true)),
                "a Diag server",
            ),
            (
                "enfMain",
                &[
                    "Z:\\DayZ\\DayZDiag_x64.exe",
                    "-filePatching",
                    "-connect=127.0.0.1",
                ],
                Some((Client, true)),
                "a Diag client without the server flag",
            ),
            (
                "enfMain",
                &["Z:\\DayZ\\DayZ_x64.exe"],
                Some((Client, false)),
                "a retail client",
            ),
            (
                "enfMain",
                &[],
                Some((Client, false)),
                "the engine with no argv",
            ),
            (
                "enfMain",
                &["Z:\\DayZ\\DayZDiag_x64.exe", "-SERVER"],
                Some((Server, true)),
                "the server flag in any case",
            ),
            (
                "enfMain",
                &["Z:\\DayZ\\DayZDiag_x64.exe", "-serverMod=@X"],
                Some((Client, true)),
                "a server mod switch is not the server flag",
            ),
            (
                "enfMain",
                &["-export", "-server"],
                Some((Client, false)),
                "a Diag export is not a server",
            ),
            (
                "DayZ_BE.exe",
                &["Z:\\DayZ\\DayZ_BE.exe"],
                None,
                "the BattlEye stub",
            ),
            ("steam", &["steam", "-applaunch", "221100"], None, "Steam"),
            ("", &[], None, "the empty process"),
        ];
        for (comm, args, expected, why) in cases {
            assert_eq!(classify(comm, args), *expected, "{why}");
        }
    }
}
