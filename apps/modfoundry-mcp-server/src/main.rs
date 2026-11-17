//! ModFoundry MCP server binary.
//!
//! Serves MCP over stdio: reads newline-delimited JSON-RPC requests on stdin,
//! writes responses on stdout. Wire into an MCP client with:
//!
//! ```json
//! { "mcpServers": { "modfoundry": { "command": "<path to modfoundry-mcp-server>" } } }
//! ```
//!
//! An agent reaches the read tools by default. `--allow-mutations` opens
//! `Mutate`/`Launch` for this session; `mf agent grant` opens anything for good.
//! `--workspace DIR` confines `workspace.undo` to changes made under that
//! folder, so a session started for one mod cannot reverse a change elsewhere.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use modfoundry_agent::{open_db, resolve_db, session_only_warning, OpenedDb, DEFAULT_AGENT_ID};
use modfoundry_commands::update;
use modfoundry_mcp_server::Server;

#[derive(Parser)]
#[command(
    name = "modfoundry-mcp-server",
    version,
    about = "ModFoundry MCP server over stdio (read-only unless mutations are allowed)"
)]
struct Args {
    /// Let this session run Mutate/Launch commands without a persisted grant.
    /// Destructive, Network and Publish still need an `mf agent grant` rule.
    #[arg(long)]
    allow_mutations: bool,
    /// Agent id this session acts as; policy rules and audit rows use it. Sessions left on
    /// the default share that id, so the app only ever allows them once.
    #[arg(long, default_value_t = DEFAULT_AGENT_ID, value_name = "ID")]
    agent_id: u64,
    /// Seconds to wait for a GUI frontend to answer a policy `ask` rule. Needs a
    /// database the GUI can also open (`--db` or `$MODFOUNDRY_DB`); without the
    /// flag an `ask` rule is a refusal.
    #[arg(long, value_name = "SECS", value_parser = clap::value_parser!(u64).range(1..))]
    ask_timeout: Option<u64>,
    /// Agent database holding the policy rules and the audit trail
    /// (default: `$MODFOUNDRY_DB`, else the host data dir).
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
    /// Confine `workspace.undo` to changes made under this folder.
    #[arg(long, value_name = "DIR")]
    workspace: Option<PathBuf>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("modfoundry-mcp-server: {e}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let args = Args::parse();
    start_update_check();
    let undo_root = resolve_undo_root(args.workspace)?;
    let server = open_server(args.db, args.ask_timeout)?;
    let server = if args.allow_mutations {
        server.allow_mutations()
    } else {
        server
    };
    let server = server.with_agent(args.agent_id);
    let server = match undo_root {
        Some(root) => server.confine_undo(root),
        None => server,
    };

    let stdin = io::stdin();
    let stdout = io::stdout();
    server.serve(&mut stdin.lock(), &mut stdout.lock())
}

/// Starts a due update check without waiting for it: the answer reaches the next session's instructions.
fn start_update_check() {
    let _ = update::start_notice();
}

/// The `--workspace` folder as an absolute path, or a start-up error naming the
/// path that is not an existing directory. Made absolute here because the undo
/// log compares it as given.
fn resolve_undo_root(workspace: Option<PathBuf>) -> io::Result<Option<PathBuf>> {
    let Some(dir) = workspace else {
        return Ok(None);
    };
    let absolute = std::path::absolute(&dir)?;
    if !absolute.is_dir() {
        return Err(io::Error::other(format!(
            "--workspace {}: not an existing directory",
            dir.display()
        )));
    }
    Ok(Some(absolute))
}

/// The database the user named must open — they asked for *that* policy and
/// trail. One this server picked for them may not, and then the session runs
/// with a store that lives only as long as the process, saying why on stderr.
fn open_server(db: Option<PathBuf>, ask_timeout: Option<u64>) -> io::Result<Server> {
    match open_db(&resolve_db(db)) {
        Ok(OpenedDb::SessionOnly { reason }) => {
            refuse_ask_timeout(ask_timeout)?;
            session_only(&reason)
        }
        Ok(OpenedDb::File { store, path }) => {
            let server = Server::with_store(store, Some(path.clone()));
            Ok(match ask_timeout {
                Some(secs) => server.with_store_confirm(path, Duration::from_secs(secs)),
                None => server,
            })
        }
        Err(message) => Err(io::Error::other(message)),
    }
}

/// `--ask-timeout` needs a database file a GUI frontend can answer through; a
/// session-only store has none, so the flag is a start-up error, not a no-op.
fn refuse_ask_timeout(ask_timeout: Option<u64>) -> io::Result<()> {
    if ask_timeout.is_some() {
        return Err(io::Error::other(
            "--ask-timeout needs a database the GUI can see; pass --db or set MODFOUNDRY_DB",
        ));
    }
    Ok(())
}

fn session_only(reason: &str) -> io::Result<Server> {
    eprintln!("{}", session_only_warning(reason));
    Ok(Server::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(extra: &[&str]) -> Result<Args, clap::Error> {
        let mut argv = vec!["modfoundry-mcp-server"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv)
    }

    #[test]
    fn a_relative_workspace_is_made_absolute() {
        let resolved = resolve_undo_root(Some(PathBuf::from(".")))
            .unwrap()
            .unwrap();
        assert!(resolved.is_absolute());
        assert_eq!(resolved, std::path::absolute(".").unwrap());
        let err = resolve_undo_root(Some(PathBuf::from("no/such/dir/mf-undo-root")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no/such/dir/mf-undo-root"), "got {err:?}");
    }

    #[test]
    fn a_missing_workspace_dir_is_a_start_up_error_naming_it() {
        let missing = std::env::temp_dir().join("mf-missing-workspace-dir");
        let _ = std::fs::remove_dir_all(&missing);
        let err = resolve_undo_root(Some(missing.clone())).unwrap_err();
        assert!(
            err.to_string()
                .contains(&missing.to_string_lossy().to_string()),
            "got {err}"
        );
    }

    #[test]
    fn ask_timeout_parses_seconds_and_defaults_to_none() {
        assert_eq!(
            parse(&["--ask-timeout", "30"]).unwrap().ask_timeout,
            Some(30)
        );
        assert_eq!(parse(&[]).unwrap().ask_timeout, None);
    }

    #[test]
    fn ask_timeout_rejects_zero_and_non_numbers() {
        assert!(parse(&["--ask-timeout", "0"]).is_err());
        assert!(parse(&["--ask-timeout", "soon"]).is_err());
    }

    #[test]
    fn ask_timeout_needs_a_shared_database() {
        let err = refuse_ask_timeout(Some(30)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "--ask-timeout needs a database the GUI can see; pass --db or set MODFOUNDRY_DB"
        );
        assert!(refuse_ask_timeout(None).is_ok());
    }
}
