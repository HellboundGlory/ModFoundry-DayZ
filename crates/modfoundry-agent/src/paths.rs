//! Where a host-side frontend's agent database comes from.

use std::path::{Path, PathBuf};

use modfoundry_store::Store;

/// The agent database a frontend should use, and what a failure to open it
/// means. A database the user named is an error when it will not open — they
/// asked for *that* one. The host default is not: a state directory that cannot
/// be created must not turn a read-only `mf formats` into a failure, so the
/// frontend degrades to a session-only store and says so.
pub enum Database {
    /// Named explicitly (`--db`, `$MODFOUNDRY_DB`).
    Named(PathBuf),
    /// The host data directory (`modfoundry_platform::app_data_dir`).
    Default(PathBuf),
    /// The host exposes no data directory at all.
    Absent,
}

impl Database {
    /// The database path to open, if any.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Database::Named(path) | Database::Default(path) => Some(path),
            Database::Absent => None,
        }
    }

    /// Whether a failure to open this database is fatal.
    pub fn is_explicit(&self) -> bool {
        matches!(self, Database::Named(_))
    }

    /// Creates the directory the database file goes in; nothing to do without a path.
    pub fn ensure_dir(&self) -> Result<(), String> {
        self.path().map_or(Ok(()), create_data_dir)
    }
}

/// Resolve the agent database for a host-side frontend: `named` (the `--db`
/// flag) wins, then `$MODFOUNDRY_DB`, then the host data directory.
pub fn resolve_db(named: Option<PathBuf>) -> Database {
    resolve_db_with(named, &|key| std::env::var_os(key).map(PathBuf::from))
}

fn resolve_db_with(named: Option<PathBuf>, from_env: &dyn Fn(&str) -> Option<PathBuf>) -> Database {
    if let Some(path) = named {
        return Database::Named(path);
    }
    if let Some(path) = from_env("MODFOUNDRY_DB").filter(|path| !path.as_os_str().is_empty()) {
        return Database::Named(path);
    }
    match modfoundry_platform::app_data_dir() {
        Some(dir) => Database::Default(dir.join("modfoundry.sqlite")),
        None => Database::Absent,
    }
}

/// What opening the frontend's agent database produced.
pub enum OpenedDb {
    /// The database file opened.
    File { store: Store, path: PathBuf },
    /// No usable file; the frontend runs on a session-only store for `reason`.
    SessionOnly { reason: String },
}

/// Opens `database`; `Err` carries the message for an explicitly named
/// database that would not open.
pub fn open_db(database: &Database) -> Result<OpenedDb, String> {
    let Some(path) = database.path() else {
        return Ok(OpenedDb::SessionOnly {
            reason: "no host data directory is available".into(),
        });
    };
    let opened = create_data_dir(path).and_then(|()| Store::open(path).map_err(|e| e.to_string()));
    match opened {
        Ok(store) => Ok(OpenedDb::File {
            store,
            path: path.to_path_buf(),
        }),
        Err(e) if database.is_explicit() => {
            Err(format!("opening agent database {}: {e}", path.display()))
        }
        Err(e) => Ok(OpenedDb::SessionOnly {
            reason: format!("{} could not be opened ({e})", path.display()),
        }),
    }
}

/// The stderr line a frontend prints when it falls back to a session-only store.
pub fn session_only_warning(reason: &str) -> String {
    format!("warning: {reason}; the agent policy and audit trail are session-only")
}

fn create_data_dir(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lookup(pairs: Vec<(&'static str, &'static str)>) -> impl Fn(&str) -> Option<PathBuf> {
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| PathBuf::from(v))
        }
    }

    #[test]
    fn an_empty_database_variable_falls_through_to_the_data_dir() {
        let empty = resolve_db_with(None, &env_lookup(vec![("MODFOUNDRY_DB", "")]));
        let unset = resolve_db_with(None, &env_lookup(vec![]));
        assert_eq!(empty.path(), unset.path());
        assert!(
            !empty.is_explicit(),
            "an ignored variable names no database"
        );
    }

    #[test]
    fn a_named_database_wins_and_is_treated_as_explicit() {
        let named = resolve_db(Some(PathBuf::from("/custom/agent.sqlite")));
        assert_eq!(named.path(), Some(Path::new("/custom/agent.sqlite")));
        assert!(named.is_explicit(), "a named database must fail loudly");
    }

    #[test]
    fn only_a_named_database_is_explicit() {
        let default = Database::Default(PathBuf::from("/data/modfoundry.sqlite"));
        assert_eq!(default.path(), Some(Path::new("/data/modfoundry.sqlite")));
        assert!(!default.is_explicit(), "the host default may degrade");
        assert_eq!(Database::Absent.path(), None);
        assert!(!Database::Absent.is_explicit());
    }
}
