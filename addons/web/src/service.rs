//! `claudio web` as a daemon service (docs/addons.md, "The web addon";
//! docs/web.md, "The web service"): `[web]` in config.toml, the argv the
//! daemon runs from it (`WebAddon::service`), and the one reply of the
//! `apply` and `status` ops. Pure: the CLI (`cli.rs`) does the I/O.

use std::net::SocketAddr;

use claudio_core::addon::ServiceState;
use serde::Deserialize;
use serde_json::{json, Value};
use toml::Table;

use crate::{host_name_ok, listen_addr, parse_args, remote, WebArgs, DEFAULT_PORT};

/// What `[web]` stores as one unit: the listener and its allowed hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unit {
    /// `remote = "…"` (TLS, a password) when true, else `listen = "…"`.
    pub remote: bool,
    pub addr: SocketAddr,
    pub allow_hosts: Vec<String>,
}

impl Default for Unit {
    /// `--listen 127.0.0.1:7337`, no allowed host.
    fn default() -> Unit {
        Unit {
            remote: false,
            addr: SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)),
            allow_hosts: Vec::new(),
        }
    }
}

impl Unit {
    /// The unit the command line names.
    pub(crate) fn of(args: &WebArgs) -> Unit {
        Unit {
            remote: args.remote.is_some(),
            addr: args.remote.unwrap_or(args.listen),
            allow_hosts: args.allow_hosts.clone(),
        }
    }

    /// Its flags, each value in its flag's word (`--listen=…`): no value
    /// can be read as another flag.
    pub(crate) fn flags(&self) -> Vec<String> {
        let listener = match self.remote {
            true => format!("--remote={}", self.addr),
            false => format!("--listen={}", self.addr),
        };
        let hosts = self.allow_hosts.iter().map(|h| format!("--allow-host={h}"));
        std::iter::once(listener).chain(hosts).collect()
    }
}

/// `[web] enabled` (absent: false).
pub(crate) fn enabled(t: &Table) -> Result<bool, String> {
    match t.get("enabled") {
        None => Ok(false),
        Some(toml::Value::Boolean(b)) => Ok(*b),
        Some(_) => Err("[web] enabled: not true or false".to_owned()),
    }
}

/// The unit `[web]` stores (`None`: no `listen`, `remote` or
/// `allow_hosts`), checked as the flags are (`listen_addr`,
/// `remote_addr`, `host_name_ok`, `combination_ok`, the value types); an
/// error names the key.
pub(crate) fn unit(t: &Table) -> Result<Option<Unit>, String> {
    let text = |key: &str| match t.get(key) {
        None => Ok(None),
        Some(toml::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("[web] {key}: not a string")),
    };
    let named = |key: &'static str| move |e: String| format!("[web] {key}: {e}");
    let listen = text("listen")?
        .map(|s| listen_addr(&s).map_err(named("listen")))
        .transpose()?;
    let remote = text("remote")?
        .map(|s| remote::remote_addr(&s).map_err(named("remote")))
        .transpose()?;
    let allow_hosts: Vec<String> = match t.get("allow_hosts") {
        None => Vec::new(),
        Some(toml::Value::Array(hosts)) => hosts
            .iter()
            .map(|h| match h.as_str() {
                Some(h) if host_name_ok(h) => Ok(h.to_ascii_lowercase()),
                _ => Err(format!("[web] allow_hosts: {h} is not a host name")),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("[web] allow_hosts: not a list of host names".to_owned()),
    };
    let unit = match (listen, remote) {
        (Some(_), Some(_)) => return Err("[web] listen, remote: one or the other".to_owned()),
        (None, None) if allow_hosts.is_empty() => return Ok(None),
        (None, Some(addr)) => Unit {
            remote: true,
            addr,
            allow_hosts,
        },
        (listen, None) => Unit {
            addr: listen.unwrap_or(Unit::default().addr),
            allow_hosts,
            ..Unit::default()
        },
    };
    // The combination rules, as the flags have them (the one port with --remote).
    parse_args(&unit.flags()).map_err(|e| format!("[web] {}: {e}", unit_keys(&unit)))?;
    Ok(Some(unit))
}

fn unit_keys(unit: &Unit) -> &'static str {
    match unit.remote {
        true => "remote, allow_hosts",
        false => "listen, allow_hosts",
    }
}

/// `WebAddon::service`: `web --service <flags>` while `[web] enabled`. A
/// disabled section is not checked further: off is always possible.
pub(crate) fn argv(t: &Table) -> Result<Option<Vec<String>>, String> {
    if !enabled(t)? {
        return Ok(None);
    }
    let unit = unit(t)?.unwrap_or_default();
    let head = ["web", "--service"].map(String::from);
    Ok(Some(head.into_iter().chain(unit.flags()).collect()))
}

/// The unit `claudio web` serves: the command line's when it names any of
/// `--listen`, `--remote` or `--allow-host` (what it does not name takes
/// its default), else the stored one (`web`, the `[web]` table), else the
/// defaults. A stored unit is checked only when it is used.
pub(crate) fn merge(web: &Table, args: &WebArgs) -> Result<Unit, String> {
    if args.listen_given || args.remote.is_some() || !args.allow_hosts.is_empty() {
        return Ok(Unit::of(args));
    }
    Ok(unit(web)?.unwrap_or_default())
}

/// The command line `claudio web` acts on: `unit`'s flags (checked as
/// flags are) with `args`' one-shot flags. `--rotate-cert` needs a
/// `--remote` unit.
pub(crate) fn run_args(unit: &Unit, args: &WebArgs) -> Result<WebArgs, String> {
    if args.rotate_cert && !unit.remote {
        return Err("--rotate-cert needs --remote (here, or stored in [web])".to_owned());
    }
    let mut run = parse_args(&unit.flags())?;
    run.no_open = args.no_open;
    run.print_url = args.print_url;
    run.rotate_token = args.rotate_token;
    run.rotate_cert = args.rotate_cert;
    run.app = args.app.clone();
    Ok(run)
}

/// The one reply of `apply` and `status` (docs/addons.md, "The web addon").
pub(crate) fn reply(state: &ServiceState) -> Value {
    match state {
        ServiceState::Off => json!({"state": "off"}),
        ServiceState::Starting => json!({"state": "starting"}),
        ServiceState::Running { pid, since, argv } => {
            json!({"state": "running", "pid": pid, "since": since, "argv": argv})
        }
        ServiceState::Failed {
            exit,
            signal,
            tail,
            retry_in,
        } => json!({
            "state": "failed",
            "exit": exit,
            "signal": signal,
            "tail": tail,
            "retry_in": retry_in.map(|d| d.as_secs()),
        }),
    }
}

/// A reply as this claudio reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reported {
    Off,
    Starting,
    Running {
        pid: u32,
        since: u64,
        argv: Vec<String>,
    },
    Failed {
        exit: Option<i32>,
        signal: Option<i32>,
        tail: String,
        retry_in: Option<u64>,
    },
    /// A state (or shape) this claudio does not know: the daemon is newer.
    Newer,
}

/// The wire form of [`reply`]; unknown fields are ignored.
#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
enum Wire {
    Off,
    Starting,
    Running {
        pid: u32,
        since: u64,
        argv: Vec<String>,
    },
    Failed {
        exit: Option<i32>,
        signal: Option<i32>,
        #[serde(default)]
        tail: String,
        retry_in: Option<u64>,
    },
}

/// `body` of an `apply` or `status` reply.
pub(crate) fn read_reply(body: &Value) -> Reported {
    match Wire::deserialize(body) {
        Ok(Wire::Off) => Reported::Off,
        Ok(Wire::Starting) => Reported::Starting,
        Ok(Wire::Running { pid, since, argv }) => Reported::Running { pid, since, argv },
        Ok(Wire::Failed {
            exit,
            signal,
            tail,
            retry_in,
        }) => Reported::Failed {
            exit,
            signal,
            tail,
            retry_in,
        },
        Err(_) => Reported::Newer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn table(text: &str) -> Table {
        text.parse().unwrap()
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_enabled_section_is_the_service_argv() {
        let argv = |text: &str| argv(&table(text)).unwrap();
        assert_eq!(argv(""), None);
        assert_eq!(argv("enabled = false\nlisten = \"0.0.0.0:1\""), None);
        assert_eq!(
            argv("enabled = true"),
            Some(words(&["web", "--service", "--listen=127.0.0.1:7337"]))
        );
        assert_eq!(
            argv("enabled = true\nlisten = \"localhost:9000\"\nallow_hosts = [\"A.Example\"]"),
            Some(words(&[
                "web",
                "--service",
                "--listen=127.0.0.1:9000",
                "--allow-host=a.example"
            ]))
        );
        assert_eq!(
            argv("enabled = true\nremote = \"[::]:8443\"\nallow_hosts = [\"b.example:8443\"]"),
            Some(words(&[
                "web",
                "--service",
                "--remote=[::]:8443",
                "--allow-host=b.example:8443"
            ]))
        );
    }

    #[test]
    fn every_argv_parses_back_to_its_unit() {
        for text in [
            "enabled = true",
            "enabled = true\nlisten = \"127.0.0.2:80\"\nallow_hosts = [\"x.example\"]",
            "enabled = true\nremote = \":7999\"\nallow_hosts = [\"a.example:7999\", \"b.example\"]",
            "enabled = true\nremote = \"[::1]\"",
        ] {
            let unit = unit(&table(text)).unwrap().unwrap_or_default();
            let argv = argv(&table(text)).unwrap().unwrap();
            let back = parse_args(&argv[1..]).unwrap();
            assert!(back.service, "{text}");
            let addr = back.remote.unwrap_or(back.listen);
            assert_eq!((back.remote.is_some(), addr), (unit.remote, unit.addr));
            assert_eq!(back.allow_hosts, unit.allow_hosts, "{text}");
        }
    }

    #[test]
    fn a_bad_section_names_its_key() {
        let err = |text: &str| argv(&table(&format!("enabled = true\n{text}"))).unwrap_err();
        assert_eq!(
            argv(&table("enabled = \"yes\"")).unwrap_err(),
            "[web] enabled: not true or false"
        );
        let e = err("listen = \"0.0.0.0:7337\"");
        assert!(
            e.starts_with("[web] listen: refusing to listen on 0.0.0.0:7337"),
            "{e}"
        );
        assert_eq!(err("listen = 7337"), "[web] listen: not a string");
        assert!(err("remote = \"fe80::1\"").starts_with("[web] remote: "));
        let e = err("allow_hosts = [\"bad/host\"]");
        assert_eq!(e, "[web] allow_hosts: \"bad/host\" is not a host name");
        assert!(err("allow_hosts = \"a.example\"").starts_with("[web] allow_hosts: "));
        let e = err("listen = \"127.0.0.1:1\"\nremote = \":2\"");
        assert_eq!(e, "[web] listen, remote: one or the other");
        let e = err("remote = \":7999\"\nallow_hosts = [\"a.example:8443\"]");
        assert!(
            e.starts_with("[web] remote, allow_hosts: ") && e.contains("one port"),
            "{e}"
        );
    }

    fn flags(list: &[&str]) -> WebArgs {
        parse_args(&words(list)).unwrap()
    }

    #[test]
    fn any_listener_flag_replaces_the_stored_unit_and_none_keeps_it() {
        let stored = table("enabled = true\nremote = \":8443\"\nallow_hosts = [\"a.example\"]");
        let kept = merge(&stored, &flags(&["--no-open"])).unwrap();
        assert_eq!(kept, unit(&stored).unwrap().unwrap(), "none given: stored");
        let hosts = merge(&stored, &flags(&["--allow-host", "b.example"])).unwrap();
        assert_eq!(
            hosts,
            Unit {
                allow_hosts: words(&["b.example"]),
                ..Unit::default()
            },
            "--allow-host alone: the whole unit, the listener at its default"
        );
        let listen = merge(&stored, &flags(&["--listen", "127.0.0.1:9000"])).unwrap();
        assert_eq!(
            (
                listen.remote,
                listen.addr.to_string(),
                listen.allow_hosts.len()
            ),
            (false, "127.0.0.1:9000".to_owned(), 0),
            "hosts cleared too"
        );
        assert_eq!(merge(&table(""), &flags(&[])).unwrap(), Unit::default());
        let bad = table("listen = \"0.0.0.0:1\"");
        assert!(merge(&bad, &flags(&[]))
            .unwrap_err()
            .starts_with("[web] listen: "));
        assert!(
            merge(&bad, &flags(&["--listen", "127.0.0.1:1"])).is_ok(),
            "replaced"
        );
    }

    #[test]
    fn run_args_check_the_merged_unit() {
        let remote = unit(&table("remote = \":7999\"")).unwrap().unwrap();
        let asked = flags(&["--rotate-cert", "--no-open", "--rotate-token"]);
        let run = run_args(&remote, &asked).unwrap();
        assert!(run.rotate_cert && run.no_open && run.rotate_token && !run.print_url);
        assert_eq!(
            run.remote.map(|a| a.to_string()),
            Some("0.0.0.0:7999".into())
        );
        let err = run_args(&Unit::default(), &asked).unwrap_err();
        assert!(err.contains("--rotate-cert needs --remote"), "{err}");
        let app = flags(&["--app", "--listen", "127.0.0.1:9000"]);
        assert_eq!(
            Unit::of(&app).flags(),
            words(&["--listen=127.0.0.1:9000"]),
            "--app is never stored nor in the service's argv"
        );
        let run = run_args(&Unit::of(&app), &app).unwrap();
        assert_eq!(run.app, Some(crate::app_window::AppTarget::Local));
        let foreign = Unit {
            allow_hosts: words(&["a.example:8443"]),
            ..remote
        };
        assert!(run_args(&foreign, &flags(&[]))
            .unwrap_err()
            .contains("one port"));
    }

    #[test]
    fn replies_read_back_and_an_unknown_state_is_a_newer_daemon() {
        let running = ServiceState::Running {
            pid: 7,
            since: 9,
            argv: words(&["web", "--service", "--listen=127.0.0.1:1"]),
        };
        assert_eq!(
            read_reply(&reply(&running)),
            Reported::Running {
                pid: 7,
                since: 9,
                argv: words(&["web", "--service", "--listen=127.0.0.1:1"])
            }
        );
        let failed = ServiceState::Failed {
            exit: None,
            signal: Some(9),
            tail: "killed".into(),
            retry_in: None,
        };
        assert!(matches!(
            read_reply(&reply(&failed)),
            Reported::Failed {
                signal: Some(9),
                retry_in: None,
                ..
            }
        ));
        assert_eq!(read_reply(&reply(&ServiceState::Off)), Reported::Off);
        let extra = json!({"state": "off", "later": true});
        assert_eq!(read_reply(&extra), Reported::Off, "unknown fields ignored");
        assert_eq!(read_reply(&json!({"state": "paused"})), Reported::Newer);
        assert_eq!(read_reply(&json!("off")), Reported::Newer);
    }

    #[test]
    fn replies_carry_the_state_and_no_more() {
        let running = ServiceState::Running {
            pid: 42,
            since: 1_800_000_000,
            argv: words(&["web", "--service", "--listen=127.0.0.1:7337"]),
        };
        assert_eq!(
            reply(&running),
            json!({"state": "running", "pid": 42, "since": 1_800_000_000u64,
                   "argv": ["web", "--service", "--listen=127.0.0.1:7337"]})
        );
        let failed = ServiceState::Failed {
            exit: Some(1),
            signal: None,
            tail: "cannot listen".into(),
            retry_in: Some(Duration::from_millis(1500)),
        };
        assert_eq!(
            reply(&failed),
            json!({"state": "failed", "exit": 1, "signal": null, "tail": "cannot listen",
                   "retry_in": 1})
        );
        assert_eq!(reply(&ServiceState::Off), json!({"state": "off"}));
        assert_eq!(reply(&ServiceState::Starting), json!({"state": "starting"}));
    }
}
