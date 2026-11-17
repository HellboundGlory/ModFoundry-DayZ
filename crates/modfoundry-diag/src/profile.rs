//! Launch profiles: a description of how to start a local
//! DayZ server / diag build for the test loop, rendered to canonical argv.
//!
//! Canonical dedicated-server args follow the shipped
//! `DayZServer/server_manager/example_launch_params.txt` and BI wiki:
//! `-config=… "-mod=…" "-serverMod=…" -profiles=… -port=…`
//! plus `-freezecheck -adminlog -dologs`. `-filePatching` is diag-build only;
//! `-server`/`-config`/`-mission` are per-profile.
//!
//! Pure (no IO): building argv from a profile is a pure string/CLI operation.

use std::path::PathBuf;

/// What kind of engine process a profile launches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    /// Native dedicated server (`DayZServer`). The first-class Linux target.
    Server,
    /// A diag build acting as a local server (`DayZDiag_x64 -server`).
    DiagServer,
}

/// Optional boolean engine switches.
#[derive(Debug, Clone, Copy, Default)]
pub struct LogFlags {
    /// `-doLogs` — widen engine log verbosity.
    pub do_logs: bool,
    /// `-adminLog` — enable admin log.
    pub admin_log: bool,
    /// `-netLog` — enable network traffic logging.
    pub net_log: bool,
    /// `-freezeCheck` — dump on freeze.
    pub freeze_check: bool,
    /// `-filePatching` — diag builds only; loose-file loading.
    pub file_patching: bool,
}

/// A configured launch profile: which content roots (in order) and which
/// flags. Ordering of `mods` is preserved — override behaviour
/// depends on `-mod` order.
#[derive(Debug, Clone)]
pub struct ServerProfile {
    pub kind: LaunchKind,
    /// `-config=…` server config file.
    pub config: Option<PathBuf>,
    /// `-mission=…` mission override (per-profile).
    pub mission: Option<PathBuf>,
    /// `-port=…` dedicated-server listen port.
    pub port: Option<u16>,
    /// `-profiles=…` log/profiles dir (capture target).
    pub profiles: Option<PathBuf>,
    /// Ordered `-mod=…` content roots (a;b;…).
    pub mods: Vec<PathBuf>,
    /// Ordered `-serverMod=…` server-side-only content roots.
    pub server_mods: Vec<PathBuf>,
    /// `-ip=…` bind address.
    pub ip: Option<String>,
    pub flags: LogFlags,
}

impl ServerProfile {
    /// Render this profile to engine argv (excluding the exe itself).
    pub fn argv(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(cfg) = &self.config {
            out.push(format!("-config={}", cfg.display()));
        }
        if let Some(m) = &self.mission {
            out.push(format!("-mission={}", m.display()));
        }
        out.push(mod_arg("-mod=", &self.mods));
        out.push(mod_arg("-serverMod=", &self.server_mods));
        if let Some(prof) = &self.profiles {
            out.push(format!("-profiles={}", prof.display()));
        }
        if let Some(port) = self.port {
            out.push(format!("-port={port}"));
        }
        if let Some(ip) = &self.ip {
            out.push(format!("-ip={ip}"));
        }
        // `-server` is only meaningful for a diag build running as a local server.
        if self.kind == LaunchKind::DiagServer {
            out.push("-server".to_string());
        }
        if self.flags.file_patching {
            out.push("-filePatching".to_string());
        }
        if self.flags.freeze_check {
            out.push("-freezecheck".to_string());
        }
        if self.flags.admin_log {
            out.push("-adminlog".to_string());
        }
        if self.flags.do_logs {
            out.push("-dologs".to_string());
        }
        if self.flags.net_log {
            out.push("-netLog".to_string());
        }
        out
    }
}

/// A client auto-connect target. The DayZ client takes the server address as a
/// single `-connect=<ip>:<gameport>[:<queryport>]` triple — *not* the separate
/// `-ip`/`-port` fields the server profile models (verified in a
/// live client-under-Proton run: `-connect=192.168.1.254:2402:27016`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    pub host: String,
    pub game_port: u16,
    /// Query port (steamQueryPort); the engine accepts the triple without it too.
    pub query_port: Option<u16>,
}

impl Connect {
    /// Render the `-connect=…` value.
    pub fn to_arg(&self) -> String {
        match self.query_port {
            Some(q) => format!("{}:{}:{}", self.host, self.game_port, q),
            None => format!("{}:{}", self.host, self.game_port),
        }
    }

    /// Parse `host:gameport[:queryport]` (the inverse of [`Connect::to_arg`]).
    /// Returns `None` for a malformed string or an out-of-range port.
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split(':');
        let host = it.next()?.trim();
        if host.is_empty() {
            return None;
        }
        let game_port = it.next()?.trim().parse::<u16>().ok()?;
        let query_port = match it.next() {
            Some(q) => Some(q.trim().parse::<u16>().ok()?),
            None => None,
        };
        if it.next().is_some() {
            return None;
        }
        Some(Self {
            host: host.to_string(),
            game_port,
            query_port,
        })
    }
}

/// A client launch profile.
///
/// The client cannot be spawned as a plain child the way the server is: it runs
/// under Steam's Proton runtime (`DayZ_x64.exe` directly crashes). The working
/// Linux path is to build the Proton runtime command itself — the Steam Linux
/// Runtime `run` script with the Proton build and the game exe — because
/// `steam -applaunch 221100` opens the official DayZ Launcher, not the game
/// (`modfoundry_platform::proton_launch`). The official
/// launcher assembles mods from Steam subscriptions, so each local mod is
/// staged as an `@name` symlink inside the DayZ install dir and passed as a
/// relative `-mod=@name`. Client logs land in the Proton prefix, not
/// `-profiles`, so capture is a separate dir (see
/// `modfoundry_commands::run_ops::client_log_dir`).
#[derive(Debug, Clone)]
pub struct ClientProfile {
    /// Steam app id for DayZ (both client and dedicated server are 221100/223350;
    /// the client is 221100).
    pub app_id: u64,
    /// Ordered `@name` mod link names (already staged as symlinks in the install dir).
    pub mods: Vec<String>,
    /// `-connect=…` auto-connect target.
    pub connect: Option<Connect>,
    /// `-name=<profile>` game profile name.
    pub profile_name: Option<String>,
    /// Extra game args appended verbatim (e.g. `-nolauncher`, `-world=empty`).
    pub extra: Vec<String>,
    pub flags: LogFlags,
}

impl ClientProfile {
    /// Render the **game** arguments (what the engine receives), excluding the
    /// Steam wrapper. Pure.
    pub fn argv(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![format!("-mod={}", self.mods.join(";"))];
        if let Some(c) = &self.connect {
            out.push(format!("-connect={}", c.to_arg()));
        }
        if let Some(n) = &self.profile_name {
            out.push(format!("-name={n}"));
        }
        out.extend(self.extra.iter().cloned());
        if self.flags.file_patching {
            out.push("-filePatching".to_string());
        }
        if self.flags.net_log {
            out.push("-netLog".to_string());
        }
        if self.flags.do_logs {
            out.push("-dologs".to_string());
        }
        out
    }

    /// Render the full Steam launch command: `steam -applaunch <appid> <game args>`.
    /// On current Steam this opens the official DayZ Launcher rather than the
    /// game, so it is kept only for the explicit `--launcher` override path.
    /// Pure (the Steam executable path is resolved at the boundary).
    pub fn steam_argv(&self) -> Vec<String> {
        let mut out = vec!["-applaunch".to_string(), self.app_id.to_string()];
        out.extend(self.argv());
        out
    }
}

/// Render `-mod` from an ordered list: absolute or relative paths, joined with
/// `;`. Empty list still yields `-mod=` (the engine expects the flag present).
fn mod_arg(prefix: &str, roots: &[std::path::PathBuf]) -> String {
    format!(
        "{prefix}{}",
        roots
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(";")
    )
}

#[cfg(test)]
mod tests {
    use super::{ClientProfile, Connect, LaunchKind, LogFlags, ServerProfile};
    use std::path::PathBuf;

    fn profile(flags: LogFlags) -> ServerProfile {
        ServerProfile {
            kind: LaunchKind::Server,
            config: Some(PathBuf::from("serverDZ.cfg")),
            mission: None,
            port: Some(2302),
            profiles: Some(PathBuf::from("/tmp/profiles")),
            mods: vec![PathBuf::from("/mods/@A"), PathBuf::from("/mods/@B")],
            server_mods: vec![PathBuf::from("/mods/@Srv")],
            ip: None,
            flags,
        }
    }

    #[test]
    fn renders_canonical_server_argv_in_order() {
        let p = profile(LogFlags {
            do_logs: true,
            admin_log: true,
            freeze_check: true,
            ..Default::default()
        });
        let argv = p.argv();
        // -config then -mod then -serverMod then -profiles then -port, flags last.
        assert_eq!(argv[0], "-config=serverDZ.cfg");
        assert_eq!(argv[1], "-mod=/mods/@A;/mods/@B");
        assert_eq!(argv[2], "-serverMod=/mods/@Srv");
        assert_eq!(argv[3], "-profiles=/tmp/profiles");
        assert_eq!(argv[4], "-port=2302");
        assert!(argv.iter().any(|a| a == "-freezecheck"));
        assert!(argv.iter().any(|a| a == "-adminlog"));
        assert!(argv.iter().any(|a| a == "-dologs"));
    }

    #[test]
    fn file_patching_only_present_when_requested() {
        let yes = profile(LogFlags {
            file_patching: true,
            ..Default::default()
        });
        assert!(yes.argv().iter().any(|a| a == "-filePatching"));
        let no = profile(LogFlags::default());
        assert!(!no.argv().iter().any(|a| a == "-filePatching"));
    }

    #[test]
    fn optional_fields_are_omitted_when_absent() {
        let p = profile(LogFlags::default());
        let argv = p.argv();
        assert!(!argv.iter().any(|a| a.starts_with("-ip=")));
        assert!(!argv.iter().any(|a| a.starts_with("-mission=")));
        assert!(!argv.iter().any(|a| a == "-netLog"));
    }

    #[test]
    fn empty_mod_lists_render_empty_mod_flag() {
        let p = ServerProfile {
            mods: vec![],
            server_mods: vec![],
            ..profile(LogFlags::default())
        };
        let argv = p.argv();
        assert!(argv.iter().any(|a| a == "-mod="));
        assert!(argv.iter().any(|a| a == "-serverMod="));
    }

    #[test]
    fn netlog_renders_when_set() {
        let p = profile(LogFlags {
            net_log: true,
            ..Default::default()
        });
        assert!(p.argv().iter().any(|a| a == "-netLog"));
    }

    #[test]
    fn diag_server_launch_kind_renders_server_flag() {
        // A diag build acting as a local server is the one kind that must pass
        // `-server`.
        let p = ServerProfile {
            kind: LaunchKind::DiagServer,
            ..profile(LogFlags::default())
        };
        assert!(p.argv().iter().any(|a| a == "-server"));
    }

    fn client() -> ClientProfile {
        ClientProfile {
            app_id: 221100,
            mods: vec!["@p3diag".into(), "@other".into()],
            connect: Some(Connect {
                host: "192.168.1.254".into(),
                game_port: 2402,
                query_port: Some(27016),
            }),
            profile_name: Some("mftest".into()),
            extra: vec![],
            flags: LogFlags::default(),
        }
    }

    #[test]
    fn connect_renders_game_and_query_triple() {
        assert_eq!(
            Connect {
                host: "10.0.0.5".into(),
                game_port: 2302,
                query_port: Some(27016)
            }
            .to_arg(),
            "10.0.0.5:2302:27016"
        );
        // Query port optional.
        assert_eq!(
            Connect {
                host: "10.0.0.5".into(),
                game_port: 2302,
                query_port: None
            }
            .to_arg(),
            "10.0.0.5:2302"
        );
    }

    #[test]
    fn connect_parse_roundtrips_and_rejects_malformed() {
        assert_eq!(
            Connect::parse("192.168.1.254:2402:27016"),
            Some(Connect {
                host: "192.168.1.254".into(),
                game_port: 2402,
                query_port: Some(27016)
            })
        );
        assert_eq!(
            Connect::parse("10.0.0.5:2302"),
            Some(Connect {
                host: "10.0.0.5".into(),
                game_port: 2302,
                query_port: None
            })
        );
        // Round-trips through to_arg.
        let c = Connect::parse("1.2.3.4:9999:1").unwrap();
        assert_eq!(Connect::parse(&c.to_arg()), Some(c));
        // Malformed inputs.
        assert_eq!(Connect::parse(""), None);
        assert_eq!(Connect::parse("host"), None);
        assert_eq!(Connect::parse("host:notaport"), None);
        assert_eq!(Connect::parse("host:70000"), None); // port > u16
        assert_eq!(Connect::parse("host:1:2:3"), None); // too many fields
    }

    #[test]
    fn client_argv_renders_mod_connect_and_name() {
        let argv = client().argv();
        assert_eq!(argv[0], "-mod=@p3diag;@other");
        assert_eq!(argv[1], "-connect=192.168.1.254:2402:27016");
        assert_eq!(argv[2], "-name=mftest");
        // A client never passes the server-only flags.
        assert!(!argv.iter().any(|a| a == "-server"));
        assert!(!argv.iter().any(|a| a.starts_with("-config=")));
        assert!(!argv.iter().any(|a| a.starts_with("-profiles=")));
    }

    #[test]
    fn client_argv_omits_absent_optionals_and_honors_log_flags() {
        let mut p = client();
        p.connect = None;
        p.profile_name = None;
        p.mods = vec![];
        p.flags = LogFlags {
            do_logs: true,
            file_patching: true,
            ..Default::default()
        };
        let argv = p.argv();
        assert!(argv.iter().any(|a| a == "-mod="));
        assert!(!argv.iter().any(|a| a.starts_with("-connect=")));
        assert!(!argv.iter().any(|a| a.starts_with("-name=")));
        assert!(argv.iter().any(|a| a == "-filePatching"));
        assert!(argv.iter().any(|a| a == "-dologs"));
    }

    #[test]
    fn client_steam_argv_wraps_applaunch_with_appid() {
        // The explicit launcher override still renders `steam -applaunch <appid>`.
        let argv = client().steam_argv();
        assert_eq!(argv[0], "-applaunch");
        assert_eq!(argv[1], "221100");
        assert_eq!(argv[2], "-mod=@p3diag;@other");
        assert!(argv
            .iter()
            .any(|a| a == "-connect=192.168.1.254:2402:27016"));
    }

    #[test]
    fn client_extra_args_are_appended_verbatim_before_flags() {
        // e.g. `-nolauncher -world=empty` used for a headless dev join.
        let mut p = client();
        p.extra = vec!["-nolauncher".into(), "-world=empty".into()];
        p.flags.do_logs = true;
        let argv = p.argv();
        let nl = argv.iter().position(|a| a == "-nolauncher").unwrap();
        let wl = argv.iter().position(|a| a == "-world=empty").unwrap();
        let dl = argv.iter().position(|a| a == "-dologs").unwrap();
        assert!(nl < wl && wl < dl);
    }
}
