//! Keeping claude up to date: turns "this claude is behind" into a prompt, an
//! automatic update or nothing, according to `[claude]` in config.toml.
//!
//! Two checks feed it. The local one compares this machine's claude with the
//! newest release (looked up at most once a day by the event loop); the remote
//! one compares a host's claude, as reported in its `Welcome`, with ours. A
//! host with no claude at all (this machine included) gets it installed as
//! `install` says, whatever the other hosts have. The update or install is
//! one daemon request, `Msg::UpdateClaude`, for every host.

use std::io;

use crate::claude::update::{decide, needs_update, short, Decision};
use crate::config::UpdatePolicy;
use crate::proto::Msg;

use super::app::{App, ReplyTo};
use super::confirm::{Choice, ConfirmAction, ConfirmPrompt, Queued, Skip};
use super::interaction::Modal;

impl App {
    /// Record this machine's `claude --version` (from the local `Welcome`).
    pub fn set_local_claude(&mut self, version: Option<String>) {
        self.local_claude = version;
    }

    /// The local `Welcome` is in: no claude here is installed as `install`
    /// says.
    pub fn check_local_install(&mut self) {
        if self.local_claude.is_none() {
            self.claude_missing("local");
        }
    }

    /// The newest release is `latest`: compare it with the local claude.
    pub fn check_local_claude(&mut self, latest: &str) {
        if let Some(have) = self.local_claude.clone() {
            let policy = self.claude_policy.update_check;
            self.claude_behind("local", &have, latest, policy);
        }
    }

    /// `host` just connected and reports `remote` (`None`: no claude). A
    /// missing claude is installed as `install` says; one older than ours
    /// is updated as `remote_check` says.
    pub fn check_remote_claude(&mut self, host: &str, remote: Option<&str>) {
        let Some(remote) = remote else {
            self.claude_missing(host);
            return;
        };
        if let Some(local) = self.local_claude.clone() {
            let policy = self.claude_policy.remote_check;
            self.claude_behind(host, remote, &local, policy);
        }
    }

    /// `host` has no claude: install it, ask first, or leave it, as
    /// `install` says. Each host is looked at once per run, as below.
    fn claude_missing(&mut self, host: &str) {
        let policy = self.claude_policy.install;
        if policy == UpdatePolicy::Off || !self.claude_checked.insert(host.to_owned()) {
            return;
        }
        match decide(policy, false) {
            Decision::Nothing => {}
            Decision::Run => self.start_claude_update(host, true),
            Decision::Ask => self.queue_confirm(install_prompt(host)),
        }
    }

    /// If `host`'s claude (`have`) is behind `want`, act on `policy`. Each
    /// host is looked at once per run, so reconnects do not nag.
    fn claude_behind(&mut self, host: &str, have: &str, want: &str, policy: UpdatePolicy) {
        if policy == UpdatePolicy::Off
            || !needs_update(Some(have), want)
            || !self.claude_checked.insert(host.to_owned())
        {
            return;
        }
        // `want` is a bare release ("2.1.296") from the channel, or a local
        // `--version` line ("2.1.296 (Claude Code)"); compare version numbers
        // only, so a skip holds whichever form it was recorded from.
        let skipped = self
            .claude_skipped
            .get(host)
            .is_some_and(|v| short(v) == short(want));
        match decide(policy, skipped) {
            Decision::Nothing => {}
            Decision::Run => self.start_claude_update(host, false),
            Decision::Ask => self.queue_confirm(claude_prompt(host, have, want)),
        }
    }

    /// Ask `host`'s daemon to update (or install) claude.
    pub(super) fn start_claude_update(&mut self, host: &str, install: bool) {
        let what = if install { "installing" } else { "updating" };
        self.notify(format!("{what} claude on {host}…"));
        if install {
            self.claude_installing.insert(host.to_owned());
        }
        self.request(
            host,
            Msg::UpdateClaude { install },
            ReplyTo::ClaudeUpdate(host.to_owned()),
        );
    }

    /// Whether claude is being installed on `host`, or a prompt open or
    /// waiting asks to install it there.
    pub(super) fn claude_install_pending(&self, host: &str) -> bool {
        let install = ConfirmAction::UpdateClaude {
            host: host.to_owned(),
            install: true,
        };
        let asks = |p: &ConfirmPrompt| {
            p.choices
                .iter()
                .any(|c| c.action.as_ref() == Some(&install))
        };
        self.claude_installing.contains(host)
            || matches!(&self.modal, Some(Modal::Confirm(p)) if asks(p))
            || self
                .confirms
                .iter()
                .any(|q| matches!(q, Queued::Confirm(p) if asks(p)))
    }

    /// The outcome of [`App::start_claude_update`].
    pub(super) fn on_claude_updated(&mut self, host: &str, reply: io::Result<Msg>) {
        let install = self.claude_installing.remove(host);
        let what = if install { "install" } else { "update" };
        let failure = match reply {
            Ok(Msg::ClaudeUpdated {
                version: Some(version),
                ok: true,
                ..
            }) => {
                let done = if install {
                    format!("claude {} installed", short(&version))
                } else {
                    format!("claude updated to {}", short(&version))
                };
                self.notify(format!("{done} on {host}; new sessions use it"));
                if host == "local" {
                    self.local_claude = Some(version);
                }
                if install {
                    self.reapply_host(host);
                }
                return;
            }
            Ok(Msg::ClaudeUpdated { tail, .. }) => {
                let line = tail
                    .lines()
                    .map(str::trim)
                    .rfind(|l| !l.is_empty())
                    .unwrap_or("no output");
                format!("claude {what} failed on {host}: {line}")
            }
            Ok(_) => return,
            Err(e) => App::older_daemon_notice(host, &e)
                .unwrap_or_else(|| format!("claude {what} failed on {host}: {e}")),
        };
        // `--plain` has no session without claude: leave, saying why.
        if install && host == "local" && self.plain_pending.is_some() {
            self.plain_failed(failure);
        } else {
            self.notify(failure);
        }
    }
}

/// The question for a claude on `host` that is `have` while `want` is
/// current.
fn claude_prompt(host: &str, have: &str, want: &str) -> ConfirmPrompt {
    let want_short = short(want);
    let text = if host == "local" {
        format!(
            "claude {} is behind the latest release ({want_short}).\nRun `claude update`?",
            short(have)
        )
    } else {
        format!(
            "claude on {host} is {}, older than the {want_short} on this machine.\nRun `claude update` there?",
            short(have)
        )
    };
    let update = ConfirmAction::UpdateClaude {
        host: host.to_owned(),
        install: false,
    };
    let skip = ConfirmAction::SkipClaude(Skip {
        host: host.to_owned(),
        version: want_short.to_owned(),
    });
    ConfirmPrompt::new(
        "Claude is out of date",
        text,
        vec![
            Choice::new('y', "update", Some(update)),
            Choice::new('n', "not now", None),
            Choice::new('s', "skip this version", Some(skip)),
        ],
    )
}

/// The question for `host`, which has no claude.
fn install_prompt(host: &str) -> ConfirmPrompt {
    let place = if host == "local" {
        "this machine"
    } else {
        host
    };
    let text = format!(
        "claude is not installed on {place}.\nInstall it with the official installer\n(curl -fsSL https://claude.ai/install.sh | bash)?"
    );
    let install = ConfirmAction::UpdateClaude {
        host: host.to_owned(),
        install: true,
    };
    ConfirmPrompt::new(
        "Claude is not installed",
        text,
        vec![
            Choice::new('y', "install", Some(install)),
            Choice::new('n', "not now", None),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Effect;
    use crate::tui::app::AppConfig;
    use crate::tui::test_support;
    use crossterm::event::KeyCode;

    const LOCAL: &str = "2.1.296 (Claude Code)";

    fn new_app(policy: UpdatePolicy) -> App {
        let mut config = test_support::config();
        config.claude.update_check = policy;
        config.claude.remote_check = policy;
        config.claude.install = policy;
        App::new(AppConfig {
            local_claude: Some(LOCAL.into()),
            ..config
        })
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_terminal(test_support::press(code));
    }

    /// Let a freshly opened prompt start listening to the keyboard.
    fn arm(app: &mut App) {
        for _ in 0..4 {
            app.on_tick();
        }
    }

    fn update_requests(app: &mut App) -> Vec<(String, Msg)> {
        app.take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::Request {
                    host,
                    msg: msg @ Msg::UpdateClaude { .. },
                    to: ReplyTo::ClaudeUpdate(h),
                } => {
                    assert_eq!(host, h);
                    Some((host, msg))
                }
                _ => None,
            })
            .collect()
    }

    fn prompt(app: &App) -> &ConfirmPrompt {
        match &app.modal {
            Some(Modal::Confirm(p)) => p,
            _ => panic!("expected a confirm prompt"),
        }
    }

    fn has_prompt(app: &App) -> bool {
        matches!(app.modal, Some(Modal::Confirm(_)))
    }

    fn notice(app: &App) -> &str {
        &app.notice.as_ref().expect("a notice").text
    }

    #[test]
    fn older_remote_claude_prompts_and_y_requests_the_update() {
        let mut app = new_app(UpdatePolicy::Ask);
        app.check_remote_claude("devbox", Some("2.1.280 (Claude Code)"));
        let p = prompt(&app);
        assert!(
            p.text.contains("devbox") && p.text.contains("2.1.280") && p.text.contains("2.1.296")
        );
        assert_eq!(p.choices[0].label, "update");
        assert!(
            update_requests(&mut app).is_empty(),
            "nothing before the answer"
        );

        // A keystroke meant for the session does not answer a new prompt.
        press(&mut app, KeyCode::Char('y'));
        assert!(has_prompt(&app));
        arm(&mut app);
        press(&mut app, KeyCode::Char('y'));
        assert!(app.modal.is_none());
        assert_eq!(
            update_requests(&mut app),
            [("devbox".to_owned(), Msg::UpdateClaude { install: false })]
        );
        assert!(notice(&app).contains("updating claude on devbox"));
    }

    #[test]
    fn missing_remote_claude_offers_the_installer() {
        let mut app = new_app(UpdatePolicy::Ask);
        app.check_remote_claude("devbox", None);
        assert_eq!(prompt(&app).title, "Claude is not installed");
        assert_eq!(prompt(&app).choices.len(), 2, "no version to skip");
        assert_eq!(prompt(&app).choices[0].label, "install");
        assert!(prompt(&app).text.contains("not installed on devbox"));
        assert!(prompt(&app).text.contains("claude.ai/install.sh"));
        arm(&mut app);
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(
            update_requests(&mut app),
            [("devbox".to_owned(), Msg::UpdateClaude { install: true })]
        );
    }

    #[test]
    fn n_and_esc_decline_without_remembering() {
        for decline in [KeyCode::Char('n'), KeyCode::Esc] {
            let mut app = new_app(UpdatePolicy::Ask);
            app.check_remote_claude("devbox", Some("2.1.280"));
            arm(&mut app);
            press(&mut app, decline);
            assert!(app.modal.is_none());
            assert!(update_requests(&mut app).is_empty());
            assert!(app.claude_skipped.is_empty());
        }
    }

    #[test]
    fn skip_is_remembered_per_host_and_version() {
        let mut first = new_app(UpdatePolicy::Ask);
        first.check_remote_claude("devbox", Some("2.1.280"));
        arm(&mut first);
        press(&mut first, KeyCode::Char('s'));
        assert!(first.modal.is_none());
        assert!(update_requests(&mut first).is_empty());
        let state = first.to_state();
        assert_eq!(
            state.claude_skipped.get("devbox").map(String::as_str),
            Some("2.1.296"),
            "stored as the bare version"
        );

        // A later run with that state stays quiet for this version...
        let mut later = new_app(UpdatePolicy::Ask);
        later.claude_skipped = state.claude_skipped;
        later.check_remote_claude("devbox", Some("2.1.280"));
        assert!(later.modal.is_none());
        // ...but not for another host...
        later.check_remote_claude("other", Some("2.1.280"));
        assert!(has_prompt(&later));
        // ...nor once the target is a version that was not skipped.
        let mut newer = new_app(UpdatePolicy::Ask);
        newer
            .claude_skipped
            .insert("devbox".into(), "2.1.200".into());
        newer.check_remote_claude("devbox", Some("2.1.280"));
        assert!(has_prompt(&newer));
    }

    #[test]
    fn a_skip_matches_whichever_form_the_version_came_in() {
        // Skipped from the local check, where `want` is the channel's bare
        // version; the remote check's `want` carries the "(Claude Code)" tail.
        let mut app = new_app(UpdatePolicy::Ask);
        app.claude_skipped.insert("devbox".into(), "2.1.296".into());
        app.check_remote_claude("devbox", Some("2.1.280"));
        assert!(app.modal.is_none());

        // A state.json written before versions were normalised, read while
        // the local version is known in its bare form.
        let mut app = new_app(UpdatePolicy::Ask);
        app.set_local_claude(Some("2.1.296".into()));
        app.claude_skipped.insert("devbox".into(), LOCAL.into());
        app.check_remote_claude("devbox", Some("2.1.280"));
        assert!(app.modal.is_none());
    }

    #[test]
    fn auto_updates_and_installs_without_asking() {
        let mut app = new_app(UpdatePolicy::Auto);
        app.check_remote_claude("devbox", Some("2.1.280"));
        assert!(app.modal.is_none());
        assert_eq!(
            update_requests(&mut app),
            [("devbox".to_owned(), Msg::UpdateClaude { install: false })]
        );
        assert!(notice(&app).contains("updating claude on devbox"));

        app.check_remote_claude("bare", None);
        assert!(app.modal.is_none());
        assert_eq!(
            update_requests(&mut app),
            [("bare".to_owned(), Msg::UpdateClaude { install: true })]
        );
        assert_eq!(notice(&app), "installing claude on bare…");
    }

    #[test]
    fn a_missing_claude_is_installed_with_none_here_and_on_this_machine() {
        let mut app = App::new(test_support::config());
        assert_eq!(app.claude_policy.install, UpdatePolicy::Auto, "the default");
        assert_eq!(app.local_claude, None);
        app.check_local_install();
        assert_eq!(notice(&app), "installing claude on local…");
        app.check_remote_claude("devbox", None);
        assert_eq!(
            update_requests(&mut app),
            [
                ("local".to_owned(), Msg::UpdateClaude { install: true }),
                ("devbox".to_owned(), Msg::UpdateClaude { install: true }),
            ]
        );
        assert!(app.claude_install_pending("local") && app.claude_install_pending("devbox"));
        // Nothing here to compare an installed one with.
        app.check_remote_claude("old", Some("1.0.0"));
        assert!(app.modal.is_none() && update_requests(&mut app).is_empty());

        // This machine has claude: nothing to install.
        let mut app = new_app(UpdatePolicy::Auto);
        app.check_local_install();
        assert!(update_requests(&mut app).is_empty());
    }

    #[test]
    fn install_has_its_own_policy() {
        // Installs asked about with every version check off...
        let mut app = new_app(UpdatePolicy::Off);
        app.claude_policy.install = UpdatePolicy::Ask;
        app.check_remote_claude("devbox", Some("2.1.280"));
        assert!(app.modal.is_none());
        app.check_remote_claude("bare", None);
        assert_eq!(prompt(&app).choices[0].label, "install");
        assert!(app.claude_install_pending("bare"), "the question waits");
        arm(&mut app);
        press(&mut app, KeyCode::Char('n'));
        assert!(!app.claude_install_pending("bare"));
        assert!(update_requests(&mut app).is_empty());

        // ...and none with them on.
        let mut app = new_app(UpdatePolicy::Ask);
        app.claude_policy.install = UpdatePolicy::Off;
        app.local_claude = None;
        app.check_local_install();
        app.check_remote_claude("bare", None);
        assert!(app.modal.is_none() && app.take_effects().is_empty());
    }

    #[test]
    fn off_and_up_to_date_do_nothing() {
        let mut off = new_app(UpdatePolicy::Off);
        off.check_remote_claude("devbox", Some("2.1.280"));
        off.check_remote_claude("bare", None);
        off.check_local_claude("2.2.0");
        assert!(off.modal.is_none() && off.take_effects().is_empty());

        let mut ask = new_app(UpdatePolicy::Ask);
        ask.check_remote_claude("same", Some(LOCAL));
        ask.check_remote_claude("newer", Some("3.0.0"));
        ask.check_local_claude("2.1.296");
        assert!(ask.modal.is_none() && ask.take_effects().is_empty());
    }

    #[test]
    fn each_host_is_checked_once_per_run() {
        let mut app = new_app(UpdatePolicy::Ask);
        app.check_remote_claude("devbox", Some("2.1.280"));
        arm(&mut app);
        press(&mut app, KeyCode::Char('n'));
        app.check_remote_claude("devbox", Some("2.1.280")); // reconnect
        assert!(app.modal.is_none());

        let mut app = App::new(test_support::config());
        app.check_local_install();
        app.check_remote_claude("bare", None);
        app.on_reply(
            ReplyTo::ClaudeUpdate("bare".into()),
            Err(io::Error::other("connection lost")),
        );
        app.check_remote_claude("bare", None); // reconnect
        app.check_local_install(); // the local daemon restarted
        assert_eq!(update_requests(&mut app).len(), 2);
    }

    #[test]
    fn local_claude_is_compared_with_the_release_channel() {
        let mut app = new_app(UpdatePolicy::Ask);
        app.check_local_claude("2.1.300");
        let p = prompt(&app);
        assert!(p.text.contains("latest release (2.1.300)"));
        assert_eq!(
            p.choices[0].action,
            Some(ConfirmAction::UpdateClaude {
                host: "local".into(),
                install: false
            })
        );
        // The remote policy is separate.
        let mut split = new_app(UpdatePolicy::Ask);
        split.claude_policy.update_check = UpdatePolicy::Off;
        split.check_local_claude("2.1.300");
        assert!(split.modal.is_none());
        split.check_remote_claude("devbox", Some("2.1.280"));
        assert!(has_prompt(&split));
    }

    #[test]
    fn a_prompt_waits_for_the_open_modal() {
        let mut app = new_app(UpdatePolicy::Ask);
        app.modal = Some(Modal::Help);
        app.check_remote_claude("devbox", Some("2.1.280"));
        assert!(matches!(app.modal, Some(Modal::Help)));
        app.modal = None;
        app.on_tick();
        assert!(has_prompt(&app));
    }

    #[test]
    fn outcomes_become_notices() {
        let mut app = new_app(UpdatePolicy::Ask);
        let ok = Msg::ClaudeUpdated {
            version: Some("2.1.296 (Claude Code)".into()),
            ok: true,
            tail: String::new(),
        };
        app.on_reply(ReplyTo::ClaudeUpdate("devbox".into()), Ok(ok));
        assert_eq!(
            notice(&app),
            "claude updated to 2.1.296 on devbox; new sessions use it"
        );

        let failed = Msg::ClaudeUpdated {
            version: Some("2.1.280".into()),
            ok: false,
            tail: "downloading\nEACCES: permission denied\n  \n".into(),
        };
        app.on_reply(ReplyTo::ClaudeUpdate("devbox".into()), Ok(failed));
        assert_eq!(
            notice(&app),
            "claude update failed on devbox: EACCES: permission denied"
        );

        let old = io::Error::other("unsupported op: unknown");
        app.on_reply(ReplyTo::ClaudeUpdate("devbox".into()), Err(old));
        assert_eq!(
            notice(&app),
            "daemon on devbox is older: run `claudio daemon restart`"
        );

        let local = Msg::ClaudeUpdated {
            version: Some("2.2.0".into()),
            ok: true,
            tail: String::new(),
        };
        app.on_reply(ReplyTo::ClaudeUpdate("local".into()), Ok(local));
        assert_eq!(app.local_claude.as_deref(), Some("2.2.0"));
    }

    #[test]
    fn install_outcomes_say_installed() {
        let mut app = App::new(test_support::config());
        app.check_local_install();
        let ok = Msg::ClaudeUpdated {
            version: Some("2.1.296 (Claude Code)".into()),
            ok: true,
            tail: "Installing\nclaude installed\n".into(),
        };
        app.on_reply(ReplyTo::ClaudeUpdate("local".into()), Ok(ok));
        assert_eq!(
            notice(&app),
            "claude 2.1.296 installed on local; new sessions use it"
        );
        assert_eq!(app.local_claude.as_deref(), Some("2.1.296 (Claude Code)"));
        assert!(!app.claude_install_pending("local"));

        app.check_remote_claude("devbox", None);
        let failed = Msg::ClaudeUpdated {
            version: None,
            ok: false,
            tail: "claude installer needs curl".into(),
        };
        app.on_reply(ReplyTo::ClaudeUpdate("devbox".into()), Ok(failed));
        assert_eq!(
            notice(&app),
            "claude install failed on devbox: claude installer needs curl"
        );
        assert!(!app.quit, "only --plain leaves");
    }
}
