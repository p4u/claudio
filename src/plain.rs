//! `claudio --plain [<launch flags>] [claude args…]`
//!
//! Plain claude, routed as the manager would route a new session: one
//! session in the manager's UI, without tabs, status bar or wizard, and
//! with only a few of its keys (see `tui::plain`). The launch flags are the
//! addons' (`--proxy NAME`, `--no-proxy`, `--first-party`). This module
//! parses the command line; the rest is in `tui/`.

use std::process::ExitCode;
use std::sync::Arc;

use crate::addon::{Addons, LaunchSelect};
use crate::tui;

pub fn run(args: &[String], addons: Arc<Addons>) -> ExitCode {
    match split_args(&addons, args) {
        Ok((select, user_args)) => tui::run_plain(addons, select, user_args),
        Err(msg) => {
            eprintln!("claudio: {msg}");
            ExitCode::from(2)
        }
    }
}

/// Split what follows `--plain` into the leading launch flags of any addon
/// and the arguments for claude. `-p`, `--print` and `--api` are rejected:
/// claudio never runs `claude -p`, and there is no session manager here to
/// emulate it.
fn split_args(addons: &Addons, args: &[String]) -> Result<(LaunchSelect, Vec<String>), String> {
    let (select, rest) = crate::app::launch::split_flags(addons, args)?;
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
    Ok((select, rest.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon::test_support::TestAddon;
    use crate::addon::{Addon, FlagDecl, Pick};

    const FLAGS: &[FlagDecl] = &[
        FlagDecl {
            name: "--route",
            value: Some("NAME"),
            help: "",
            label: None,
        },
        FlagDecl {
            name: "--no-route",
            value: None,
            help: "",
            label: None,
        },
        FlagDecl {
            name: "--switch",
            value: None,
            help: "",
            label: Some("Switch"),
        },
    ];

    fn addons() -> Addons {
        let a = TestAddon::new("a").launch_flags(FLAGS);
        Addons::new(vec![Arc::new(a) as Arc<dyn Addon>]).unwrap()
    }

    fn split(list: &[&str]) -> Result<(LaunchSelect, Vec<String>), String> {
        split_args(&addons(), &args(list))
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn option(name: &str) -> Pick {
        Pick::Option {
            addon: "a".into(),
            option: name.into(),
        }
    }

    #[test]
    fn plain_args_default_to_the_addons_default() {
        let (select, rest) = split(&["--resume", "abc"]).unwrap();
        assert_eq!(select, LaunchSelect::default());
        assert_eq!(rest, args(&["--resume", "abc"]));
    }

    #[test]
    fn leading_launch_flags_are_consumed() {
        let (select, rest) = split(&["--route", "work", "-c"]).unwrap();
        assert_eq!(select.pick, option("work"));
        assert!(select.flags.is_empty());
        assert_eq!(rest, args(&["-c"]));

        let (select, rest) = split(&["--no-route", "-c"]).unwrap();
        assert_eq!(select.pick, Pick::Direct);
        assert_eq!(rest, args(&["-c"]));
    }

    #[test]
    fn a_switch_is_consumed_in_any_order() {
        let (select, rest) = split(&["--switch", "--route", "work", "-c"]).unwrap();
        assert_eq!(select.pick, option("work"));
        assert_eq!(select.flags, ["--switch"]);
        assert_eq!(rest, args(&["-c"]));
    }

    #[test]
    fn a_flag_with_a_value_needs_one() {
        assert!(split(&["--route"]).is_err());
    }

    #[test]
    fn print_and_api_are_rejected_with_a_hint() {
        for bad in ["-p", "--print", "--api"] {
            let err = split(&["--no-route", "-c", bad]).unwrap_err();
            assert!(err.contains(bad) && err.contains("claudio"), "{err}");
        }
    }

    #[test]
    fn a_prompt_after_double_dash_is_not_inspected() {
        let (_, rest) = split(&["--", "-p"]).unwrap();
        assert_eq!(rest, args(&["--", "-p"]));
    }
}
