//! `claudio --plain [--proxy NAME | --no-proxy] [claude args…]`
//!
//! Plain claude, with the proxy environment the manager would give a new
//! session: one session in the manager's UI, without tabs, status bar or
//! wizard, and with only a few of its keys (see `tui::plain`). This module
//! parses the command line; the rest is in `tui/`.

use std::process::ExitCode;

use crate::proxy::ProxyChoice;
use crate::tui;

pub fn run(args: &[String]) -> ExitCode {
    match split_args(args) {
        Ok((choice, user_args)) => tui::run_plain(choice, user_args),
        Err(msg) => {
            eprintln!("claudio: {msg}");
            ExitCode::from(2)
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
