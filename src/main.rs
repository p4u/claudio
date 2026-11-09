//! claudio — a session manager for the `claude` CLI and a drop-in wrapper
//! for it. This file is wiring: it builds the addon registry
//! (`addons::all()`) and routes argv to the core's words or an addon's;
//! anything unclaimed execs the real `claude` unchanged.

// The core library. The TUI and the addons, still modules of this package
// until P4.3-P4.6, reach it through these names; P4.7 removes this line.
// Allowed unused: a build without some addons uses fewer of them.
#[allow(unused_imports)]
use claudio_core::{
    addon, app, claude, client, config, daemon, freshness, paths, proto, release, remote, term,
    text, upgrade, version,
};

mod addons;

use claudio_tui as tui;
use claudio_tui::plain::args as plain;

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use addon::Addons;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    route(&argv, &registry())
}

/// argv to its owner: the addons' relays, the core's words, the addons'
/// words (registry order), `--help`, else the real claude unchanged.
fn route(argv: &[String], addons: &Arc<Addons>) -> ExitCode {
    if let Some(code) = addons.relay(argv) {
        return code;
    }
    if let Some(code) = core_word(argv, addons) {
        return code;
    }
    if let Some(code) = addons.dispatch(argv) {
        return code;
    }
    let args = &argv[1..];
    if wants_help(args) {
        return print_help(args, addons);
    }
    exec_claude_transparently(args)
}

/// The core's own words, in the spec's order; `None`: not one of them.
fn core_word(argv: &[String], addons: &Arc<Addons>) -> Option<ExitCode> {
    // The test harness's diag words (`__bootstrap`, …), a disjoint set.
    #[cfg(feature = "diag")]
    if let Some(code) = remote::diag::dispatch(argv) {
        return Some(code);
    }
    let code = match argv.get(1).map(String::as_str) {
        // One JSON line describing this binary; never passthrough.
        Some("__probe") => remote::probe::run(),
        // The remote end of an ssh transport.
        Some("--slave") => remote::bridge::run(),
        // The manager's hook relay to the daemon socket.
        Some("__hook") if argv.len() == 5 => {
            claude::hooks::relay(&argv[2], std::path::Path::new(&argv[3]), &argv[4])
        }
        // SessionStart: print the session's extension instructions.
        Some("__context") => claude::hooks::context(
            argv.get(2).map(String::as_str),
            argv.get(3).map(String::as_str),
        ),
        Some("--daemon") => daemon::run(Arc::clone(addons)),
        Some("upgrade") => upgrade::upgrade_cmd(argv.get(2).map(String::as_str) == Some("--check")),
        Some("daemon") => daemon::ctl::daemon_cmd(&argv[2..]),
        Some("sessions") => sessions_cmd(),
        // One plain claude with a manager session's launch, no daemon or TUI.
        Some("--plain") => plain::run(&argv[2..], Arc::clone(addons)),
        // The session manager; launch flags route its new sessions.
        None => manager_cmd(&[], Arc::clone(addons)),
        Some(w) if addons.launch_flags().iter().any(|(_, d)| d.name == w) => {
            manager_cmd(&argv[1..], Arc::clone(addons))
        }
        _ => return None,
    };
    Some(code)
}

/// The addon registry this binary runs with; a refused registration exits.
fn registry() -> Arc<Addons> {
    let addons = Addons::new(addons::all()).unwrap_or_else(|e| {
        eprintln!("claudio: {e}");
        std::process::exit(1)
    });
    Arc::new(addons)
}

/// Whether argv is asking for help (`-h`/`--help` anywhere on the line).
fn wants_help(args: &[String]) -> bool {
    args.iter().any(|a| a == "-h" || a == "--help")
}

/// Show the real `claude` help, then the addons' appendices.
/// Falls back to printing just those if `claude` can't be run.
fn print_help(args: &[String], addons: &Addons) -> ExitCode {
    let claude = std::env::var("CLAUDIO_CLAUDE_PATH").unwrap_or_else(|_| "claude".into());
    let appendix = addons.help();
    match std::process::Command::new(&claude).args(args).output() {
        Ok(o) => {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(&o.stdout);
            let _ = std::io::stderr().write_all(&o.stderr);
            let _ = stdout.write_all(appendix.as_bytes());
            ExitCode::from(o.status.code().unwrap_or(0) as u8)
        }
        Err(e) => {
            eprintln!("claudio: could not run '{claude} --help': {e}");
            print!("{appendix}");
            ExitCode::SUCCESS
        }
    }
}

// ── `claudio sessions` ────────────────────────────────────────────────────────

/// `claudio [<launch flags>]`: the session manager. The flags take no
/// claude arguments after them.
fn manager_cmd(args: &[String], addons: Arc<Addons>) -> ExitCode {
    match app::launch::split_flags(&addons, args) {
        Ok((select, [])) => tui::run(addons, select),
        Ok(_) => {
            let names: Vec<&str> = addons.launch_flags().iter().map(|(_, d)| d.name).collect();
            let names = match names.split_last() {
                Some((last, [])) => last.to_string(),
                Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
                None => String::new(),
            };
            eprintln!(
                "claudio: {names} open the session manager and take no claude arguments; \
                 use `claudio --plain …` for one claude"
            );
            ExitCode::FAILURE
        }
        Err(msg) => {
            eprintln!("claudio: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn sessions_cmd() -> ExitCode {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let rt = match rt {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("claudio sessions: {e}");
            return ExitCode::FAILURE;
        }
    };
    rt.block_on(async {
        match client::connect(&paths::daemon_socket()).await {
            Ok(c) => match c.request(proto::Msg::ListSessions).await {
                Ok(proto::Msg::Sessions { sessions }) => {
                    if sessions.is_empty() {
                        println!("no sessions");
                        return;
                    }
                    println!("{:<10} {:<14} {:<20} CWD", "ID", "STATE", "NAME/TITLE");
                    for s in &sessions {
                        let id_prefix: String = s.id.to_string().chars().take(8).collect();
                        let state = format!("{:?}", s.state).to_ascii_lowercase();
                        let name = s.name.as_deref().or(s.title.as_deref()).unwrap_or("-");
                        println!("{:<10} {:<14} {:<20} {}", id_prefix, state, name, s.cwd);
                    }
                }
                _ => eprintln!("claudio sessions: unexpected reply"),
            },
            Err(e) => eprintln!("claudio sessions: {e}"),
        }
    });
    ExitCode::SUCCESS
}

/// Replace this process with the real `claude`, forwarding argv verbatim.
fn exec_claude_transparently(args: &[String]) -> ExitCode {
    let mut cmd = std::process::Command::new(claude::binary());
    cmd.args(args);
    claude::exec(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon::test_support::TestAddon;

    #[test]
    fn main_routes_relays_before_core_words() {
        // An addon relay that claims a core word wins over it; an addon word
        // the core does not own is dispatched.
        let addons = Arc::new(
            Addons::new(vec![Arc::new(
                TestAddon::new("echo")
                    .relay("__probe", 7)
                    .command("mine", 9),
            )])
            .unwrap(),
        );
        let argv = |w: &[&str]| -> Vec<String> {
            std::iter::once("claudio")
                .chain(w.iter().copied())
                .map(String::from)
                .collect()
        };
        assert_eq!(route(&argv(&["__probe"]), &addons), ExitCode::from(7));
        assert_eq!(route(&argv(&["mine", "x"]), &addons), ExitCode::from(9));
        // The core's words are never an addon's dispatch.
        assert_eq!(core_word(&argv(&["mine"]), &addons), None);
    }
}
