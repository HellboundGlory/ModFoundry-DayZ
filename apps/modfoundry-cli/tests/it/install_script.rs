//! `packaging/linux/install.sh` run against a stub package, a scratch home and stub agent programs.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const INSTALLER: &str = include_str!("../../../../packaging/linux/install.sh");
const SKILL: &str = include_str!("../../../../packaging/skill/modfoundry/SKILL.md");

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("mf-install-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("stubs")).unwrap();
        Self(root)
    }

    fn join(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn home(&self) -> PathBuf {
        self.join("home")
    }

    fn bin(&self) -> PathBuf {
        self.join("home/.local/bin")
    }

    fn lib(&self) -> PathBuf {
        self.join("home/.local/lib/modfoundry")
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.join("calls.log")).unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Waits out "Text file busy": a fork elsewhere in the test binary can hold a new program open.
fn wait_until_runnable(program: &Path) {
    for _ in 0..200 {
        match Command::new(program).output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => return,
        }
    }
    panic!("{} stayed busy", program.display());
}

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    wait_until_runnable(path);
}

/// A package folder shaped like the release tarball, with stub programs.
fn build_package(scratch: &Scratch) {
    let pkg = scratch.join("pkg");
    std::fs::create_dir_all(pkg.join("skill/modfoundry")).unwrap();
    std::fs::write(pkg.join("install.sh"), INSTALLER).unwrap();
    std::fs::write(pkg.join("skill/modfoundry/SKILL.md"), SKILL).unwrap();
    write_exec(&pkg.join("mf"), "echo 'mf 0.0.0-test'\n");
    for name in ["modfoundry-mcp-server", "modfoundry-steam"] {
        write_exec(&pkg.join(name), "exit 0\n");
    }
    std::fs::write(pkg.join("libsteam_api.so"), b"").unwrap();
}

/// A logging stub agent whose `mcp get` prints `answer-<name>` when present, else exits 1.
fn stub_agent(scratch: &Scratch, name: &str) {
    let answer = scratch.join(&format!("answer-{name}"));
    // Runs with no arguments are the readiness probe in `wait_until_runnable`, not calls.
    let body = format!(
        "[ $# -gt 0 ] || exit 0\n\
         echo \"[$(pwd -P)] {name} $*\" >> '{log}'\n\
         if [ \"$1 $2\" = \"mcp get\" ]; then\n\
         [ -f '{answer}' ] || {{ echo 'No MCP server named modfoundry'; exit 1; }}\n\
         cat '{answer}'\n\
         fi\n",
        log = scratch.join("calls.log").display(),
        answer = answer.display(),
    );
    write_exec(&scratch.join("stubs").join(name), &body);
}

fn run_installer(scratch: &Scratch, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        scratch.join("stubs").display(),
        system_tools(scratch).display()
    );
    Command::new("bash")
        .arg(scratch.join("pkg/install.sh"))
        .args(args)
        .env_clear()
        .env("HOME", scratch.home())
        .env("PATH", path)
        .current_dir(scratch.home())
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn claude_user_registration_adds_the_server_and_writes_marker() {
    let scratch = Scratch::new("claude-user");
    build_package(&scratch);
    stub_agent(&scratch, "claude");
    let out = run_installer(&scratch, &["--target", "claude-user"]);
    assert!(out.status.success(), "{}", text(&out));
    let server = scratch.bin().join("modfoundry-mcp-server");
    let add = format!("claude mcp add -s user modfoundry -- {}", server.display());
    assert!(scratch.calls().contains(&add), "{}", scratch.calls());
    let marker = std::fs::read_to_string(scratch.lib().join(".modfoundry-install")).unwrap();
    assert_eq!(marker, "installed by install.sh\n");
    assert!(scratch.lib().join("skill/modfoundry/SKILL.md").exists());
}

#[test]
fn an_entry_for_another_program_is_left_alone() {
    let cases = [
        (
            "codex",
            "codex",
            r#"{"name": "modfoundry", "command": "/opt/other/modfoundry-mcp-server"}"#,
            "modfoundry is already registered with codex for another program; left as it is",
        ),
        (
            "claude",
            "claude-user",
            "modfoundry:\n  Command: /opt/other/modfoundry-mcp-server\n",
            "modfoundry is already registered with claude for another program; left as it is",
        ),
    ];
    for (agent, target, answer, message) in cases {
        let scratch = Scratch::new(&format!("{agent}-foreign"));
        build_package(&scratch);
        stub_agent(&scratch, agent);
        std::fs::write(scratch.join(&format!("answer-{agent}")), answer).unwrap();
        let out = run_installer(&scratch, &["--target", target]);
        assert!(out.status.success(), "{agent}: {}", text(&out));
        let calls = scratch.calls();
        assert!(
            !calls.contains(&format!("{agent} mcp remove")),
            "{agent}: {calls}"
        );
        assert!(
            !calls.contains(&format!("{agent} mcp add")),
            "{agent}: {calls}"
        );
        assert!(text(&out).contains(message), "{agent}: {}", text(&out));
    }
}

#[test]
fn command_code_entry_for_this_program_is_replaced() {
    let scratch = Scratch::new("command-code-own");
    build_package(&scratch);
    stub_agent(&scratch, "command-code");
    let server = scratch.bin().join("modfoundry-mcp-server");
    std::fs::write(
        scratch.join("answer-command-code"),
        format!("Command: {}\n", server.display()),
    )
    .unwrap();
    let out = run_installer(&scratch, &["--target", "command-code"]);
    assert!(out.status.success(), "{}", text(&out));
    let calls = scratch.calls();
    let remove = calls
        .find("command-code mcp remove -s user modfoundry")
        .unwrap_or_else(|| panic!("no remove: {calls}"));
    let add = calls
        .find(&format!(
            "command-code mcp add --transport stdio -s user modfoundry -- {}",
            server.display()
        ))
        .unwrap_or_else(|| panic!("no add: {calls}"));
    assert!(remove < add, "{calls}");
}

#[test]
fn agents_target_without_a_config_writes_the_skill() {
    let scratch = Scratch::new("agents-new");
    build_package(&scratch);
    let out = run_installer(&scratch, &["--target", "agents"]);
    assert!(out.status.success(), "{}", text(&out));
    let skill = scratch.home().join(".agents/skills/modfoundry/SKILL.md");
    assert_eq!(std::fs::read_to_string(skill).unwrap(), SKILL);
}

/// The `jq` on the test's own `PATH`, linked into the stub folder so the installer finds it too.
fn link_real_jq(scratch: &Scratch) -> bool {
    let found = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("jq"))
            .find(|candidate| candidate.is_file())
    });
    match found {
        Some(jq) => {
            std::os::unix::fs::symlink(jq, scratch.join("stubs/jq")).unwrap();
            true
        }
        None => {
            eprintln!("jq is not installed; the merge case was not run");
            false
        }
    }
}

#[test]
fn agents_target_merges_into_an_existing_config_with_jq() {
    let scratch = Scratch::new("agents-merge");
    if !link_real_jq(&scratch) {
        return;
    }
    build_package(&scratch);
    let agents = scratch.home().join(".agents");
    std::fs::create_dir_all(&agents).unwrap();
    let other = serde_json::json!({
        "theme": "dark",
        "mcpServers": { "other": { "command": "/opt/other/server", "args": ["--fast"] } }
    });
    std::fs::write(agents.join("mcp.json"), other.to_string()).unwrap();
    let out = run_installer(&scratch, &["--target", "agents"]);
    assert!(out.status.success(), "{}", text(&out));
    let config = std::fs::read_to_string(agents.join("mcp.json")).unwrap();
    let server = scratch.bin().join("modfoundry-mcp-server");
    let mut expected = other;
    expected["mcpServers"]["modfoundry"] =
        serde_json::json!({ "command": server.display().to_string(), "args": [] });
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&config).unwrap(),
        expected
    );
}

#[test]
fn yes_registers_only_the_agents_found_and_the_agents_folder() {
    let scratch = Scratch::new("yes");
    build_package(&scratch);
    stub_agent(&scratch, "claude");
    stub_agent(&scratch, "codex");
    let out = run_installer(&scratch, &["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    let calls = scratch.calls();
    assert!(
        calls.contains("claude mcp add -s user modfoundry"),
        "{calls}"
    );
    assert!(calls.contains("codex mcp add modfoundry"), "{calls}");
    assert!(!calls.contains("command-code"), "{calls}");
    assert!(scratch.home().join(".agents/mcp.json").exists());
    assert!(scratch
        .home()
        .join(".agents/skills/modfoundry/SKILL.md")
        .exists());
}

#[test]
fn claude_project_registration_adds_local_scope_inside_the_folder() {
    let scratch = Scratch::new("claude-project");
    build_package(&scratch);
    stub_agent(&scratch, "claude");
    let project = scratch.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let target = format!("claude-project={}", project.display());
    let out = run_installer(&scratch, &["--target", &target]);
    assert!(out.status.success(), "{}", text(&out));
    let folder = std::fs::canonicalize(&project).unwrap();
    let server = scratch.bin().join("modfoundry-mcp-server");
    let add = format!(
        "[{}] claude mcp add -s local modfoundry -- {}",
        folder.display(),
        server.display()
    );
    assert!(scratch.calls().contains(&add), "{}", scratch.calls());
}

#[test]
fn without_flags_off_a_terminal_only_programs_install_and_the_command_is_printed() {
    let scratch = Scratch::new("no-flags");
    build_package(&scratch);
    stub_agent(&scratch, "claude");
    let out = run_installer(&scratch, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(scratch.lib().join("mf").exists());
    assert_eq!(scratch.calls(), "");
    let server = scratch.bin().join("modfoundry-mcp-server");
    let shown = text(&out);
    assert!(
        shown
            .lines()
            .any(|line| line == server.display().to_string()),
        "{shown}"
    );
}

#[test]
fn uninstall_removes_our_agents_entry_and_skill_and_keeps_the_others() {
    let scratch = Scratch::new("uninstall-agents");
    if !link_real_jq(&scratch) {
        return;
    }
    build_package(&scratch);
    let agents = scratch.home().join(".agents");
    std::fs::create_dir_all(&agents).unwrap();
    let other =
        serde_json::json!({ "mcpServers": { "other": { "command": "/opt/other/server" } } });
    std::fs::write(agents.join("mcp.json"), other.to_string()).unwrap();
    let out = run_installer(&scratch, &["--target", "agents"]);
    assert!(out.status.success(), "{}", text(&out));
    let out = run_installer(&scratch, &["--uninstall"]);
    assert!(out.status.success(), "{}", text(&out));
    let config = std::fs::read_to_string(agents.join("mcp.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&config).unwrap(),
        other
    );
    assert!(!agents.join("skills/modfoundry").exists());
    assert!(!scratch.lib().exists());
}

#[test]
fn agents_target_leaves_another_programs_entry_in_the_config() {
    let scratch = Scratch::new("agents-foreign");
    if !link_real_jq(&scratch) {
        return;
    }
    build_package(&scratch);
    let agents = scratch.home().join(".agents");
    std::fs::create_dir_all(&agents).unwrap();
    let before = serde_json::json!({
        "mcpServers": {
            "modfoundry": { "command": "/opt/other/modfoundry-mcp-server", "args": [] }
        }
    });
    std::fs::write(agents.join("mcp.json"), before.to_string()).unwrap();
    let out = run_installer(&scratch, &["--target", "agents"]);
    assert!(out.status.success(), "{}", text(&out));
    let config = std::fs::read_to_string(agents.join("mcp.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&config).unwrap(),
        before
    );
    assert!(
        text(&out).contains(
            "modfoundry is already registered with agents for another program; left as it is"
        ),
        "{}",
        text(&out)
    );
}

#[test]
fn agents_missing_from_path_are_skipped_with_their_commands_printed() {
    let cases = [
        ("claude-user", "claude mcp add -s user modfoundry -- "),
        ("codex", "codex mcp add modfoundry -- "),
        (
            "command-code",
            "command-code mcp add --transport stdio -s user modfoundry -- ",
        ),
    ];
    for (target, command) in cases {
        let scratch = Scratch::new("missing-agent");
        build_package(&scratch);
        let out = run_installer(&scratch, &["--target", target]);
        assert!(out.status.success(), "{target}: {}", text(&out));
        let server = scratch.bin().join("modfoundry-mcp-server");
        let shown = text(&out);
        assert!(
            shown.contains(&format!("{command}{}", server.display())),
            "{target}: {shown}"
        );
    }
}

#[test]
fn bad_targets_stop_before_anything_is_installed() {
    let cases = [
        vec!["--target", "vscode"],
        vec![
            "--target",
            "claude-project=/nonexistent/modfoundry-test-folder",
        ],
    ];
    for args in cases {
        let scratch = Scratch::new("bad-target");
        build_package(&scratch);
        let out = run_installer(&scratch, &args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
        assert!(!scratch.lib().exists(), "{args:?}");
    }
}

#[test]
fn help_lists_the_flags_and_targets() {
    let scratch = Scratch::new("help");
    build_package(&scratch);
    let out = run_installer(&scratch, &["--help"]);
    assert!(out.status.success(), "{}", text(&out));
    let shown = text(&out);
    for word in [
        "--allow-mutations",
        "--yes",
        "--target",
        "--uninstall",
        "claude-user",
        "claude-project=<dir>",
        "codex",
        "command-code",
        "agents",
        "print",
    ] {
        assert!(shown.contains(word), "{word}: {shown}");
    }
}

#[test]
fn agents_target_leaves_a_config_that_is_not_json_as_it_is() {
    let scratch = Scratch::new("agents-broken");
    if !link_real_jq(&scratch) {
        return;
    }
    build_package(&scratch);
    let agents = scratch.home().join(".agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("mcp.json"), "{ not json").unwrap();
    let out = run_installer(&scratch, &["--target", "agents"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        std::fs::read_to_string(agents.join("mcp.json")).unwrap(),
        "{ not json"
    );
    assert!(
        text(&out).contains("is not valid JSON; left as it is"),
        "{}",
        text(&out)
    );
}

/// The system programs without `jq` and the agent CLIs, so a run never reaches a real one.
fn system_tools(scratch: &Scratch) -> PathBuf {
    const LEFT_OUT: [&str; 4] = ["jq", "claude", "codex", "command-code"];
    let tools = scratch.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    for dir in ["/usr/bin", "/bin"] {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let left_out = name.to_str().is_some_and(|n| LEFT_OUT.contains(&n));
            if !left_out && tools.join(&name).symlink_metadata().is_err() {
                std::os::unix::fs::symlink(entry.path(), tools.join(&name)).unwrap();
            }
        }
    }
    tools
}

#[test]
fn agents_target_without_jq_prints_the_entry_and_leaves_the_config() {
    let scratch = Scratch::new("agents-no-jq");
    build_package(&scratch);
    let agents = scratch.home().join(".agents");
    std::fs::create_dir_all(&agents).unwrap();
    let before = r#"{"mcpServers": {"other": {"command": "/opt/other/server"}}}"#;
    std::fs::write(agents.join("mcp.json"), before).unwrap();
    let out = run_installer(&scratch, &["--target", "agents"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        std::fs::read_to_string(agents.join("mcp.json")).unwrap(),
        before
    );
    let server = scratch.bin().join("modfoundry-mcp-server");
    let entry = format!(
        r#""modfoundry": {{"command": "{}", "args": []}}"#,
        server.display()
    );
    assert!(text(&out).contains(&entry), "{}", text(&out));
}

#[test]
fn a_foreign_mf_in_the_bin_folder_stops_the_install() {
    let scratch = Scratch::new("foreign-mf");
    build_package(&scratch);
    let bin = scratch.bin();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("mf"), "someone else's program\n").unwrap();
    let out = run_installer(&scratch, &["--target", "agents"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("move it aside first"), "{}", text(&out));
    assert!(!scratch.lib().exists());
}

#[test]
fn uninstall_removes_a_claude_entry_only_when_it_names_this_program() {
    let cases = [
        (true, None),
        (false, Some("/opt/other/modfoundry-mcp-server")),
    ];
    for (removed, other_program) in cases {
        let scratch = Scratch::new("uninstall-claude");
        build_package(&scratch);
        stub_agent(&scratch, "claude");
        let command = match other_program {
            Some(path) => path.to_owned(),
            None => scratch
                .bin()
                .join("modfoundry-mcp-server")
                .display()
                .to_string(),
        };
        assert!(run_installer(&scratch, &["--target", "print"])
            .status
            .success());
        std::fs::write(
            scratch.join("answer-claude"),
            claude_entry(&command, "User"),
        )
        .unwrap();
        let out = run_installer(&scratch, &["--uninstall"]);
        assert!(out.status.success(), "{}", text(&out));
        let calls = scratch.calls();
        assert!(calls.contains("claude mcp get modfoundry"), "{calls}");
        assert_eq!(
            calls.contains("claude mcp remove -s user modfoundry"),
            removed,
            "{calls}"
        );
    }
}

#[test]
fn an_entry_that_only_passes_our_path_as_an_argument_is_not_ours() {
    let scratch = Scratch::new("argument-only");
    build_package(&scratch);
    stub_agent(&scratch, "claude");
    let server = scratch.bin().join("modfoundry-mcp-server");
    let entry = format!(
        "modfoundry:\n  Command: /usr/bin/node\n  Args: {}\n",
        server.display()
    );
    std::fs::write(scratch.join("answer-claude"), entry).unwrap();
    let out = run_installer(&scratch, &["--uninstall"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        !scratch.calls().contains("claude mcp remove"),
        "{}",
        scratch.calls()
    );
}

#[test]
fn a_reinstall_replaces_our_entry_when_home_has_a_space_in_it() {
    let scratch = Scratch::new("home with space");
    build_package(&scratch);
    stub_agent(&scratch, "codex");
    let server = scratch.bin().join("modfoundry-mcp-server");
    let entry = format!(
        r#"{{"name": "modfoundry", "command": "{}"}}"#,
        server.display()
    );
    std::fs::write(scratch.join("answer-codex"), entry).unwrap();
    let out = run_installer(&scratch, &["--target", "codex"]);
    assert!(out.status.success(), "{}", text(&out));
    let calls = scratch.calls();
    assert!(calls.contains("codex mcp remove modfoundry"), "{calls}");
    assert!(calls.contains("codex mcp add modfoundry"), "{calls}");
}

#[test]
fn allow_mutations_reaches_the_claude_add() {
    let scratch = Scratch::new("allow-mutations");
    build_package(&scratch);
    stub_agent(&scratch, "claude");
    let out = run_installer(&scratch, &["--allow-mutations", "--target", "claude-user"]);
    assert!(out.status.success(), "{}", text(&out));
    let server = scratch.bin().join("modfoundry-mcp-server");
    let add = format!(
        "claude mcp add -s user modfoundry -- {} --allow-mutations",
        server.display()
    );
    assert!(scratch.calls().contains(&add), "{}", scratch.calls());
}

#[test]
fn a_missing_empty_or_blank_config_is_written_as_new() {
    let cases = [
        ("no-file", None, false),
        ("empty-file", Some(""), false),
        ("whitespace-only", Some("\n  \n"), true),
    ];
    for (case, existing, with_jq) in cases {
        let scratch = Scratch::new(&format!("agents-{case}"));
        build_package(&scratch);
        if with_jq {
            link_real_jq(&scratch);
        }
        let agents = scratch.home().join(".agents");
        if let Some(content) = existing {
            std::fs::create_dir_all(&agents).unwrap();
            std::fs::write(agents.join("mcp.json"), content).unwrap();
        }
        let out = run_installer(&scratch, &["--target", "agents"]);
        assert!(out.status.success(), "{case}: {}", text(&out));
        let config = std::fs::read_to_string(agents.join("mcp.json")).unwrap();
        assert!(config.contains("\"modfoundry\""), "{case}: {config}");
        let server = scratch.bin().join("modfoundry-mcp-server");
        let expected = serde_json::json!({
            "mcpServers": {
                "modfoundry": { "command": server.display().to_string(), "args": [] }
            }
        });
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&config).unwrap(),
            expected,
            "{case}"
        );
    }
}

fn claude_entry(command: &str, scope: &str) -> String {
    format!("modfoundry:\n  Scope: {scope} config (available in all your projects)\n  Command: {command}\n")
}

#[test]
fn uninstall_leaves_an_entry_for_a_server_in_the_bin_folder_that_is_not_ours() {
    let scratch = Scratch::new("uninstall-foreign-bin");
    build_package(&scratch);
    stub_agent(&scratch, "claude");
    std::fs::create_dir_all(scratch.bin()).unwrap();
    let server = scratch.bin().join("modfoundry-mcp-server");
    std::fs::write(&server, "a developer's own build").unwrap();
    let entry = claude_entry(&server.display().to_string(), "User");
    std::fs::write(scratch.join("answer-claude"), entry).unwrap();

    let out = run_installer(&scratch, &["--uninstall"]);

    assert!(out.status.success(), "{}", text(&out));
    assert!(
        !scratch.calls().contains("mcp remove"),
        "{}",
        scratch.calls()
    );
}

#[test]
fn uninstall_leaves_a_claude_entry_of_another_scope() {
    let scratch = Scratch::new("uninstall-other-scope");
    build_package(&scratch);
    stub_agent(&scratch, "claude");
    assert!(run_installer(&scratch, &["--target", "print"])
        .status
        .success());
    let server = scratch.bin().join("modfoundry-mcp-server");
    let entry = claude_entry(&server.display().to_string(), "Local");
    std::fs::write(scratch.join("answer-claude"), entry).unwrap();

    let out = run_installer(&scratch, &["--uninstall"]);

    assert!(out.status.success(), "{}", text(&out));
    assert!(
        !scratch.calls().contains("mcp remove"),
        "{}",
        scratch.calls()
    );
}

#[test]
fn an_answer_without_a_command_line_counts_as_no_entry() {
    let scratch = Scratch::new("cc-odd-answer");
    build_package(&scratch);
    stub_agent(&scratch, "command-code");
    std::fs::write(
        scratch.join("answer-command-code"),
        "A new version is available\n",
    )
    .unwrap();

    let out = run_installer(&scratch, &["--target", "command-code"]);

    assert!(out.status.success(), "{}", text(&out));
    assert!(
        scratch.calls().contains("command-code mcp add"),
        "{}",
        scratch.calls()
    );
}

#[test]
fn a_project_folder_under_the_home_tilde_is_found() {
    let scratch = Scratch::new("claude-project-tilde");
    build_package(&scratch);
    stub_agent(&scratch, "claude");
    let project = scratch.home().join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let out = run_installer(&scratch, &["--target", "claude-project=~/proj"]);

    assert!(out.status.success(), "{}", text(&out));
    let expected = format!(
        "[{}] claude mcp add -s local",
        project.canonicalize().unwrap().display()
    );
    assert!(scratch.calls().contains(&expected), "{}", scratch.calls());
}

#[test]
fn a_skill_of_the_users_own_in_the_agents_folder_is_kept() {
    let scratch = Scratch::new("agents-own-skill");
    build_package(&scratch);
    let skill = scratch.home().join(".agents/skills/modfoundry/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "my own notes").unwrap();

    let out = run_installer(&scratch, &["--target", "agents"]);

    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), "my own notes");
}

#[test]
fn a_missing_claude_prints_the_project_command_with_its_folder() {
    let scratch = Scratch::new("missing-claude-project");
    build_package(&scratch);
    let project = scratch.home().join("my proj");
    std::fs::create_dir_all(&project).unwrap();
    let target = format!("claude-project={}", project.display());

    let out = run_installer(&scratch, &["--target", &target]);

    assert!(out.status.success(), "{}", text(&out));
    let quoted = project.display().to_string().replace(' ', "\\ ");
    let expected = format!("cd {quoted} && claude mcp add -s local");
    assert!(text(&out).contains(&expected), "{}", text(&out));
}

#[test]
fn a_web_entry_of_the_same_name_is_left_alone() {
    let scratch = Scratch::new("claude-web-entry");
    build_package(&scratch);
    stub_agent(&scratch, "claude");
    let entry = "modfoundry:\n  Scope: User config (available in all your projects)\n  Type: http\n  URL: https://example.invalid/mcp\n";
    std::fs::write(scratch.join("answer-claude"), entry).unwrap();

    let out = run_installer(&scratch, &["--target", "claude-user"]);

    assert!(out.status.success(), "{}", text(&out));
    assert!(!scratch.calls().contains("mcp add"), "{}", scratch.calls());
    assert!(text(&out).contains("left as it is"), "{}", text(&out));
}

#[test]
fn a_first_install_to_the_agents_folder_prints_no_shell_errors() {
    let scratch = Scratch::new("agents-first-quiet");
    build_package(&scratch);

    let out = run_installer(&scratch, &["--target", "agents"]);

    assert!(out.status.success(), "{}", text(&out));
    assert!(!text(&out).contains("No such file"), "{}", text(&out));
}

/// The words bash makes of `line`, as a user pasting it into a shell would get them.
fn shell_words(line: &str) -> Vec<String> {
    let out = Command::new("bash")
        .args(["-c", r#"eval "set -- $1"; printf '%s\n' "$@""#, "_", line])
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_home_with_a_quote_and_a_space_gives_valid_json_and_a_pasteable_command() {
    let scratch = Scratch::new(r#"q"uote sp"#);
    build_package(&scratch);
    let out = run_installer(
        &scratch,
        &[
            "--allow-mutations",
            "--target",
            "agents",
            "--target",
            "print",
        ],
    );
    assert!(out.status.success(), "{}", text(&out));
    let server = scratch.bin().join("modfoundry-mcp-server");
    let config = std::fs::read_to_string(scratch.home().join(".agents/mcp.json")).unwrap();
    let config: serde_json::Value = serde_json::from_str(&config).expect(&config);
    assert_eq!(
        config["mcpServers"]["modfoundry"]["command"],
        server.display().to_string()
    );
    let shown = text(&out);
    let printed = shown
        .lines()
        .find(|line| line.ends_with("--allow-mutations"))
        .expect(&shown);
    assert_eq!(
        shell_words(printed),
        [server.display().to_string(), "--allow-mutations".to_owned()]
    );
}

#[test]
fn uninstall_without_jq_says_to_remove_the_agents_entry_by_hand() {
    let scratch = Scratch::new("uninstall-no-jq");
    build_package(&scratch);
    let out = run_installer(&scratch, &["--target", "agents"]);
    assert!(out.status.success(), "{}", text(&out));
    let config = scratch.home().join(".agents/mcp.json");
    let before = std::fs::read_to_string(&config).unwrap();

    let out = run_installer(&scratch, &["--uninstall"]);

    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
    assert!(
        text(&out).contains(&format!(
            "jq not found; remove the modfoundry entry from mcpServers in {} by hand",
            config.display()
        )),
        "{}",
        text(&out)
    );
}

#[test]
fn a_home_with_a_quote_and_a_backslash_keeps_its_agents_entry_ours() {
    let scratch = Scratch::new(r#"q"uote b\s"#);
    if !link_real_jq(&scratch) {
        return;
    }
    build_package(&scratch);
    let config = scratch.home().join(".agents/mcp.json");
    let out = run_installer(&scratch, &["--target", "agents"]);
    assert!(out.status.success(), "{}", text(&out));

    let out = run_installer(&scratch, &["--allow-mutations", "--target", "agents"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!text(&out).contains("left as it is"), "{}", text(&out));
    assert!(!text(&out).contains("warning"), "{}", text(&out));
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(
        saved["mcpServers"]["modfoundry"]["args"],
        serde_json::json!(["--allow-mutations"])
    );

    let out = run_installer(&scratch, &["--uninstall"]);
    assert!(out.status.success(), "{}", text(&out));
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert!(saved["mcpServers"].get("modfoundry").is_none(), "{saved}");
}
