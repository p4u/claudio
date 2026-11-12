//! The command line and env of a session's process: claude with the
//! daemon's hook settings and the spawn's args and env, or a login shell.

use std::ffi::OsStr;
use std::path::Path;

use portable_pty::CommandBuilder;

use super::host;
use crate::addon::ClaudeFlags;
use crate::claude::hooks;
use crate::claude::SESSION_MARKERS;
use crate::proto::SpawnSpec;

/// `<claude> --settings <hooks> [--allow-dangerously-skip-permissions]
/// [args…] [-n <name>]`, in `cwd`, with the session env applied. With a
/// `context` file, SessionStart also runs `__context file <context>`.
///
/// The skip-permissions flag is added here, on every launch path (new spawn,
/// `--resume`, resume-retry), rather than in `spec.args`, so the journal never
/// stores it. It is left out when the args already choose a permissions flag.
///
/// An optional flag of the args (one `spec.optional` names, or one of
/// [`host::LEGACY_OPTIONAL`]) is dropped with its value (the next argument,
/// or `--flag=value`) when this claude lacks it (`flags`), so an old claude
/// still starts; exactly one value goes with the flag. The journal keeps
/// them for a newer claude. The args keep their order, and `-n <name>`
/// follows them: the client puts the plugin flags last, so the variadic
/// `--mcp-config` is always followed by a flag or nothing.
pub(super) fn command(
    cfg: &super::Config,
    spec: &SpawnSpec,
    cwd: &Path,
    token: &str,
    skip_permissions: bool,
    flags: &ClaudeFlags,
    context: Option<&Path>,
) -> CommandBuilder {
    let settings = hooks::settings(&cfg.claudio, &cfg.socket, token, context);
    let mut cmd = CommandBuilder::new(cfg.claude_bin());
    cmd.arg("--settings");
    cmd.arg(settings.to_string());
    if skip_permissions
        && !spec
            .args
            .iter()
            .any(|a| a == host::ALLOW_SKIP_PERMISSIONS || a == "--dangerously-skip-permissions")
    {
        cmd.arg(host::ALLOW_SKIP_PERMISSIONS);
    }
    let optional = |flag: &str| {
        spec.optional.iter().any(|o| o == flag) || host::LEGACY_OPTIONAL.contains(&flag)
    };
    let unknown = |flag: &str| optional(flag) && !flags.knows(flag);
    let mut args = spec.args.iter();
    while let Some(a) = args.next() {
        if unknown(a) {
            args.next();
            continue;
        }
        if a.split_once('=').is_some_and(|(flag, _)| unknown(flag)) {
            continue;
        }
        cmd.arg(a);
    }
    if let Some(name) = &spec.name {
        cmd.arg("-n");
        cmd.arg(name);
    }
    cmd.cwd(cwd);
    cfg.agent.apply(&mut cmd);
    let env_remove = spec.env_remove.as_deref();
    apply_env(&mut cmd, cfg.path.as_deref(), &spec.env, env_remove);
    cmd
}

/// Start from the daemon's env (the builder's default), remove what the
/// session loses ([`removed_env`]), set the terminal type and the daemon's
/// login-shell `PATH` ([`super::Config::path`]), then set the spec's env
/// (an explicit `PATH` there wins).
fn apply_env(
    cmd: &mut CommandBuilder,
    path: Option<&OsStr>,
    env: &[(String, String)],
    env_remove: Option<&[String]>,
) {
    for key in removed_env(env, env_remove) {
        cmd.env_remove(key);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
}

/// What a session's process loses from the daemon's env: the claude
/// session markers (which would make it a child of whatever started the
/// daemon) plus the spawn's `env_remove`, or the [`legacy_scrub`] for a
/// client that sends none.
fn removed_env<'a>(env: &[(String, String)], env_remove: Option<&'a [String]>) -> Vec<&'a str> {
    match env_remove {
        None => legacy_scrub(env),
        Some(list) => SESSION_MARKERS
            .iter()
            .copied()
            .chain(list.iter().map(String::as_str))
            .collect(),
    }
}

/// compat(v0.10): the previous release's rule, for clients that send no
/// `env_remove` (docs/daemon-protocol.md, "Mixed versions"). The session
/// markers go; a proxy token (`ANTHROPIC_AUTH_TOKEN` in the env) replaces
/// any inherited `ANTHROPIC_API_KEY`, which would otherwise win over it; and
/// a first-party proxy env (one without `CLAUDE_CODE_USE_GATEWAY`) drops an
/// inherited `CLAUDE_CODE_USE_GATEWAY`, which would turn the feature flags
/// off again.
fn legacy_scrub(env: &[(String, String)]) -> Vec<&'static str> {
    let has = |key: &str| env.iter().any(|(k, _)| k == key);
    let mut remove = SESSION_MARKERS.to_vec();
    if has("ANTHROPIC_AUTH_TOKEN") {
        remove.push("ANTHROPIC_API_KEY");
        if !has("CLAUDE_CODE_USE_GATEWAY") {
            remove.push("CLAUDE_CODE_USE_GATEWAY");
        }
    }
    remove
}

/// At most this many names in a spawn's `env_remove`.
const MAX_ENV_REMOVE: usize = 64;

/// Why a spawn's `env_remove` is refused: more than [`MAX_ENV_REMOVE`]
/// names, or a name that is not `[A-Za-z_][A-Za-z0-9_]{0,127}`. The
/// reason gives the name's position, never the name (it may be a value
/// sent by mistake).
pub(super) fn env_remove_ok(list: &[String]) -> Result<(), String> {
    if list.len() > MAX_ENV_REMOVE {
        return Err(format!("more than {MAX_ENV_REMOVE} names"));
    }
    let ok = |name: &str| {
        let mut chars = name.chars();
        name.len() <= 128
            && chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    match list.iter().position(|name| !ok(name)) {
        Some(i) => Err(format!("name {} is not an env var name", i + 1)),
        None => Ok(()),
    }
}

/// At most this many flags in a spawn's `optional`.
const MAX_OPTIONAL: usize = 16;

/// Why a spawn's `optional` is refused: more than [`MAX_OPTIONAL`] flags,
/// or one that is not `--[a-z0-9][a-z0-9-]{0,39}`.
pub(super) fn optional_ok(list: &[String]) -> Result<(), String> {
    if list.len() > MAX_OPTIONAL {
        return Err(format!("more than {MAX_OPTIONAL} flags"));
    }
    let ok = |flag: &str| {
        let name = flag.strip_prefix("--").unwrap_or_default();
        let mut chars = name.chars();
        name.len() <= 40
            && chars
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    match list.iter().position(|flag| !ok(flag)) {
        Some(i) => Err(format!("flag {} is not --[a-z0-9][a-z0-9-]*", i + 1)),
        None => Ok(()),
    }
}

/// `<shell> -l` in `cwd`: a plain terminal. No hooks, no proxy env, and
/// only the claude session markers are scrubbed, so a `claude` typed into it
/// is a top-level session, not a child of whatever started the daemon. It
/// gets the login `PATH` and the agent relay as a claude session does.
pub(super) fn shell_command(shell: &Path, cwd: &Path, cfg: &super::Config) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(shell);
    cmd.arg("-l");
    cmd.cwd(cwd);
    cfg.agent.apply(&mut cmd);
    apply_env(&mut cmd, cfg.path.as_deref(), &[], Some(&[]));
    cmd
}

/// claude animates its terminal title with a leading status glyph
/// (`✳ Claude Code`, `◐ Fix the parser`); strip it so the title only changes
/// — and is only broadcast — when the actual text does.
pub(super) fn clean_title(title: &str) -> &str {
    title
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::Value;

    use super::*;

    #[test]
    fn title_glyphs_are_stripped() {
        assert_eq!(clean_title("◐ Claude Code"), "Claude Code");
        assert_eq!(clean_title("✳ Fix the parser"), "Fix the parser");
        assert_eq!(clean_title("plain"), "plain");
        assert_eq!(clean_title("◑ "), "");
    }

    /// A daemon config whose login `PATH` is `path`, its relay not listening.
    fn daemon_config(path: Option<&OsStr>) -> super::super::Config {
        super::super::Config {
            path: path.map(OsStr::to_os_string),
            ..super::super::test_support::TestDaemon::config(Path::new("/run"))
        }
    }

    /// Flags as a claude whose `--help` lists those marked true.
    fn knowing(flags: &[(&str, bool)]) -> ClaudeFlags {
        let help: Vec<&str> = flags
            .iter()
            .filter(|(_, on)| *on)
            .map(|(f, _)| *f)
            .collect();
        ClaudeFlags::from_help(&help.join("\n"))
    }

    #[test]
    fn env_is_scrubbed_and_token_replaces_api_key() {
        let mut cmd = CommandBuilder::new("claude");
        cmd.env("CLAUDECODE", "1");
        cmd.env("CLAUDE_CODE_ENTRYPOINT", "cli");
        cmd.env("CLAUDE_CODE_CHILD_SESSION", "1");
        cmd.env("CLAUDE_CODE_USE_GATEWAY", "1");
        cmd.env("MY_SETTING", "keep");
        cmd.env("ANTHROPIC_API_KEY", "old");
        apply_env(
            &mut cmd,
            None,
            &[("ANTHROPIC_AUTH_TOKEN".into(), "tok".into())],
            None,
        );
        assert_eq!(cmd.get_env("CLAUDECODE"), None);
        assert_eq!(cmd.get_env("CLAUDE_CODE_ENTRYPOINT"), None);
        assert_eq!(cmd.get_env("CLAUDE_CODE_CHILD_SESSION"), None);
        // A first-party env drops an inherited gateway flag; user settings survive.
        assert_eq!(cmd.get_env("CLAUDE_CODE_USE_GATEWAY"), None);
        assert_eq!(
            cmd.get_env("MY_SETTING").and_then(|v| v.to_str()),
            Some("keep")
        );
        assert_eq!(cmd.get_env("ANTHROPIC_API_KEY"), None);
        assert_eq!(cmd.get_env("ANTHROPIC_AUTH_TOKEN"), Some(OsStr::new("tok")));
        assert_eq!(cmd.get_env("TERM"), Some(OsStr::new("xterm-256color")));
        assert_eq!(cmd.get_env("COLORTERM"), Some(OsStr::new("truecolor")));
    }

    #[test]
    fn shell_command_is_a_plain_login_shell() {
        let cmd = shell_command(
            Path::new("/bin/zsh"),
            Path::new("/tmp"),
            &daemon_config(None),
        );
        let argv: Vec<_> = cmd
            .get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, ["/bin/zsh", "-l"], "no --settings, no hooks");
        assert_eq!(
            cmd.get_cwd().map(|c| c.as_os_str()),
            Some(OsStr::new("/tmp"))
        );
        assert_eq!(cmd.get_env("TERM"), Some(OsStr::new("xterm-256color")));
        assert_eq!(cmd.get_env("CLAUDECODE"), None);
    }

    #[test]
    fn login_path_is_set_unless_the_spec_sets_one() {
        let login = OsStr::new("/home/u/.npm-packages/bin:/usr/bin");
        let mut cmd = CommandBuilder::new("claude");
        cmd.env("PATH", "/usr/bin");
        // A proxy env (no PATH of its own) keeps the login PATH.
        apply_env(
            &mut cmd,
            Some(login),
            &[("ANTHROPIC_AUTH_TOKEN".into(), "t".into())],
            None,
        );
        assert_eq!(cmd.get_env("PATH"), Some(login));
        let mut cmd = CommandBuilder::new("claude");
        apply_env(
            &mut cmd,
            Some(login),
            &[("PATH".into(), "/explicit".into())],
            Some(&[]),
        );
        assert_eq!(cmd.get_env("PATH"), Some(OsStr::new("/explicit")));
        // Without a login PATH, the daemon's own stays.
        let mut cmd = CommandBuilder::new("claude");
        cmd.env("PATH", "/usr/bin");
        apply_env(&mut cmd, None, &[], None);
        assert_eq!(cmd.get_env("PATH"), Some(OsStr::new("/usr/bin")));
        // Terminals get it too.
        let cmd = shell_command(
            Path::new("/bin/sh"),
            Path::new("/tmp"),
            &daemon_config(Some(login)),
        );
        assert_eq!(cmd.get_env("PATH"), Some(login));
    }

    #[test]
    fn api_key_kept_without_auth_token() {
        let mut cmd = CommandBuilder::new("claude");
        cmd.env("ANTHROPIC_API_KEY", "key");
        apply_env(&mut cmd, None, &[], None);
        assert_eq!(cmd.get_env("ANTHROPIC_API_KEY"), Some(OsStr::new("key")));
    }

    #[test]
    fn command_line_shape() {
        let cfg = super::super::Config {
            socket: "/run/d.sock".into(),
            agent: Default::default(),
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude: "/bin/claude".into(),
            claudio: "/bin/claudio".into(),
            skip_permissions: true,
            latex: false,
            build: None,
            path: None,
            addon_timeout: std::time::Duration::from_secs(150),
            old_peer: false,
            config_file: None,
            service_timing: Default::default(),
        };
        let spec = SpawnSpec {
            id: uuid::Uuid::nil(),
            cwd: "/".into(),
            name: Some("work".into()),
            args: vec!["--resume".into(), "abc".into()],
            env: vec![],
            rows: 24,
            cols: 80,
            ephemeral: false,
            extensions: None,
            env_remove: None,
            optional: vec![],
        };
        let flags = ClaudeFlags::default();
        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", false, &flags, None);
        let argv = argv_of(&cmd);
        assert_eq!(argv[0], "/bin/claude");
        assert_eq!(argv[1], "--settings");
        let settings: Value = serde_json::from_str(&argv[2]).unwrap();
        let hook = settings["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(hook.contains("tok123") && hook.contains("/run/d.sock"));
        assert_eq!(&argv[3..], ["--resume", "abc", "-n", "work"]);

        // With skip-permissions on, the flag follows --settings, ahead of
        // the spec args (which are what the journal stores).
        let cmd = command(&cfg, &spec, Path::new("/"), "tok123", true, &flags, None);
        assert_eq!(
            &argv_of(&cmd)[3..],
            [ALLOW, "--resume", "abc", "-n", "work"]
        );
        assert_eq!(spec.args, ["--resume", "abc"]);

        // A context file adds its hook to SessionStart: the path, not the text.
        let start = |context: Option<&Path>| {
            let argv = argv_of(&command(
                &cfg,
                &spec,
                Path::new("/"),
                "t",
                false,
                &flags,
                context,
            ));
            let settings: Value = serde_json::from_str(&argv[2]).unwrap();
            settings["hooks"]["SessionStart"][0]["hooks"].clone()
        };
        assert_eq!(start(None).as_array().unwrap().len(), 1);
        let ctx = Path::new("/run/context/x.txt");
        assert_eq!(
            start(Some(ctx))[1]["command"],
            "'/bin/claudio' __context file '/run/context/x.txt'"
        );
    }

    const ALLOW: &str = "--allow-dangerously-skip-permissions";

    fn argv_of(cmd: &CommandBuilder) -> Vec<String> {
        cmd.get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn skip_permissions_not_duplicated() {
        let cfg = super::super::Config {
            socket: "/run/d.sock".into(),
            agent: Default::default(),
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude: "/bin/claude".into(),
            claudio: "/bin/claudio".into(),
            skip_permissions: true,
            latex: false,
            build: None,
            path: None,
            addon_timeout: std::time::Duration::from_secs(150),
            old_peer: false,
            config_file: None,
            service_timing: Default::default(),
        };
        for given in [ALLOW, "--dangerously-skip-permissions"] {
            let spec = SpawnSpec {
                id: uuid::Uuid::nil(),
                cwd: "/".into(),
                name: None,
                args: vec![given.into()],
                env: vec![],
                rows: 24,
                cols: 80,
                ephemeral: false,
                extensions: None,
                env_remove: None,
                optional: vec![],
            };
            let flags = ClaudeFlags::default();
            let argv = argv_of(&command(
                &cfg,
                &spec,
                Path::new("/"),
                "t",
                true,
                &flags,
                None,
            ));
            assert_eq!(&argv[3..], [given]);
        }
    }

    #[test]
    fn plugin_dirs_are_dropped_for_a_claude_without_the_flag() {
        let cfg = super::super::Config {
            socket: "/run/d.sock".into(),
            agent: Default::default(),
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude: "/bin/claude".into(),
            claudio: "/bin/claudio".into(),
            skip_permissions: false,
            latex: false,
            build: None,
            path: None,
            addon_timeout: std::time::Duration::from_secs(150),
            old_peer: false,
            config_file: None,
            service_timing: Default::default(),
        };
        let spec = SpawnSpec {
            id: uuid::Uuid::nil(),
            cwd: "/".into(),
            name: None,
            args: vec![
                "--plugin-dir".into(),
                "/c/kb.zip".into(),
                "--resume".into(),
                "abc".into(),
                "--plugin-dir=/c/x.zip".into(),
                "--plugin-dir".into(),
            ],
            env: vec![],
            rows: 24,
            cols: 80,
            ephemeral: false,
            extensions: None,
            env_remove: None,
            optional: vec![],
        };
        let argv = |plugin_dir: bool| {
            let flags = knowing(&[("--plugin-dir", plugin_dir)]);
            argv_of(&command(
                &cfg,
                &spec,
                Path::new("/"),
                "t",
                false,
                &flags,
                None,
            ))
        };
        assert_eq!(&argv(false)[3..], ["--resume", "abc"]);
        assert_eq!(&argv(true)[3..], &spec.args[..]);
    }

    /// The plugin flags stay where the client put them, after every other
    /// argument and right before `-n <name>`, so the variadic
    /// `--mcp-config` never takes a positional; a claude without one of
    /// them loses exactly that flag and its value.
    #[test]
    fn plugin_flags_keep_their_order_and_are_dropped_when_unknown() {
        let cfg = super::super::Config {
            socket: "/run/d.sock".into(),
            agent: Default::default(),
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude: "/bin/claude".into(),
            claudio: "/bin/claudio".into(),
            skip_permissions: false,
            latex: false,
            build: None,
            path: None,
            addon_timeout: std::time::Duration::from_secs(150),
            old_peer: false,
            config_file: None,
            service_timing: Default::default(),
        };
        let agents = r#"{"reviewer":{"description":"d","prompt":"p"}}"#;
        let mcp = r#"{"mcpServers":{"kb":{"type":"http","url":"https://e.org"}}}"#;
        let spec = SpawnSpec {
            id: uuid::Uuid::nil(),
            cwd: "/".into(),
            name: Some("s1".into()),
            args: vec![
                "--resume".into(),
                "abc".into(),
                "--agents".into(),
                agents.into(),
                "--mcp-config".into(),
                mcp.into(),
                "--agents".into(),
                agents.into(),
                format!("--mcp-config={mcp}"),
            ],
            env: vec![],
            rows: 24,
            cols: 80,
            ephemeral: false,
            extensions: None,
            env_remove: None,
            optional: vec![],
        };
        let argv = |agents: bool, mcp_config: bool| {
            let flags = knowing(&[("--agents", agents), ("--mcp-config", mcp_config)]);
            argv_of(&command(
                &cfg,
                &spec,
                Path::new("/"),
                "t",
                false,
                &flags,
                None,
            ))
        };
        let all = argv(true, true);
        assert_eq!(&all[3..12], &spec.args[..]);
        assert_eq!(&all[12..], ["-n", "s1"]);
        let mcp_eq = format!("--mcp-config={mcp}");
        assert_eq!(
            &argv(false, true)[3..],
            ["--resume", "abc", "--mcp-config", mcp, &mcp_eq, "-n", "s1"]
        );
        assert_eq!(
            &argv(true, false)[3..],
            ["--resume", "abc", "--agents", agents, "--agents", agents, "-n", "s1"]
        );
        assert_eq!(&argv(false, false)[3..], ["--resume", "abc", "-n", "s1"]);
    }

    /// A flag the spawn marks optional goes, with its value, only when this
    /// claude lacks it; a flag it does not mark (and that is not one of the
    /// legacy three) is always passed.
    #[test]
    fn optional_flags_are_dropped_only_when_unknown() {
        let cfg = super::super::Config {
            socket: "/run/d.sock".into(),
            agent: Default::default(),
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude: "/bin/claude".into(),
            claudio: "/bin/claudio".into(),
            skip_permissions: false,
            latex: false,
            build: None,
            path: None,
            addon_timeout: std::time::Duration::from_secs(150),
            old_peer: false,
            config_file: None,
            service_timing: Default::default(),
        };
        let spec = SpawnSpec {
            args: vec![
                "--x-test".into(),
                "v".into(),
                "--x-test=w".into(),
                "--y-test".into(),
                "u".into(),
            ],
            optional: vec!["--x-test".into()],
            ..Default::default()
        };
        let argv = |help: &str| {
            let flags = ClaudeFlags::from_help(help);
            argv_of(&command(
                &cfg,
                &spec,
                Path::new("/"),
                "t",
                false,
                &flags,
                None,
            ))[3..]
                .to_vec()
        };
        assert_eq!(argv(""), ["--y-test", "u"]);
        assert_eq!(argv("  --x-test <v>  A test flag"), spec.args);
        // Not marked: passed whether claude lists it or not.
        let unmarked = SpawnSpec {
            optional: Vec::new(),
            ..spec.clone()
        };
        let flags = ClaudeFlags::default();
        let got = argv_of(&command(
            &cfg,
            &unmarked,
            Path::new("/"),
            "t",
            false,
            &flags,
            None,
        ));
        assert_eq!(&got[3..], &unmarked.args[..]);
    }

    #[test]
    fn optional_is_capped_and_holds_flags_only() {
        let flags = |list: &[&str]| list.iter().map(|f| f.to_string()).collect::<Vec<_>>();
        assert!(optional_ok(&[]).is_ok());
        assert!(optional_ok(&flags(&[
            "--agents",
            "--x-1",
            &format!("--{}", "a".repeat(40))
        ]))
        .is_ok());
        for bad in [
            "agents",
            "-a",
            "--",
            "--Agents",
            "--a_b",
            "--a=b",
            "---a",
            &format!("--{}", "a".repeat(41)),
        ] {
            let why = optional_ok(&flags(&["--ok", bad])).unwrap_err();
            assert_eq!(why, "flag 2 is not --[a-z0-9][a-z0-9-]*", "{bad}");
        }
        assert_eq!(
            optional_ok(&vec!["--a".to_owned(); 17]).unwrap_err(),
            "more than 16 flags"
        );
        assert!(optional_ok(&vec!["--a".to_owned(); 16]).is_ok());
    }

    /// A builder whose inherited env holds `vars`.
    fn inheriting(vars: &[(&str, &str)]) -> CommandBuilder {
        let mut cmd = CommandBuilder::new("claude");
        for (k, v) in vars {
            cmd.env(k, v);
        }
        cmd
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Whether `cmd` sets `key` (values are never printed, C1).
    fn has(cmd: &CommandBuilder, key: &str) -> bool {
        cmd.get_env(key).is_some()
    }

    const INHERITED: [(&str, &str); 4] = [
        ("ANTHROPIC_API_KEY", "inherited-key"),
        ("CLAUDE_CODE_USE_GATEWAY", "1"),
        ("CLAUDECODE", "1"),
        ("MY_SETTING", "keep"),
    ];

    /// compat(v0.10): a spawn without `env_remove` gets the previous
    /// release's rule (it replaces the tests of `proxy::env::env_diff`).
    #[test]
    fn old_client_without_env_remove_gets_the_compatibility_scrub() {
        // A proxy token replaces the inherited key; a first-party env (no
        // gateway flag of its own) drops the inherited gateway flag.
        let mut cmd = inheriting(&INHERITED);
        let token = pairs(&[("ANTHROPIC_AUTH_TOKEN", "t")]);
        apply_env(&mut cmd, None, &token, None);
        assert!(!has(&cmd, "ANTHROPIC_API_KEY"));
        assert!(!has(&cmd, "CLAUDE_CODE_USE_GATEWAY"));
        assert!(!has(&cmd, "CLAUDECODE"));
        assert!(has(&cmd, "MY_SETTING") && has(&cmd, "ANTHROPIC_AUTH_TOKEN"));
        // A gateway env brings its own flag.
        let mut cmd = inheriting(&INHERITED);
        let gateway = pairs(&[
            ("ANTHROPIC_AUTH_TOKEN", "t"),
            ("CLAUDE_CODE_USE_GATEWAY", "env"),
        ]);
        apply_env(&mut cmd, None, &gateway, None);
        assert!(!has(&cmd, "ANTHROPIC_API_KEY"));
        assert!(
            cmd.get_env("CLAUDE_CODE_USE_GATEWAY") == Some(OsStr::new("env")),
            "the env's gateway flag wins"
        );
        // A user's own traffic and compact-window settings stay in both.
        for env in [&token, &gateway] {
            let mut cmd = inheriting(&[
                ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
                ("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "1000000"),
            ]);
            apply_env(&mut cmd, None, env, None);
            assert!(has(&cmd, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"));
            assert!(has(&cmd, "CLAUDE_CODE_AUTO_COMPACT_WINDOW"));
        }
        // No token (other entries alone do not count as one): the key and
        // the gateway flag stay, the markers go.
        for env in [vec![], pairs(&[("API_TIMEOUT_MS", "1")])] {
            let mut cmd = inheriting(&INHERITED);
            apply_env(&mut cmd, None, &env, None);
            assert!(has(&cmd, "ANTHROPIC_API_KEY") && has(&cmd, "CLAUDE_CODE_USE_GATEWAY"));
            assert!(!has(&cmd, "CLAUDECODE"));
        }
        let mut markers = removed_env(&[], None);
        markers.sort_unstable();
        let mut expected = SESSION_MARKERS.to_vec();
        expected.sort_unstable();
        assert_eq!(markers, expected);
    }

    #[test]
    fn env_remove_some_removes_exactly_the_markers_and_the_list() {
        let token = pairs(&[("ANTHROPIC_AUTH_TOKEN", "t")]);
        // `Some([])`: the markers only, even with a token in the env.
        let mut cmd = inheriting(&INHERITED);
        apply_env(&mut cmd, None, &token, Some(&[]));
        assert!(has(&cmd, "ANTHROPIC_API_KEY") && has(&cmd, "CLAUDE_CODE_USE_GATEWAY"));
        assert!(!has(&cmd, "CLAUDECODE"));
        // The list goes as well, and nothing else.
        let list = ["ANTHROPIC_API_KEY".to_owned()];
        let mut cmd = inheriting(&INHERITED);
        apply_env(&mut cmd, None, &token, Some(&list));
        assert!(!has(&cmd, "ANTHROPIC_API_KEY") && !has(&cmd, "CLAUDECODE"));
        assert!(has(&cmd, "CLAUDE_CODE_USE_GATEWAY") && has(&cmd, "MY_SETTING"));
        let removed = removed_env(&token, Some(&list));
        assert_eq!(removed.len(), SESSION_MARKERS.len() + 1);
        assert!(SESSION_MARKERS.iter().all(|m| removed.contains(m)));
        // A name the env also sets: the env's value is the one claude sees.
        let mut cmd = inheriting(&INHERITED);
        let own = pairs(&[("ANTHROPIC_API_KEY", "own")]);
        apply_env(&mut cmd, None, &own, Some(&list));
        assert!(cmd.get_env("ANTHROPIC_API_KEY") == Some(OsStr::new("own")));
    }

    #[test]
    fn a_shell_loses_only_the_markers() {
        let cmd = shell_command(Path::new("/bin/sh"), Path::new("/"), &daemon_config(None));
        // Every inherited variable but the markers is still there.
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy();
            let marker = SESSION_MARKERS.contains(&&*key);
            assert_eq!(has(&cmd, &key), !marker, "{key}");
        }
        assert!(!has(&cmd, "CLAUDECODE"));
        // Never the key, whatever env a shell spec carried.
        let token = pairs(&[("ANTHROPIC_AUTH_TOKEN", "t")]);
        assert_eq!(removed_env(&token, Some(&[])), SESSION_MARKERS.to_vec());
    }

    #[test]
    fn env_remove_is_capped_and_holds_names_only() {
        let names = |list: &[&str]| list.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert!(env_remove_ok(&[]).is_ok());
        assert!(env_remove_ok(&names(&["ANTHROPIC_API_KEY", "_x", "a1"])).is_ok());
        assert!(env_remove_ok(&names(&[&"A".repeat(128)])).is_ok());
        for bad in ["", "1A", "A-B", "A=B", "sk-ant secret", &"A".repeat(129)] {
            let why = env_remove_ok(&names(&["OK", bad])).unwrap_err();
            assert_eq!(why, "name 2 is not an env var name");
        }
        assert_eq!(
            env_remove_ok(&vec!["A".to_owned(); 65]).unwrap_err(),
            "more than 64 names"
        );
        assert!(env_remove_ok(&vec!["A".to_owned(); 64]).is_ok());
    }
}
