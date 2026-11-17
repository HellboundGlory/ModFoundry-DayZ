//! How a policy `ask` reaches a frontend that can answer it.
//! Without a hook the broker refuses; nothing is ever approved by default.

use crate::broker::now_unix;
use modfoundry_core::EffectKind;
use modfoundry_store::confirm::{ConfirmAnswer, NewConfirmation};
use modfoundry_store::Store;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What a frontend is told before it decides whether to allow one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmRequest {
    pub actor: String,
    pub command: String,
    pub effect: EffectKind,
    pub args_hash: String,
    pub summary: String,
}

/// Answers a policy `ask`, on the dispatching thread.
pub trait Confirm: Send {
    fn confirm(&self, req: &ConfirmRequest) -> ConfirmAnswer;
}

/// Raises the request in the shared database and polls for a frontend's answer.
pub struct StoreConfirm {
    db: PathBuf,
    timeout: Duration,
    poll: Duration,
}

impl StoreConfirm {
    /// Ask through the database at `db`, waiting at most `timeout` and
    /// re-reading every 250 ms.
    pub fn new(db: impl AsRef<Path>, timeout: Duration) -> Self {
        Self {
            db: db.as_ref().to_path_buf(),
            timeout,
            poll: Duration::from_millis(250),
        }
    }
}

impl Confirm for StoreConfirm {
    /// Anything short of an explicit answer is a `Deny`.
    fn confirm(&self, req: &ConfirmRequest) -> ConfirmAnswer {
        let Ok(store) = Store::open(&self.db) else {
            return ConfirmAnswer::Deny;
        };
        let Ok(id) = store.request_confirmation(&NewConfirmation {
            at_unix: now_unix(),
            actor: req.actor.clone(),
            command: req.command.clone(),
            effect: req.effect,
            args_hash: req.args_hash.clone(),
            summary: req.summary.clone(),
        }) else {
            return ConfirmAnswer::Deny;
        };

        let deadline = Instant::now() + self.timeout;
        loop {
            match store.confirmation(id) {
                Ok(Some(row)) => {
                    if let Some(answer) = row.answer {
                        return answer;
                    }
                }
                // Refused below, so the answer is recorded and stands.
                _ => break,
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            std::thread::sleep(self.poll.min(deadline - now));
        }
        // First writer wins: if a frontend answered after the last poll, its
        // answer stands and is the one it was told took effect.
        match store.answer_confirmation(id, ConfirmAnswer::Deny, now_unix()) {
            Ok(false) => store
                .confirmation(id)
                .ok()
                .flatten()
                .and_then(|row| row.answer)
                .unwrap_or(ConfirmAnswer::Deny),
            _ => ConfirmAnswer::Deny,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn db_path(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("mf-confirm-{label}-{}-{n}", std::process::id()))
    }

    #[test]
    fn a_confirmation_that_cannot_be_read_is_answered_deny() {
        let db = db_path("unreadable");
        let request = ConfirmRequest {
            actor: "agent".into(),
            command: "test.plan".into(),
            effect: EffectKind::Mutate,
            args_hash: "0".into(),
            summary: "plan".into(),
        };
        let confirm = StoreConfirm::new(&db, Duration::from_secs(5));
        let handle = std::thread::spawn(move || confirm.confirm(&request));

        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut id = None;
        while id.is_none() && Instant::now() < deadline {
            id = conn
                .query_row(
                    "SELECT id FROM confirmation_request WHERE answer IS NULL",
                    [],
                    |row| row.get(0),
                )
                .ok();
            std::thread::sleep(Duration::from_millis(10));
        }
        let id: i64 = id.expect("the request was raised");
        conn.execute(
            "UPDATE confirmation_request SET effect = 'nonsense' WHERE id = ?1",
            [id],
        )
        .unwrap();

        assert_eq!(handle.join().unwrap(), ConfirmAnswer::Deny);
        let answer: Option<String> = conn
            .query_row(
                "SELECT answer FROM confirmation_request WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(answer.as_deref(), Some("deny"), "the refusal is recorded");
    }
}
