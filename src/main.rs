//! claudio — a session manager for the `claude` CLI and a drop-in wrapper
//! for it. This file is wiring: it builds the addon registry (`addons()`)
//! and routes argv to the core's words or an addon's; anything unclaimed
//! execs the real `claude` unchanged.

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use claudio_core::addon::{Addon, Addons};
use claudio_core::{app, claude, client, daemon, paths, proto, remote, upgrade};

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
    #[cfg(not(feature = "api"))]
    if let Some(code) = refuse_print_without_api(&argv[1..]) {
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
        Some("--plain") => claudio_tui::plain::args::run(&argv[2..], Arc::clone(addons)),
        // The session manager; launch flags route its new sessions.
        None => manager_cmd(&[], Arc::clone(addons)),
        Some(w) if addons.launch_flags().iter().any(|(_, d)| d.name == w) => {
            manager_cmd(&argv[1..], Arc::clone(addons))
        }
        _ => return None,
    };
    Some(code)
}

/// The addons this build registers (docs/addons.md), in registry order.
/// Each is a cargo feature (`api`, `web`, `vocdoni`, all default); without
/// any, the core alone. The API addon parses every remaining argv
/// (`cli::parse`), so it is last.
fn addons() -> Vec<Arc<dyn Addon>> {
    vec![
        #[cfg(feature = "vocdoni")]
        Arc::new(claudio_vocdoni::Vocdoni::new()),
        #[cfg(feature = "web")]
        Arc::new(claudio_web::WebAddon),
        #[cfg(feature = "api")]
        Arc::new(claudio_api::ApiAddon),
    ]
}

/// The addon registry this binary runs with; a refused registration exits.
fn registry() -> Arc<Addons> {
    let addons = Addons::new(addons()).unwrap_or_else(|e| {
        eprintln!("claudio: {e}");
        std::process::exit(1)
    });
    Arc::new(addons)
}

/// A build without the api addon: `-p`/`--print`/`--api` before a bare
/// `--` exit 2, because claudio never invokes `claude -p` (CLAUDE.md).
#[cfg(not(feature = "api"))]
fn refuse_print_without_api(args: &[String]) -> Option<ExitCode> {
    let print = args
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "-p" || a == "--print" || a == "--api");
    if !print {
        return None;
    }
    eprintln!("claudio: print mode and the API server need the api addon; this build has none");
    Some(ExitCode::from(2))
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
        Ok((select, [])) => claudio_tui::run(addons, select),
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
    use claudio_core::addon::test_support::TestAddon;

    fn argv(words: &[&str]) -> Vec<String> {
        std::iter::once("claudio")
            .chain(words.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn addons_all_registers() {
        assert!(Addons::new(addons()).is_ok());
    }

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
        assert_eq!(route(&argv(&["__probe"]), &addons), ExitCode::from(7));
        assert_eq!(route(&argv(&["mine", "x"]), &addons), ExitCode::from(9));
        // The core's words are never an addon's dispatch.
        assert_eq!(core_word(&argv(&["mine"]), &addons), None);
    }

    #[cfg(not(feature = "api"))]
    #[test]
    fn a_build_without_the_api_addon_refuses_print_mode() {
        let addons = registry();
        for flag in ["-p", "--print", "--api"] {
            let args = argv(&["--model", "m", flag, "hi"]);
            assert_eq!(route(&args, &addons), ExitCode::from(2), "{flag}");
        }
        // After a bare `--`, `-p` is a prompt for the real claude.
        let prompt = argv(&["--", "-p"]);
        assert_eq!(refuse_print_without_api(&prompt[1..]), None);
        assert_eq!(refuse_print_without_api(&argv(&["--resume"])[1..]), None);
    }

    #[cfg(not(feature = "web"))]
    #[tokio::test]
    async fn a_build_without_the_web_addon_asks_claude_for_no_lenses() {
        use claudio_core::addon::{LaunchOp, LaunchSelect};
        use claudio_core::app::addons::test_support::{fake_host, next_request, reply};
        use claudio_core::app::addons::{AddonRuntime, RuntimeOptions};
        use claudio_core::app::launch::{self, LaunchSpec, Step};
        use claudio_core::proto::{Msg, SessionId, SpawnSpec};

        let opts = RuntimeOptions {
            config_path: "/nonexistent/claudio/config.toml".into(),
            watch_config: false,
        };
        let (rt, _rx) = AddonRuntime::start(registry(), opts);
        let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
        let client = fake_host(&[], move |mut s| async move {
            while let Some((req, msg)) = next_request(&mut s).await {
                let Msg::Spawn(spec) = msg else {
                    reply(&mut s, req, Msg::Pong).await;
                    continue;
                };
                reply(
                    &mut s,
                    req,
                    Msg::Spawned {
                        id: spec.id,
                        pid: None,
                    },
                )
                .await;
                let _ = sent_tx.send(spec);
                return;
            }
        })
        .await;
        let sid = SessionId::from_u128(7);
        let spec = LaunchSpec {
            op: LaunchOp::New,
            sid,
            host: "local".into(),
            cwd: "/w".into(),
            select: LaunchSelect::of(None),
            frontend: claudio_core::addon::Frontend::Tui,
            base: Msg::Spawn(SpawnSpec {
                id: sid,
                cwd: "/w".into(),
                rows: 24,
                cols: 80,
                ..Default::default()
            }),
            core_lenses: Default::default(),
        };
        let step = launch::run(&rt, spec, client).await;
        assert!(matches!(step, Step::Sent { .. }), "{step:?}");
        let spawned = sent_rx.await.unwrap();
        let lenses = spawned.extensions.unwrap_or_default().lenses;
        let names: Vec<&str> = lenses.iter().map(|l| l.name.as_str()).collect();
        assert!(
            !names.contains(&"latex") && !names.contains(&"mermaid"),
            "{names:?}"
        );
        rt.shutdown();
    }
}
