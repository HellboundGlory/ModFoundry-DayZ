//! The permission policy: per-actor allow/deny/ask rules.
//!
//! An agent's default reach is read-only; every mutation, launch, network call or
//! publish needs a rule that says so. Rules are selectors — actor, command and
//! effect class — where `*` (see [`ANY`]) matches anything. Several rules can
//! match one call, so lookup resolves by **specificity**: the exact actor beats
//! the wildcard actor, the exact command beats `*`, the exact effect beats a
//! wildcard effect. Ties go to the most recently written rule.

use crate::{effect_from_str, effect_str, Store, StoreError};
use modfoundry_core::EffectKind;
use rusqlite::{OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// Selector value that matches any actor, command or effect class.
pub const ANY: &str = "*";

/// What the policy says about a matching call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Run it.
    Allow,
    /// Refuse it before the body runs.
    Deny,
    /// Neither: the interactive host must confirm before it runs.
    Ask,
}

impl Decision {
    text_forms!(impl "decision" {
        Decision::Allow => "allow",
        Decision::Deny => "deny",
        Decision::Ask => "ask",
    });
}

/// A persisted policy rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub id: i64,
    pub actor: String,
    pub command: String,
    /// `None` is the wildcard: the rule applies to every effect class.
    pub effect: Option<EffectKind>,
    pub decision: Decision,
    pub created_at: i64,
}

impl Store {
    /// Write a rule, replacing any existing rule with the same selector.
    pub fn set_policy_rule(
        &self,
        actor: &str,
        command: &str,
        effect: Option<EffectKind>,
        decision: Decision,
        at_unix: i64,
    ) -> Result<i64, StoreError> {
        let effect = effect.map(effect_str).unwrap_or(ANY);
        self.conn().execute(
            "INSERT INTO policy_rule (actor, command, effect, decision, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (actor, command, effect)
             DO UPDATE SET decision = excluded.decision, created_at = excluded.created_at",
            rusqlite::params![actor, command, effect, decision.as_str(), at_unix],
        )?;
        Ok(self.conn().query_row(
            "SELECT id FROM policy_rule WHERE actor = ?1 AND command = ?2 AND effect = ?3",
            rusqlite::params![actor, command, effect],
            |row| row.get(0),
        )?)
    }

    /// Every rule, oldest first.
    pub fn policy_rules(&self) -> Result<Vec<PolicyRule>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, actor, command, effect, decision, created_at
             FROM policy_rule ORDER BY id",
        )?;
        let rules = stmt
            .query_map([], map_rule)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rules)
    }

    /// Drop the rule with exactly this selector, reporting whether one matched.
    pub fn remove_policy_rule(
        &self,
        actor: &str,
        command: &str,
        effect: Option<EffectKind>,
    ) -> Result<bool, StoreError> {
        let effect = effect.map(effect_str).unwrap_or(ANY);
        let removed = self.conn().execute(
            "DELETE FROM policy_rule WHERE actor = ?1 AND command = ?2 AND effect = ?3",
            rusqlite::params![actor, command, effect],
        )?;
        Ok(removed > 0)
    }

    /// The most specific rule matching this call, or `None` when no rule applies
    /// (which for a non-interactive actor means "not granted").
    pub fn decision_for(
        &self,
        actor: &str,
        command: &str,
        effect: EffectKind,
    ) -> Result<Option<Decision>, StoreError> {
        let effect = effect_str(effect);
        let found: Option<String> = self
            .conn()
            .prepare_cached(
                "SELECT decision,
                        (actor = ?1) + (command = ?2) + (effect = ?3) AS specificity
                 FROM policy_rule
                 WHERE (actor = ?1 OR actor = ?4)
                   AND (command = ?2 OR command = ?4)
                   AND (effect = ?3 OR effect = ?4)
                 ORDER BY specificity DESC, created_at DESC, id DESC
                 LIMIT 1",
            )?
            .query_row(rusqlite::params![actor, command, effect, ANY], |row| {
                row.get(0)
            })
            .optional()?;
        found.as_deref().map(Decision::parse).transpose()
    }

    /// The rule that names exactly `command`, for any effect class: the rules that match `effect`
    /// rank as in [`decision_for`](Self::decision_for), then rules for another class.
    pub fn named_decision(
        &self,
        actor: &str,
        command: &str,
        effect: EffectKind,
    ) -> Result<Option<Decision>, StoreError> {
        let effect = effect_str(effect);
        let found: Option<String> = self
            .conn()
            .prepare_cached(
                "SELECT decision FROM policy_rule
                 WHERE command = ?2 AND (actor = ?1 OR actor = ?4)
                 ORDER BY (effect = ?3 OR effect = ?4) DESC,
                          (actor = ?1) + (effect = ?3) DESC, created_at DESC, id DESC
                 LIMIT 1",
            )?
            .query_row(rusqlite::params![actor, command, effect, ANY], |row| {
                row.get(0)
            })
            .optional()?;
        found.as_deref().map(Decision::parse).transpose()
    }
}

fn map_rule(row: &Row<'_>) -> rusqlite::Result<PolicyRule> {
    let effect: String = row.get(3)?;
    let decision: String = row.get(4)?;
    Ok(PolicyRule {
        id: row.get(0)?,
        actor: row.get(1)?,
        command: row.get(2)?,
        effect: if effect == ANY {
            None
        } else {
            Some(effect_from_str(&effect).map_err(|e| crate::bad_text(3, e))?)
        },
        decision: Decision::parse(&decision).map_err(|e| crate::bad_text(4, e))?,
        created_at: row.get(5)?,
    })
}
