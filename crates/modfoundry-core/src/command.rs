//! Command-registry shell.
//!
//! Every ModFoundry operation — read or mutation — is registered once as a
//! typed [`Command`] with a namespaced id (e.g. `project.scaffold`,
//! `config.validate`) and an [`EffectKind`]. GUI menus, CLI verbs, MCP tools and
//! automation all enumerate and dispatch the same registry, so there is one
//! implementation and one permission vocabulary.
//!
//! The authorize→validate→preview→apply→audit pipeline is real: the
//! effect gate and dispatch live here, the broker above this crate authorizes
//! against the persisted policy and audits every call, a `{"preview": true}` call
//! is answered by [`Command::plan`] without running the body, and a mutation
//! reports the change-set (owned by `modfoundry-vfs`) that undoes it. `events`
//! are still to come, and `modfoundry-core` stays IO-free throughout: it never
//! reads the store, the filesystem or a change-set's contents itself.
//!
//! Command implementations are synchronous for now. The async pipeline (builds,
//! launches, MCP transport) is added behind the same registry without changing
//! its interface shape.

use crate::diagnostic::Diagnostic;
use crate::{ChangeSummary, CommandError, EffectKind};
use serde::Serialize;
use std::collections::HashMap;

/// Who is invoking a command. Governs which [`EffectKind`]s are permitted.
/// `User`/`CliSession` may run anything; `Agent`/`Automation` reach the classes
/// they always may ([`EffectKind::allowed_by_default`]) plus whatever their
/// [`Grants`] carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    User,
    CliSession,
    Agent { id: u64, grants: Grants },
    Automation { grants: Grants },
}

/// Effect classes explicitly granted to an actor, on top of its default reach.
///
/// The policy store lives above `Core`, so the gate cannot read it; instead the
/// dispatch broker resolves the persisted policy and hands the result to the
/// gate on the actor ([`Actor::with_grant`]). That keeps `modfoundry-core`
/// IO-free and keeps [`Core::dispatch`] the only entry point a frontend needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Grants(u8);

impl Grants {
    /// No effect class beyond the actor's default reach.
    pub const NONE: Grants = Grants(0);

    /// Whether `effect` was explicitly granted.
    pub fn allows(self, effect: EffectKind) -> bool {
        self.0 & effect_bit(effect) != 0
    }

    /// This set plus `effect`.
    pub fn with(self, effect: EffectKind) -> Self {
        Grants(self.0 | effect_bit(effect))
    }
}

const fn effect_bit(effect: EffectKind) -> u8 {
    match effect {
        EffectKind::Read => 1 << 0,
        EffectKind::IndexWrite => 1 << 1,
        EffectKind::Mutate => 1 << 2,
        EffectKind::Destructive => 1 << 3,
        EffectKind::Launch => 1 << 4,
        EffectKind::Network => 1 << 5,
        EffectKind::Publish => 1 << 6,
    }
}

impl Actor {
    /// Whether this actor may run a command of `effect` without an explicit grant.
    pub fn may_run(self, effect: EffectKind) -> bool {
        match self {
            Actor::User | Actor::CliSession => true,
            Actor::Agent { grants, .. } | Actor::Automation { grants } => {
                effect.allowed_by_default() || grants.allows(effect)
            }
        }
    }

    /// This actor with `effect` explicitly granted. The channel by which the
    /// policy store (above `Core`) reaches the gate.
    pub fn with_grant(self, effect: EffectKind) -> Self {
        match self {
            Actor::Agent { id, grants } => Actor::Agent {
                id,
                grants: grants.with(effect),
            },
            Actor::Automation { grants } => Actor::Automation {
                grants: grants.with(effect),
            },
            actor => actor,
        }
    }
}

/// Result of executing a command. Frontends print/render these uniformly.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Read-only commands: arbitrary structured data (counts, parse results).
    Data(serde_json::Value),
    /// A mutation that ran, with its summary and — when
    /// the body produced one — the change-set that inverts it, which the host
    /// records so `workspace.undo` can replay it.
    Changed {
        summary: ChangeSummary,
        #[serde(skip)]
        change_set: Option<serde_json::Value>,
    },
    /// A read command that also produced analyzer findings (lint/validate).
    Diagnostics {
        count: usize,
        diagnostics: Vec<Diagnostic>,
    },
    /// A mutation that was *not* run: the change-set it would apply. The payload
    /// is the VFS write API's change-set as JSON; `modfoundry-core` does not own
    /// that schema, so it stays opaque here exactly as it does in the store.
    Preview(serde_json::Value),
}

impl Outcome {
    /// A mutation that changed content, with no undo record — for outputs that
    /// are regenerable (or already recorded elsewhere).
    pub fn changed(summary: ChangeSummary) -> Self {
        Outcome::Changed {
            summary,
            change_set: None,
        }
    }

    /// A mutation whose change-set the host can record and invert.
    pub fn applied(summary: ChangeSummary, change_set: serde_json::Value) -> Self {
        Outcome::Changed {
            summary,
            change_set: Some(change_set),
        }
    }

    /// The change-set this outcome recorded, when it has one.
    pub fn change_set(&self) -> Option<&serde_json::Value> {
        match self {
            Outcome::Changed { change_set, .. } => change_set.as_ref(),
            _ => None,
        }
    }

    /// The frontend-neutral JSON payload: `Data` passes through, `Diagnostics`
    /// and `Changed` become their shared shapes, `Preview` is the planned
    /// change-set. Frontends render from this, so CLI, MCP and GUI cannot
    /// disagree about what a command returned.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Outcome::Data(v) => v.clone(),
            Outcome::Diagnostics { diagnostics, .. } => {
                serde_json::to_value(diagnostics).unwrap_or(serde_json::Value::Null)
            }
            Outcome::Changed { summary, .. } => {
                serde_json::to_value(summary).unwrap_or(serde_json::Value::Null)
            }
            Outcome::Preview(plan) => plan.clone(),
        }
    }
}

/// A registered, executable operation.
///
/// Implementors provide a static [`spec`](Command::spec) (id + effect + summary)
/// and [`execute`](Command::execute).
pub trait Command: Send + Sync {
    /// The registered namespaced id, e.g. `"config.validate"`.
    fn spec(&self) -> CommandSpec<'static>;

    /// JSON Schema for this command's arguments, so tool/form frontends (MCP
    /// `tools/list`, GUI forms) can advertise the shape `execute` accepts.
    /// Argument-less commands keep the default open object.
    fn args_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }

    /// Whether this command can answer `{"preview": true}` at all.
    ///
    /// The runtime answer is [`plan`](Command::plan) returning a plan; this is
    /// the same fact for frontends that must decide *before* calling — the MCP
    /// tool list shows a mutation a session may only preview, and a tool schema
    /// advertises the argument only when it will be honoured.
    fn previewable(&self) -> bool {
        false
    }

    /// What this call *would* change, without changing it: the change-set a
    /// `{"preview": true}` call returns and the host shows as a diff.
    ///
    /// `None` (the default) means the command has no separate plan and a preview
    /// of it is invalid. Implementors build the plan by running the same body
    /// that `execute` runs against a dry-run VFS transaction, so a preview
    /// cannot drift from what applying does.
    fn plan(&self, args: serde_json::Value) -> Result<Option<serde_json::Value>, CommandError> {
        let _ = args;
        Ok(None)
    }

    /// What a confirmation prompt says this call would do; `None` keeps the generic line.
    fn confirm_summary(&self, _args: &serde_json::Value) -> Option<String> {
        None
    }

    /// Run the command against the supplied argument value. Implementors decide
    /// how strictly they validate args; malformed input returns
    /// [`CommandError::InvalidArguments`].
    fn execute(&self, args: serde_json::Value) -> Result<Outcome, CommandError>;

    /// A quiet command's runs are not audited.
    fn quiet(&self) -> bool {
        false
    }

    /// The other registered commands this one runs inside its body.
    fn runs(&self) -> &'static [&'static str] {
        &[]
    }
}

/// The reserved argument every mutating command accepts: `{"preview": true}`
/// asks for the change-set instead of the change.
pub const PREVIEW_ARG: &str = "preview";

/// The one command id that is part of the dispatch contract rather than a
/// registry-only detail: `workspace.undo`'s payload (the recorded change-set)
/// is supplied by the host, so the broker that resolves it and the registry
/// that executes it must agree on the id.
pub const WORKSPACE_UNDO: &str = "workspace.undo";

/// The update command that replaces the installed programs; only the user runs it.
pub const HOST_UPDATE: &str = "host.update";

/// Whether `args` asks for a preview.
pub fn is_preview(args: &serde_json::Value) -> bool {
    args.get(PREVIEW_ARG)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// `args` without the preview flag, so a body or plan sees only its own
/// arguments.
fn without_preview(mut args: serde_json::Value) -> serde_json::Value {
    if let Some(object) = args.as_object_mut() {
        object.remove(PREVIEW_ARG);
    }
    args
}

/// Static identity of a command, used by registry lookup and by frontends when
/// they enumerate commands to build menus/tool lists.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec<'a> {
    pub id: &'a str,
    pub effect: EffectKind,
    /// One-line human summary shown in `mf --help`-style listings.
    pub summary: &'a str,
}

/// The shared command registry. One instance is created per session and handed
/// to whichever frontend(s) are active.
pub struct Core {
    commands: HashMap<&'static str, Box<dyn Command>>,
}

impl Core {
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
        }
    }

    /// Register a command. Returns the previous command of the same id, if any
    /// (so frontends can layer/replace behaviour deliberately).
    pub fn register(&mut self, cmd: Box<dyn Command>) -> Option<Box<dyn Command>> {
        let id = cmd.spec().id;
        self.commands.insert(id, cmd)
    }

    /// Look up a command by its namespaced id.
    pub fn command(&self, id: &str) -> Option<&dyn Command> {
        self.commands.get(id).map(|c| c.as_ref())
    }

    /// All registered commands (unsorted; frontends may sort for display).
    pub fn commands(&self) -> impl Iterator<Item = &dyn Command> {
        self.commands.values().map(|c| c.as_ref())
    }

    /// Dispatch `args` to the command named `id` on behalf of `actor`.
    ///
    /// Enforces the effect gate first, so a restricted actor can never reach a
    /// mutating command's body even if a frontend mis-exposes it. A
    /// `{"preview": true}` call is the one exception, and not really one: it
    /// needs only read reach, because it returns the change-set without running
    /// the body — which is what lets an agent that may not mutate still see what
    /// a mutation would do.
    pub fn dispatch(
        &self,
        actor: Actor,
        id: &str,
        args: serde_json::Value,
    ) -> Result<Outcome, CommandError> {
        let cmd = self
            .command(id)
            .ok_or_else(|| CommandError::NotFound(id.into()))?;
        let preview = is_preview(&args);
        let effect = if preview {
            EffectKind::Read
        } else {
            cmd.spec().effect
        };
        if !actor.may_run(effect) {
            return Err(CommandError::PermissionDenied(format!(
                "actor {actor:?} needs a grant for effect {effect:?} on {id}"
            )));
        }
        if preview {
            return match cmd.plan(without_preview(args))? {
                Some(plan) => Ok(Outcome::Preview(plan)),
                None => Err(CommandError::InvalidArguments(format!(
                    "{id} has no plan to preview"
                ))),
            };
        }
        cmd.execute(args)
    }
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EffectKind;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct AddOne;

    impl Command for AddOne {
        fn spec(&self) -> CommandSpec<'static> {
            CommandSpec {
                id: "test.add_one",
                effect: EffectKind::Read,
                summary: "add one to a number",
            }
        }
        fn execute(&self, args: serde_json::Value) -> Result<Outcome, CommandError> {
            let n = args.get("n").and_then(|v| v.as_i64()).ok_or_else(|| {
                CommandError::InvalidArguments("expected integer field `n`".into())
            })?;
            Ok(Outcome::Data(serde_json::json!({ "result": n + 1 })))
        }
    }

    struct Nuke;

    impl Command for Nuke {
        fn spec(&self) -> CommandSpec<'static> {
            CommandSpec {
                id: "test.nuke",
                effect: EffectKind::Destructive,
                summary: "destructive command used to test gating",
            }
        }
        fn execute(&self, _args: serde_json::Value) -> Result<Outcome, CommandError> {
            Ok(Outcome::changed(ChangeSummary {
                kind: EffectKind::Destructive,
                files_changed: 0,
                description: "nuked (test)".into(),
            }))
        }
    }

    /// A mutation with a plan: it records every call, so a test can tell a
    /// preview (which must not run the body) from an apply.
    struct Scripter(Arc<AtomicUsize>);

    impl Command for Scripter {
        fn spec(&self) -> CommandSpec<'static> {
            CommandSpec {
                id: "test.scripter",
                effect: EffectKind::Mutate,
                summary: "write a file (test)",
            }
        }
        fn plan(&self, args: serde_json::Value) -> Result<Option<serde_json::Value>, CommandError> {
            // The preview flag is the caller's, not the command's: a plan sees
            // the same arguments `execute` would.
            assert!(args.get("preview").is_none(), "preview leaked into args");
            Ok(Some(serde_json::json!({
                "root": "mod:X",
                "changes": [ { "path": "mod:X:a.c", "op": "create" } ],
            })))
        }
        fn execute(&self, _args: serde_json::Value) -> Result<Outcome, CommandError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Outcome::applied(
                ChangeSummary {
                    kind: EffectKind::Mutate,
                    files_changed: 1,
                    description: "wrote a.c".into(),
                },
                serde_json::json!({ "root": "mod:X", "changes": [ { "path": "mod:X:a.c", "before": null } ] }),
            ))
        }
    }

    #[test]
    fn preview_returns_the_plan_and_never_runs_the_body() {
        let mut core = Core::new();
        let runs = Arc::new(AtomicUsize::new(0));
        core.register(Box::new(Scripter(runs.clone())));

        let out = core
            .dispatch(
                Actor::User,
                "test.scripter",
                serde_json::json!({ "preview": true, "path": "a.c" }),
            )
            .expect("preview");
        let Outcome::Preview(plan) = out else {
            panic!("expected a preview, got {out:?}");
        };
        assert_eq!(plan["root"], "mod:X");
        assert_eq!(plan["changes"][0]["path"], "mod:X:a.c");
        assert_eq!(
            runs.load(Ordering::SeqCst),
            0,
            "a preview must not run the body"
        );

        // `preview: false` (and its absence) apply for real.
        for args in [
            serde_json::json!({ "preview": false, "path": "a.c" }),
            serde_json::json!({ "path": "a.c" }),
        ] {
            assert!(matches!(
                core.dispatch(Actor::User, "test.scripter", args),
                Ok(Outcome::Changed { .. })
            ));
        }
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn anyone_who_may_read_may_preview_a_mutation() {
        let mut core = Core::new();
        let runs = Arc::new(AtomicUsize::new(0));
        core.register(Box::new(Scripter(runs.clone())));
        let agent = Actor::Agent {
            id: 1,
            grants: Grants::NONE,
        };
        assert!(matches!(
            core.dispatch(
                agent,
                "test.scripter",
                serde_json::json!({ "preview": true })
            ),
            Ok(Outcome::Preview(_))
        ));
        // Without the flag the same actor is refused: preview is not an escape
        // hatch into the body.
        assert!(matches!(
            core.dispatch(agent, "test.scripter", serde_json::json!({})),
            Err(CommandError::PermissionDenied(_))
        ));
    }

    #[test]
    fn a_command_without_a_plan_cannot_be_previewed() {
        let mut core = Core::new();
        core.register(Box::new(Nuke));
        let err = core
            .dispatch(
                Actor::User,
                "test.nuke",
                serde_json::json!({ "preview": true }),
            )
            .unwrap_err();
        assert!(matches!(err, CommandError::InvalidArguments(_)), "{err:?}");
    }

    #[test]
    fn a_change_set_travels_with_the_outcome_but_not_in_its_wire_shape() {
        let mut core = Core::new();
        let runs = Arc::new(AtomicUsize::new(0));
        core.register(Box::new(Scripter(runs.clone())));
        let out = core
            .dispatch(Actor::User, "test.scripter", serde_json::json!({}))
            .expect("apply");
        let change_set = out.change_set().expect("an undoable mutation");
        assert_eq!(change_set["changes"][0]["path"], "mod:X:a.c");
        assert_eq!(out.to_json()["files_changed"], 1);
        assert_eq!(
            Outcome::changed(ChangeSummary {
                kind: EffectKind::Mutate,
                files_changed: 0,
                description: "no record".into(),
            })
            .change_set(),
            None
        );
    }

    #[test]
    fn registers_and_dispatch_by_id() {
        let mut core = Core::new();
        core.register(Box::new(AddOne));
        let out = core.dispatch(Actor::User, "test.add_one", serde_json::json!({ "n": 41 }));
        match out.unwrap() {
            Outcome::Data(v) => assert_eq!(v["result"], 41 + 1),
            _ => panic!("expected Data"),
        }
    }

    #[test]
    fn unknown_command_is_not_found() {
        let core = Core::new();
        let err = core
            .dispatch(Actor::User, "missing.cmd", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, CommandError::NotFound(_)));
    }

    #[test]
    fn agent_cannot_run_destructive() {
        let mut core = Core::new();
        core.register(Box::new(Nuke));
        let err = core
            .dispatch(
                Actor::Agent {
                    id: 7,
                    grants: Grants::NONE,
                },
                "test.nuke",
                serde_json::json!({}),
            )
            .unwrap_err();
        assert!(matches!(err, CommandError::PermissionDenied(_)));
        // A user may.
        assert!(matches!(
            core.dispatch(Actor::User, "test.nuke", serde_json::json!({})),
            Ok(Outcome::Changed { .. })
        ));
    }

    #[test]
    fn an_explicit_grant_lets_an_actor_run_beyond_its_default_reach() {
        let mut core = Core::new();
        core.register(Box::new(Nuke));
        let granted = Actor::Agent {
            id: 7,
            grants: Grants::NONE,
        }
        .with_grant(EffectKind::Destructive);
        assert!(matches!(
            core.dispatch(granted, "test.nuke", serde_json::json!({})),
            Ok(Outcome::Changed { .. })
        ));

        // The grant lives on the actor, so it neither leaks to another effect
        // class nor applies to another actor value.
        let wrong_class = Actor::Agent {
            id: 7,
            grants: Grants::NONE,
        }
        .with_grant(EffectKind::Mutate);
        assert!(wrong_class.may_run(EffectKind::Mutate));
        assert!(!wrong_class.may_run(EffectKind::Destructive));
    }

    #[test]
    fn grants_accumulate_one_effect_class_at_a_time() {
        let grants = Grants::NONE
            .with(EffectKind::Mutate)
            .with(EffectKind::Publish);
        assert!(grants.allows(EffectKind::Mutate));
        assert!(grants.allows(EffectKind::Publish));
        assert!(!grants.allows(EffectKind::Destructive));
        assert!(!Grants::NONE.allows(EffectKind::Read));
    }

    #[test]
    fn invalid_arguments_are_reported() {
        let mut core = Core::new();
        core.register(Box::new(AddOne));
        let err = core
            .dispatch(Actor::User, "test.add_one", serde_json::json!({ "n": "x" }))
            .unwrap_err();
        assert!(matches!(err, CommandError::InvalidArguments(_)));
    }

    #[test]
    fn to_json_presents_each_outcome_shape() {
        let data = Outcome::Data(serde_json::json!({ "n": 1 }));
        assert_eq!(data.to_json(), serde_json::json!({ "n": 1 }));

        let diagnostics = Outcome::Diagnostics {
            count: 1,
            diagnostics: vec![Diagnostic::new("MF-C310", crate::Severity::Error, "bad")],
        };
        let v = diagnostics.to_json();
        assert_eq!(v[0]["code"], "MF-C310");
        assert_eq!(v[0]["severity"], "error");

        let changed = Outcome::changed(ChangeSummary {
            kind: EffectKind::Mutate,
            files_changed: 2,
            description: "wrote two files".into(),
        });
        assert_eq!(changed.to_json()["files_changed"], 2);

        let preview = Outcome::Preview(serde_json::json!({ "root": "mod:X" }));
        assert_eq!(preview.to_json()["root"], "mod:X");
    }
}
