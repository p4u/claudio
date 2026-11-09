//! The command line and env of a session's process: claude with the
//! daemon's hook settings and the spawn's args and env, or a login shell.

use std::ffi::OsStr;
use std::path::Path;

use portable_pty::CommandBuilder;

use super::host;
use crate::addon::ClaudeFlags;
use crate::claude::hooks;
use crate::proto::SpawnSpec;
use crate::proxy::env::env_diff;

/// `<claude> --settings <hooks> [--allow-dangerously-skip-permissions]
/// [args…] [-n <name>]`, in `cwd`, with the session env applied. With a
/// `context` file, SessionStart also runs `__context file <context>`.
///
/// The skip-permissions flag is added here, on every launch path (new spawn,
/// `--resume`, resume-retry), rather than in `spec.args`, so the journal never
/// stores it. It is left out when the args already choose a permissions flag.
///
/// A `--plugin-dir`, `--agents` or `--mcp-config` pair (or `--flag=value`)
/// of the args is dropped when this claude lacks the flag (`flags`), so an
/// old claude still starts; exactly one value goes with the flag. The
/// journal keeps them for a newer claude. The args keep their order, and
/// `-n <name>` follows them: the client puts the plugin flags last, so the
/// variadic `--mcp-config` is always followed by a flag or nothing.
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
    let unknown = |flag: &str| host::LEGACY_OPTIONAL.contains(&flag) && !flags.knows(flag);
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
    apply_env(&mut cmd, cfg.path.as_deref(), &spec.env);
    cmd
}

/// Start from the daemon's env (the builder's default), apply the shared
/// [`env_diff`] (scrubs what would leak from a daemon started inside claude;
/// a proxy token replaces any API key), set the terminal type and the
/// daemon's login-shell `PATH` ([`super::Config::path`]), then set the spec's
/// env (an explicit `PATH` there wins).
fn apply_env(cmd: &mut CommandBuilder, path: Option<&OsStr>, env: &[(String, String)]) {
    let diff = env_diff(env);
    for key in diff.remove {
        cmd.env_remove(key);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    for (k, v) in diff.set {
        cmd.env(k, v);
    }
}

/// `<shell> -l` in `cwd`: a plain terminal. No hooks, no proxy env, and the
/// claude session markers are scrubbed so a `claude` typed into it is a
/// top-level session, not a child of whatever started the daemon.
pub(super) fn shell_command(shell: &Path, cwd: &Path, path: Option<&OsStr>) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(shell);
    cmd.arg("-l");
    cmd.cwd(cwd);
    apply_env(&mut cmd, path, &[]);
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
        let cmd = shell_command(Path::new("/bin/zsh"), Path::new("/tmp"), None);
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
        );
        assert_eq!(cmd.get_env("PATH"), Some(login));
        let mut cmd = CommandBuilder::new("claude");
        apply_env(
            &mut cmd,
            Some(login),
            &[("PATH".into(), "/explicit".into())],
        );
        assert_eq!(cmd.get_env("PATH"), Some(OsStr::new("/explicit")));
        // Without a login PATH, the daemon's own stays.
        let mut cmd = CommandBuilder::new("claude");
        cmd.env("PATH", "/usr/bin");
        apply_env(&mut cmd, None, &[]);
        assert_eq!(cmd.get_env("PATH"), Some(OsStr::new("/usr/bin")));
        // Terminals get it too.
        let cmd = shell_command(Path::new("/bin/sh"), Path::new("/tmp"), Some(login));
        assert_eq!(cmd.get_env("PATH"), Some(login));
    }

    #[test]
    fn api_key_kept_without_auth_token() {
        let mut cmd = CommandBuilder::new("claude");
        cmd.env("ANTHROPIC_API_KEY", "key");
        apply_env(&mut cmd, None, &[]);
        assert_eq!(cmd.get_env("ANTHROPIC_API_KEY"), Some(OsStr::new("key")));
    }

    #[test]
    fn command_line_shape() {
        let cfg = super::super::Config {
            socket: "/run/d.sock".into(),
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude: "/bin/claude".into(),
            claudio: "/bin/claudio".into(),
            skip_permissions: true,
            latex: false,
            build: None,
            path: None,
            addon_timeout: std::time::Duration::from_secs(150),
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
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude: "/bin/claude".into(),
            claudio: "/bin/claudio".into(),
            skip_permissions: true,
            latex: false,
            build: None,
            path: None,
            addon_timeout: std::time::Duration::from_secs(150),
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
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude: "/bin/claude".into(),
            claudio: "/bin/claudio".into(),
            skip_permissions: false,
            latex: false,
            build: None,
            path: None,
            addon_timeout: std::time::Duration::from_secs(150),
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
            lock: "/run/d.lock".into(),
            journal: "/j.json".into(),
            claude: "/bin/claude".into(),
            claudio: "/bin/claudio".into(),
            skip_permissions: false,
            latex: false,
            build: None,
            path: None,
            addon_timeout: std::time::Duration::from_secs(150),
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
}
