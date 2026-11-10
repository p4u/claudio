//! Daemon tests for addon services ([`crate::daemon::service`]): a
//! `TestAddon` named `svc` whose service is `[svc] argv` of a config.toml in
//! the test's dir, run by a fake claudio (a shell script) that acts on its
//! first argument and records each start (`starts`) and its `$0` (`argv0`).

use serde_json::json;

use super::*;
use crate::addon::test_support::TestAddon;
use crate::addon::{Addon, ServiceState};
use crate::daemon::service::LOG_MAX;
use crate::daemon::ServiceTiming;

/// The fake claudio a service runs (`--version`: `write_executable`'s check).
const FAKE_SERVICE: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && exit 0
dir=$(dirname "$0")
echo "$$" >> "$dir/starts"
printf '%s\n' "$0" > "$dir/argv0"
case "$1" in
  serve) echo ready; exec cat > /dev/null ;;
  group) sleep 600 & echo $! > "$dir/grandchild"; echo ready; exec cat > /dev/null ;;
  exit) echo ready; sleep 0.2; exit "$2" ;;
  silent) exec sleep 600 ;;
  boom) echo boom >&2; exit 2 ;;
  orphan) echo ready; setsid sleep 2 & sleep 0.2; exit 1 ;;
  stubborn) trap '' TERM; echo ready; while :; do sleep 1; done ;;
  noisy) echo ready; head -c 3145728 /dev/zero | tr '\0' x >&2; : > "$dir/written"; exec cat > /dev/null ;;
  mode) if [ -e "$dir/ok" ]; then echo ready; exec cat > /dev/null; fi; echo 'not yet' >&2; exit 4 ;;
esac
exit 9
"#;

fn fast() -> ServiceTiming {
    ServiceTiming {
        ready: Duration::from_secs(5),
        stop: Duration::from_millis(500),
        backoff: Duration::from_millis(100),
        backoff_max: Duration::from_millis(400),
        stable: Duration::from_secs(60),
    }
}

/// `[svc]` with `argv` (none: an empty section) in `dir/config.toml`.
fn write_config(dir: &Path, argv: Option<&[&str]>) {
    let text = match argv {
        Some(argv) => format!("[svc]\nargv = {argv:?}\n"),
        None => "[svc]\n".to_owned(),
    };
    std::fs::write(dir.join("config.toml"), text).unwrap();
}

/// A daemon running the fake service with `argv`, under `timing`.
fn start_svc(argv: Option<&[&str]>, timing: ServiceTiming, old_peer: bool) -> TestDaemon {
    let dir = TestDaemon::new_dir();
    let script = dir.join("fake-claudio");
    write_executable(&script, FAKE_SERVICE);
    write_config(&dir, argv);
    let svc: Arc<dyn Addon> = Arc::new(TestAddon::new("svc").sections(&["svc"]).with_service());
    let config = Config {
        claudio: script,
        config_file: Some(dir.join("config.toml")),
        service_timing: timing,
        old_peer,
        ..TestDaemon::config(&dir)
    };
    TestDaemon::start_with_addons(dir, config, Addons::new(vec![svc]).unwrap())
}

fn state(d: &TestDaemon) -> ServiceState {
    d.daemon.services.state("svc")
}

async fn reload(d: &TestDaemon, force: bool) -> ServiceState {
    d.daemon.services.reload(&d.daemon, "svc", force).await
}

/// The first state `pick` accepts, within the read timeout.
async fn until(d: &TestDaemon, pick: impl Fn(&ServiceState) -> bool) -> ServiceState {
    within(async {
        loop {
            let s = state(d);
            if pick(&s) {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
}

fn pid_of(s: &ServiceState) -> Option<u32> {
    match s {
        ServiceState::Running { pid, .. } => Some(*pid),
        _ => None,
    }
}

async fn running(d: &TestDaemon) -> u32 {
    let s = until(d, |s| pid_of(s).is_some()).await;
    pid_of(&s).unwrap()
}

fn alive(pid: u32) -> bool {
    // SAFETY: kill(2) with signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

async fn gone(pid: u32) {
    within(async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
}

fn starts(d: &TestDaemon) -> usize {
    std::fs::read_to_string(d.dir.join("starts"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_service_starts_with_the_daemon_and_runs_its_binary() {
    let d = start_svc(Some(&["serve"]), fast(), false);
    let pid = running(&d).await;
    let ServiceState::Running { argv, since, .. } = state(&d) else {
        panic!("not running");
    };
    assert_eq!(argv, ["serve"]);
    assert!(since > 1_700_000_000, "{since}");
    let argv0 = std::fs::read_to_string(d.dir.join("argv0")).unwrap();
    assert_eq!(argv0.trim_end(), d.config.claudio.to_str().unwrap());
    assert!(alive(pid));
    // The same argv again: nothing happens.
    assert_eq!(pid_of(&reload(&d, false).await), Some(pid));
    assert_eq!(starts(&d), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_and_an_exit_0_both_restart_with_a_backoff() {
    let d = start_svc(Some(&["exit", "3"]), fast(), false);
    let failed = until(&d, |s| matches!(s, ServiceState::Failed { .. })).await;
    let ServiceState::Failed { exit, retry_in, .. } = failed else {
        unreachable!()
    };
    assert_eq!(exit, Some(3));
    assert!(retry_in.is_some_and(|r| r <= Duration::from_millis(400)));
    within(async {
        while starts(&d) < 3 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    write_config(&d.dir, Some(&["exit", "0"]));
    assert!(pid_of(&reload(&d, false).await).is_some());
    let before = starts(&d);
    let failed = until(&d, |s| {
        matches!(s, ServiceState::Failed { exit: Some(0), .. })
    })
    .await;
    assert!(matches!(
        failed,
        ServiceState::Failed {
            retry_in: Some(_),
            ..
        }
    ));
    within(async {
        while starts(&d) < before + 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reload_to_nothing_stops_it_for_good() {
    let d = start_svc(Some(&["serve"]), fast(), false);
    let pid = running(&d).await;
    write_config(&d.dir, None);
    assert_eq!(reload(&d, false).await, ServiceState::Off);
    gone(pid).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(state(&d), ServiceState::Off);
    assert_eq!(starts(&d), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_argv_or_force_restarts_and_a_failed_one_starts_at_once() {
    let timing = ServiceTiming {
        backoff: Duration::from_secs(30),
        backoff_max: Duration::from_secs(60),
        ..fast()
    };
    let d = start_svc(Some(&["serve"]), timing, false);
    let first = running(&d).await;
    write_config(&d.dir, Some(&["serve", "b"]));
    let second = pid_of(&reload(&d, false).await).expect("running");
    assert_ne!(second, first);
    gone(first).await;
    let third = pid_of(&reload(&d, true).await).expect("running");
    assert_ne!(third, second);
    gone(second).await;

    write_config(&d.dir, Some(&["mode"]));
    let failed = reload(&d, false).await;
    let ServiceState::Failed {
        exit,
        tail,
        retry_in,
        ..
    } = failed
    else {
        panic!("{failed:?}");
    };
    assert_eq!((exit, tail.as_str()), (Some(4), "not yet"));
    assert!(retry_in.is_some_and(|r| r > Duration::from_secs(20)));
    // The cause is gone; the same argv starts now, not after the backoff.
    std::fs::write(d.dir.join("ok"), "").unwrap();
    assert!(pid_of(&reload(&d, false).await).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reloads_queued_behind_one_run_as_one() {
    let d = start_svc(Some(&["serve"]), fast(), false);
    running(&d).await;
    let before = starts(&d);
    let (a, b, c) = tokio::join!(reload(&d, true), reload(&d, true), reload(&d, true));
    for s in [a, b, c] {
        assert!(pid_of(&s).is_some(), "{s:?}");
    }
    // The first restarts; the two queued behind it run as one.
    assert_eq!(starts(&d), before + 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_stops_the_whole_group() {
    let d = start_svc(Some(&["group"]), fast(), false);
    let pid = running(&d).await;
    let grandchild: u32 = std::fs::read_to_string(d.dir.join("grandchild"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(grandchild));
    let mut c = d.client().await;
    c.request(Msg::Shutdown).await;
    gone(pid).await;
    gone(grandchild).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_service_ends_on_stdin_eof_when_its_daemon_dies() {
    let d = start_svc(Some(&["serve"]), fast(), false);
    let pid = running(&d).await;
    // No signal: only the pipe closes, as when the daemon is SIGKILLed.
    d.daemon.services.abandon();
    gone(pid).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stubborn_service_is_killed_after_the_grace() {
    let d = start_svc(Some(&["stubborn"]), fast(), false);
    let pid = running(&d).await;
    write_config(&d.dir, None);
    let started = std::time::Instant::now();
    assert_eq!(reload(&d, false).await, ServiceState::Off);
    assert!(started.elapsed() >= fast().stop);
    gone(pid).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_ready_line_in_time_is_a_failure() {
    let timing = ServiceTiming {
        ready: Duration::from_millis(300),
        backoff: Duration::from_secs(30),
        ..fast()
    };
    let d = start_svc(Some(&["silent"]), timing, false);
    let failed = until(&d, |s| matches!(s, ServiceState::Failed { .. })).await;
    let ServiceState::Failed { tail, .. } = failed else {
        unreachable!()
    };
    assert_eq!(tail, "no ready line within 300ms");
    let pid: u32 = std::fs::read_to_string(d.dir.join("starts"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    gone(pid).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_start_reports_its_exit_and_stderr() {
    let d = start_svc(Some(&["boom"]), fast(), false);
    let failed = until(&d, |s| matches!(s, ServiceState::Failed { .. })).await;
    let ServiceState::Failed {
        exit,
        signal,
        tail,
        retry_in,
    } = failed
    else {
        unreachable!()
    };
    assert_eq!((exit, signal, tail.as_str()), (Some(2), None, "boom"));
    assert!(retry_in.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn its_log_never_passes_2_mib() {
    use std::os::unix::fs::PermissionsExt;
    let d = start_svc(Some(&["noisy"]), fast(), false);
    running(&d).await;
    let (log, old) = (d.dir.join("rt/svc.log"), d.dir.join("rt/svc.log.1"));
    let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
    within(async {
        // The child wrote 3 MiB; the copier has drained the pipe once the
        // two files hold more than 2 MiB minus the 1 MiB rotated away.
        while !d.dir.join("written").exists() || size(&log) + size(&old) < LOG_MAX {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(size(&log) <= LOG_MAX && size(&old) <= LOG_MAX);
    assert!(size(&log) + size(&old) <= 2 * LOG_MAX);
    assert!(size(&old) > 0, "rotated");
    let mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_sections_and_a_bad_config_fail_without_a_start() {
    let d = start_svc(None, fast(), false);
    std::fs::write(d.dir.join("config.toml"), "[svc]\nargv = 3\n").unwrap();
    let expected = ServiceState::Failed {
        exit: None,
        signal: None,
        tail: "[svc] argv: not a list of strings".to_owned(),
        retry_in: None,
    };
    assert_eq!(reload(&d, false).await, expected);
    std::fs::write(d.dir.join("config.toml"), "[svc\n").unwrap();
    let ServiceState::Failed { tail, retry_in, .. } = reload(&d, false).await else {
        panic!("not failed");
    };
    assert_eq!(
        (tail.as_str(), retry_in),
        ("config.toml does not parse", None)
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(starts(&d), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_playing_the_previous_release_runs_no_service() {
    let d = start_svc(Some(&["serve"]), fast(), true);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(state(&d), ServiceState::Off);
    assert_eq!(starts(&d), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_addon_reloads_and_reads_its_service_through_its_ctx() {
    let d = start_svc(None, fast(), false);
    let mut c = d.client().await;
    let op = |op: &str, body| Msg::AddonOp {
        addon: "svc".into(),
        op: op.into(),
        body,
    };
    let reply = |m: Msg| match m {
        Msg::AddonReply { body } => body.as_str().unwrap_or_default().to_owned(),
        other => panic!("{other:?}"),
    };
    assert_eq!(reply(c.call(op("state", json!({}))).await), "Off");
    write_config(&d.dir, Some(&["serve"]));
    let got = reply(c.call(op("reload", json!({"force": false}))).await);
    assert!(got.starts_with("Running"), "{got}");
    assert!(reply(c.call(op("state", json!({}))).await).starts_with("Running"));
}

/// An unasked exit is reported under the slot's turn: a reload that comes
/// while the report waits for the child's last stderr is not overwritten by
/// it. (`orphan` leaves a process outside its group holding stderr, which
/// opens the race window only where `setsid` exists; elsewhere the test
/// still checks that nothing replaces the reload's child.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reload_during_an_exit_report_is_not_overwritten() {
    let d = start_svc(Some(&["orphan"]), fast(), false);
    let pid = running(&d).await;
    gone(pid).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    write_config(&d.dir, Some(&["serve"]));
    let new = pid_of(&reload(&d, false).await).expect("running");
    let before = starts(&d);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(pid_of(&state(&d)), Some(new), "{:?}", state(&d));
    assert_eq!(starts(&d), before, "nothing replaced the reload's child");
}
