//! A launch worked out in full before anything is written: the commands a run starts, and the
//! links, copies and files it stages.

use modfoundry_diag::LaunchCommand;
use modfoundry_platform::stage::{remove_leftover, Staged};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A launch session worked out without writing anything.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunPlan {
    /// The commands the run starts, in the order it starts them.
    pub launches: Vec<PlannedLaunch>,
    /// Launch staging, applied through `Staged` and removed when the run ends.
    pub steps: Vec<StageStep>,
    /// Project files and folders the run makes and keeps (a missing `serverDZ.cfg`, the profiles folder),
    /// applied through `WriteTxn` / `ensure_dir`, never through `Staged`.
    pub project: Vec<StageStep>,
    /// Paths a dead run left that the launch would remove first.
    pub leftovers: Vec<PathBuf>,
}

/// One command of a run, and the part it plays in it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedLaunch {
    pub role: LaunchRole,
    pub command: LaunchCommand,
}

/// Which side of a run a command starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchRole {
    Server,
    Client,
}

/// One change the launch makes to an install.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StageStep {
    CreateDir {
        path: PathBuf,
    },
    Link {
        link: PathBuf,
        target: PathBuf,
    },
    /// Re-points a dangling link through `Staged::relink`.
    Relink {
        link: PathBuf,
        target: PathBuf,
    },
    /// Removes a stale file before staging.
    Remove {
        path: PathBuf,
    },
    Copy {
        from: PathBuf,
        to: PathBuf,
    },
    Write {
        path: PathBuf,
        #[serde(skip)]
        contents: Vec<u8>,
        bytes: usize,
    },
}

impl StageStep {
    /// The path the step makes or removes.
    pub fn path(&self) -> &Path {
        match self {
            StageStep::CreateDir { path }
            | StageStep::Remove { path }
            | StageStep::Write { path, .. } => path,
            StageStep::Link { link, .. } | StageStep::Relink { link, .. } => link,
            StageStep::Copy { to, .. } => to,
        }
    }
}

impl Serialize for PlannedLaunch {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let command = &self.command;
        let mut out = serializer.serialize_struct("PlannedLaunch", 6)?;
        out.serialize_field("role", &self.role)?;
        out.serialize_field("program", &command.program.to_string_lossy())?;
        out.serialize_field("args", &lossy(&command.args))?;
        out.serialize_field("cwd", &command.cwd)?;
        out.serialize_field(
            "env",
            &command
                .env
                .iter()
                .map(|(name, value)| (name.as_str(), value.to_string_lossy()))
                .collect::<Vec<_>>(),
        )?;
        out.serialize_field("env_remove", &command.env_remove)?;
        out.end()
    }
}

/// `values` as strings, losing only the bytes that are not UTF-8.
fn lossy(values: &[OsString]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect()
}

/// Runs `steps` through `staged` in order, naming the step that fails.
pub fn apply(steps: &[StageStep], staged: &mut Staged) -> anyhow::Result<()> {
    for step in steps {
        stage(step, staged)
            .map_err(|error| anyhow::anyhow!("{}: {error}", step.path().display()))?;
    }
    Ok(())
}

fn stage(step: &StageStep, staged: &mut Staged) -> std::io::Result<()> {
    match step {
        StageStep::CreateDir { path } => staged.create_dir(path),
        StageStep::Link { link, target } => staged.link(link, target),
        StageStep::Relink { link, target } => staged.relink(link, target),
        StageStep::Remove { path } => remove_leftover(path),
        StageStep::Copy { from, to } => staged.copy(from, to),
        StageStep::Write { path, contents, .. } => staged.write(path, contents),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mf-plan-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn apply_makes_each_kind_of_step_and_the_guard_undoes_them() {
        let root = tmp("apply");
        let target = root.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(root.join("source"), b"src").unwrap();
        let stale = root.join("stale");
        std::fs::write(&stale, b"old").unwrap();
        let dangling = root.join("dangling");
        std::os::unix::fs::symlink(root.join("wiped"), &dangling).unwrap();
        let steps = vec![
            StageStep::CreateDir {
                path: root.join("keys"),
            },
            StageStep::Link {
                link: root.join("fresh"),
                target: target.clone(),
            },
            StageStep::Relink {
                link: dangling.clone(),
                target: target.clone(),
            },
            StageStep::Remove {
                path: stale.clone(),
            },
            StageStep::Copy {
                from: root.join("source"),
                to: root.join("copy"),
            },
            StageStep::Write {
                path: root.join("written"),
                contents: b"body".to_vec(),
                bytes: 4,
            },
        ];

        let mut staged = Staged::in_install(&root);
        apply(&steps, &mut staged).unwrap();

        assert!(root.join("keys").is_dir());
        assert_eq!(std::fs::read_link(root.join("fresh")).unwrap(), target);
        assert_eq!(std::fs::read_link(&dangling).unwrap(), target);
        assert!(!stale.exists());
        assert_eq!(std::fs::read(root.join("copy")).unwrap(), b"src");
        assert_eq!(std::fs::read(root.join("written")).unwrap(), b"body");

        drop(staged);
        assert!(!root.join("keys").exists());
        assert!(root.join("fresh").symlink_metadata().is_err());
        assert!(dangling.symlink_metadata().is_err());
        assert!(!root.join("copy").exists());
        assert!(!root.join("written").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_plan_serialises_its_launch_and_steps_for_a_reader() {
        let root = tmp("json");
        let plan = RunPlan {
            launches: vec![PlannedLaunch {
                role: LaunchRole::Client,
                command: LaunchCommand {
                    program: root.join("DayZ_x64.exe"),
                    args: vec![OsString::from("-window")],
                    cwd: Some(root.clone()),
                    env: vec![("SteamAppId".into(), OsString::from("221100"))],
                    env_remove: vec!["LD_PRELOAD".into()],
                },
            }],
            steps: vec![StageStep::Write {
                path: root.join("cfg"),
                contents: b"body".to_vec(),
                bytes: 4,
            }],
            project: Vec::new(),
            leftovers: Vec::new(),
        };

        let json = serde_json::to_value(&plan).unwrap();
        let launch = &json["launches"][0];
        assert_eq!(launch["role"], "client");
        assert_eq!(
            launch["program"].as_str().map(Path::new),
            Some(root.join("DayZ_x64.exe").as_path())
        );
        assert_eq!(launch["args"], serde_json::json!(["-window"]));
        assert_eq!(launch["env"], serde_json::json!([["SteamAppId", "221100"]]));
        assert_eq!(launch["env_remove"], serde_json::json!(["LD_PRELOAD"]));
        assert_eq!(json["steps"][0]["op"], "write");
        assert_eq!(json["steps"][0]["bytes"], 4);
        assert!(
            json["steps"][0].get("contents").is_none(),
            "file contents stay out of the answer"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failing_step_names_its_path() {
        let root = tmp("apply-fail");
        let steps = [StageStep::Copy {
            from: root.join("missing"),
            to: root.join("copy"),
        }];

        let error = apply(&steps, &mut Staged::in_install(&root))
            .unwrap_err()
            .to_string();

        assert!(
            error.contains(&root.join("copy").display().to_string()),
            "the error must name the step: {error}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
