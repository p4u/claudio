//! `claudio --plain [--proxy NAME | --no-proxy] [claude args…]`
//!
//! Plain claude, with the proxy environment the manager would give a new
//! session: no daemon, PTY, tabs or hooks. The real `claude` is `exec`ed, so
//! the tty, signals and exit code are exactly claude's. The proxy token
//! reaches it through the environment only, never argv.

use std::process::{Command, ExitCode};
use std::time::Duration;

use crate::claude;
use crate::proxy::{api, env::env_diff, resolve, ProxyChoice};

/// How long to wait for the proxy's `/v1/claudio/config` before falling back
/// to the built-in model defaults.
const CONFIG_TIMEOUT: Duration = Duration::from_secs(2);

pub fn run(args: &[String]) -> ExitCode {
    let (choice, user_args) = match split_args(args) {
        Ok(split) => split,
        Err(msg) => {
            eprintln!("claudio: {msg}");
            return ExitCode::from(2);
        }
    };
    let env = match proxy_env(&choice) {
        Ok(env) => env,
        Err(msg) => {
            eprintln!("claudio: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let mut cmd = Command::new(claude::binary());
    cmd.args(claude_args(user_args));
    apply_env(&mut cmd, &env);
    claude::exec(cmd)
}

/// The arguments claude is started with. The one place to add arguments
/// claudio passes on top of the user's.
fn claude_args(user_args: Vec<String>) -> Vec<String> {
    user_args
}

/// Split what follows `--plain` into the proxy choice (leading `--proxy NAME`
/// or `--no-proxy`) and the arguments for claude. `-p`, `--print` and `--api`
/// are rejected: claudio never runs `claude -p`, and there is no session
/// manager here to emulate it.
fn split_args(args: &[String]) -> Result<(ProxyChoice, Vec<String>), String> {
    let (choice, rest) = match args {
        [flag] if flag == "--proxy" => return Err("--proxy requires a profile name".into()),
        [flag, name, rest @ ..] if flag == "--proxy" => (ProxyChoice::Profile(name.clone()), rest),
        [flag, rest @ ..] if flag == "--no-proxy" => (ProxyChoice::Direct, rest),
        _ => (ProxyChoice::Default, args),
    };
    // Everything after a bare `--` is the prompt, not flags.
    let flags = rest.split(|a| a == "--").next().unwrap_or_default();
    if let Some(bad) = flags
        .iter()
        .find(|a| matches!(a.as_str(), "-p" | "--print" | "--api"))
    {
        return Err(format!(
            "{bad} is not supported with --plain (it starts the real interactive claude); \
             use `claudio {bad} …` for print mode or the API server"
        ));
    }
    Ok((choice, rest.to_vec()))
}

/// The proxy environment for `choice`: nothing when no proxy applies,
/// otherwise the profile's session env with the proxy's model config (or the
/// built-in defaults, after one stderr line, if the proxy doesn't answer in
/// time).
fn proxy_env(choice: &ProxyChoice) -> Result<Vec<(String, String)>, String> {
    let (_, default) = resolve::load_proxy_profiles();
    let name = choice.pick(default.as_deref());
    let config = name
        .and_then(resolve::resolve_profile)
        .and_then(|(url, token)| fetch_config(&url, &token));
    resolve::proxy_env_for(name, config.as_ref())
}

/// Fetch the proxy's config within [`CONFIG_TIMEOUT`]; `None` (with a note on
/// stderr) when it is unavailable.
fn fetch_config(url: &str, token: &str) -> Option<api::ConfigResponse> {
    let config = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
        .and_then(|rt| {
            // The timer needs the runtime, so build the timeout inside it.
            rt.block_on(async {
                tokio::time::timeout(CONFIG_TIMEOUT, api::fetch_config(url, token)).await
            })
            .ok()
            .and_then(Result::ok)
            .flatten()
        });
    if config.is_none() {
        eprintln!("claudio: proxy config unavailable, using built-in model defaults");
    }
    config
}

/// Apply the shared [`env_diff`] to a `std::process::Command`.
fn apply_env(cmd: &mut Command, env: &[(String, String)]) {
    let diff = env_diff(env);
    for key in diff.remove {
        cmd.env_remove(key);
    }
    for (k, v) in diff.set {
        cmd.env(k, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plain_args_default_to_the_default_proxy() {
        let (choice, rest) = split_args(&args(&["--resume", "abc"])).unwrap();
        assert_eq!(choice, ProxyChoice::Default);
        assert_eq!(rest, args(&["--resume", "abc"]));
    }

    #[test]
    fn leading_proxy_flags_are_consumed() {
        let (choice, rest) = split_args(&args(&["--proxy", "work", "-c"])).unwrap();
        assert_eq!(choice, ProxyChoice::Profile("work".into()));
        assert_eq!(rest, args(&["-c"]));

        let (choice, rest) = split_args(&args(&["--no-proxy", "-c"])).unwrap();
        assert_eq!(choice, ProxyChoice::Direct);
        assert_eq!(rest, args(&["-c"]));
    }

    #[test]
    fn proxy_flag_needs_a_name() {
        assert!(split_args(&args(&["--proxy"])).is_err());
    }

    #[test]
    fn print_and_api_are_rejected_with_a_hint() {
        for bad in ["-p", "--print", "--api"] {
            let err = split_args(&args(&["--no-proxy", "-c", bad])).unwrap_err();
            assert!(err.contains(bad) && err.contains("claudio"), "{err}");
        }
    }

    #[test]
    fn a_prompt_after_double_dash_is_not_inspected() {
        let (_, rest) = split_args(&args(&["--", "-p"])).unwrap();
        assert_eq!(rest, args(&["--", "-p"]));
    }

    #[test]
    fn claude_args_pass_through() {
        assert_eq!(claude_args(args(&["-c"])), args(&["-c"]));
    }

    #[test]
    fn env_scrubs_markers_and_replaces_api_key_with_the_token() {
        let mut cmd = Command::new("claude");
        apply_env(&mut cmd, &[("ANTHROPIC_AUTH_TOKEN".into(), "tok".into())]);
        let get = |key: &str| {
            cmd.get_envs()
                .find(|(k, _)| *k == OsStr::new(key))
                .map(|(_, v)| v)
        };
        // `Some(None)` means "removed from the child's environment".
        assert_eq!(get("CLAUDECODE"), Some(None));
        assert_eq!(get("ANTHROPIC_API_KEY"), Some(None));
        assert_eq!(get("ANTHROPIC_AUTH_TOKEN"), Some(Some(OsStr::new("tok"))));
    }

    #[test]
    fn env_keeps_api_key_without_a_token() {
        let mut cmd = Command::new("claude");
        apply_env(&mut cmd, &[]);
        assert!(cmd
            .get_envs()
            .all(|(k, _)| k != OsStr::new("ANTHROPIC_API_KEY")));
        assert!(cmd
            .get_envs()
            .any(|(k, v)| k == OsStr::new("CLAUDE_CODE_ENTRYPOINT") && v.is_none()));
    }

    #[test]
    fn no_proxy_choice_yields_empty_env() {
        assert_eq!(proxy_env(&ProxyChoice::Direct), Ok(Vec::new()));
    }
}
