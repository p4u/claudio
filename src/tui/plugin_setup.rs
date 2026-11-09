//! The plugin setup screen: every item of a proxy plugin's manifest, by
//! section, each with a checkbox; the member chooses what to apply.
//!
//! It opens on a manifest's first sight (no `[plugins.<name>]` table in
//! config.toml yet) and from the extensions screen (Enter on the plugin's
//! row). Keys: Up/Down (or j/k) move, Space toggles, `a` turns every item
//! on, `n` every item off, Enter saves and closes (the choices go to
//! config.toml and every host is asked again), Esc closes without saving.
//! Esc on first sight defers: nothing is written or sent for the manifest,
//! the row says `setup pending (Enter)`, and the screen opens again at the
//! next claudio start. The screen swallows every key, and it waits its turn
//! in the confirm queue like a prompt: one at a time, in arrival order. A
//! first-sight screen ignores keys for a moment after appearing, like a
//! prompt, so a keystroke meant for a session cannot answer it.
//!
//! Pure, like `confirm.rs`: it reads the Vocdoni addon's plugin state
//! through its bridge, and saving or deferring is an [`Effect::Bridge`];
//! drawn by `ui/plugin_setup.rs`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::addons::vocdoni::plugins::choices::choices_for;
use crate::addons::vocdoni::plugins::manifest::{Manifest, Section};

use std::collections::BTreeSet;

use super::app::{AddonResult, App, BridgeCall, Effect, Mode};
use super::confirm::Queued;
use super::extensions_view::ExtensionsView;
use super::interaction::Modal;
use crate::addons::vocdoni::bridge::{Bridge, Pending};
use crate::addons::vocdoni::plugins::state::{ManifestState, PluginPart};
use crate::addons::vocdoni::proxy::plugins::PluginList;
use crate::app::extensions::Source;

/// Ticks (250 ms each) during which a first-sight screen ignores keys.
const ARM_TICKS: u8 = 4;

/// One item of the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct SetupItem {
    pub id: String,
    pub section: Section,
    pub title: String,
    pub on: bool,
}

/// The setup screen of one plugin's manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginSetup {
    pub name: String,
    pub version: String,
    pub items: Vec<SetupItem>,
    pub selected: usize,
    /// Opened on first sight: Esc defers instead of just closing.
    pub first_sight: bool,
    /// Remaining ticks before keys count.
    pub(super) armed: u8,
    /// The extensions screen it was opened from, shown again on close.
    pub back: Option<ExtensionsView>,
}

/// What a key did on the screen.
#[derive(Debug, PartialEq)]
enum Outcome {
    Stay,
    Save,
    Close,
}

impl PluginSetup {
    /// The screen for `m`, with the ids of `disabled` off.
    pub fn new(m: &Manifest, disabled: &[String], first_sight: bool) -> PluginSetup {
        let items = m
            .items()
            .into_iter()
            .map(|i| SetupItem {
                on: !disabled.contains(&i.id),
                id: i.id,
                section: i.section,
                title: i.title,
            })
            .collect();
        PluginSetup {
            name: m.name.clone(),
            version: m.version.clone(),
            items,
            selected: 0,
            first_sight,
            armed: if first_sight { ARM_TICKS } else { 0 },
            back: None,
        }
    }

    /// Show `m` (a newer fetch) instead, keeping the toggle of every id
    /// still there; a new id is on.
    pub fn replace(&mut self, m: &Manifest) {
        let off: Vec<String> = self.off_ids();
        let mut next = PluginSetup::new(m, &off, self.first_sight);
        next.armed = self.armed;
        next.back = self.back.take();
        next.selected = self.selected.min(next.items.len().saturating_sub(1));
        *self = next;
    }

    /// Whether `m` is not the manifest the screen shows (another version
    /// or other items).
    fn differs(&self, m: &Manifest) -> bool {
        let items = m.items();
        let ids = items.iter().map(|i| &i.id);
        self.version != m.version || !self.items.iter().map(|i| &i.id).eq(ids)
    }

    /// The ids turned off.
    pub fn off_ids(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|i| !i.on)
            .map(|i| i.id.clone())
            .collect()
    }

    fn on_key(&mut self, key: &KeyEvent) -> Outcome {
        // Ctrl or Alt chords are not answers; Shift only makes a capital.
        if self.armed > 0 || !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
            return Outcome::Stay;
        }
        let last = self.items.len().saturating_sub(1);
        match key.code {
            KeyCode::Enter => return Outcome::Save,
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::Char(' ') => {
                if let Some(item) = self.items.get_mut(self.selected) {
                    item.on = !item.on;
                }
            }
            KeyCode::Char('a') => self.items.iter_mut().for_each(|i| i.on = true),
            KeyCode::Char('n') => self.items.iter_mut().for_each(|i| i.on = false),
            _ => {}
        }
        Outcome::Stay
    }

    /// The key hint: Esc defers on first sight, cancels otherwise.
    pub fn hint(&self) -> &'static str {
        if self.first_sight {
            "Space toggle · a all on · n all off · Enter save · Esc later"
        } else {
            "Space toggle · a all on · n all off · Enter save · Esc cancel"
        }
    }
}

impl App {
    /// The Vocdoni addon's plugin state, when this build has the addon.
    pub(super) fn bridge(&self) -> Option<Bridge> {
        Bridge::of(&self.addons)
    }

    /// What the addon runtime says: an addon's data changed.
    pub fn on_addon(&mut self, result: AddonResult) {
        match result {
            AddonResult::Changed { .. } => self.on_plugins_changed(),
        }
    }

    /// The plugin state changed: the registry's plugin rows, the setup
    /// screens open or queued (a newer manifest replaces their items), and
    /// the first-sight screens now due.
    fn on_plugins_changed(&mut self) {
        let Some(bridge) = self.bridge() else {
            return;
        };
        let lists = bridge.lists();
        let lists: Vec<&PluginList> = lists.iter().collect();
        self.extensions.set_plugins(&lists, &bridge.enabled());
        for name in self.setup_names() {
            if let Some(ManifestState::Ready(m)) = bridge.manifest(&name) {
                if let Some(setup) = self.setup_of(&name).filter(|s| s.differs(&m)) {
                    setup.replace(&m);
                }
            }
        }
        for name in bridge.first_sight() {
            if let (false, Some(ManifestState::Ready(m))) =
                (self.setup_shown(&name), bridge.manifest(&name))
            {
                self.queue_setup(PluginSetup::new(&m, &[], true));
            }
        }
        self.redraw = true;
    }

    /// The setup screen of plugin `name`, if one is open or queued.
    fn setup_of(&mut self, name: &str) -> Option<&mut PluginSetup> {
        if let Some(Modal::PluginSetup(s)) = &mut self.modal {
            if s.name == name {
                return Some(s);
            }
        }
        self.confirms.iter_mut().find_map(|q| match q {
            Queued::Setup(s) if s.name == name => Some(&mut **s),
            _ => None,
        })
    }

    /// The plugins whose setup screen is open or queued.
    fn setup_names(&self) -> Vec<String> {
        let open = match &self.modal {
            Some(Modal::PluginSetup(s)) => Some(s.name.clone()),
            _ => None,
        };
        let queued = self.confirms.iter().filter_map(|q| match q {
            Queued::Setup(s) => Some(s.name.clone()),
            _ => None,
        });
        open.into_iter().chain(queued).collect()
    }

    /// Whether a setup screen of plugin `name` is open or queued.
    pub(super) fn setup_shown(&mut self, name: &str) -> bool {
        self.setup_of(name).is_some()
    }

    /// Show `setup` now, or once no other modal is open.
    fn queue_setup(&mut self, setup: PluginSetup) {
        self.confirms.push_back(Queued::Setup(Box::new(setup)));
        self.show_next_confirm();
    }

    /// Enter on the extensions screen's plugin row `index`: its setup
    /// screen, over (and back to) the extensions screen.
    pub(super) fn open_plugin_setup(&mut self, index: usize, back: ExtensionsView) {
        let Some(ext) = self.extensions.extensions.get(index) else {
            return;
        };
        let name = ext.name.clone();
        if let Source::Plugin {
            refused: Some(why), ..
        } = &ext.source
        {
            self.notify(format!("plugin {name} is refused: {why}"));
            return;
        }
        let bridge = self.bridge();
        let m = match bridge.as_ref().and_then(|b| b.manifest(&name)) {
            Some(ManifestState::Ready(m)) => m,
            Some(ManifestState::Failed(why)) => {
                self.notify(format!("manifest of {name}: {why}"));
                return;
            }
            _ => {
                self.notify(format!("manifest of {name}: not here yet"));
                return;
            }
        };
        let disabled = bridge
            .and_then(|b| b.choices(&name))
            .map(|c| c.disabled)
            .unwrap_or_default();
        let mut setup = PluginSetup::new(&m, &disabled, false);
        setup.back = Some(back);
        self.modal = Some(Modal::PluginSetup(Box::new(setup)));
        self.redraw = true;
    }

    /// A key on the setup screen.
    pub(super) fn plugin_setup_key(&mut self, key: &KeyEvent) {
        let Some(Modal::PluginSetup(setup)) = &mut self.modal else {
            return;
        };
        let outcome = setup.on_key(key);
        self.redraw = true;
        if outcome == Outcome::Stay {
            return;
        }
        let Some(Modal::PluginSetup(setup)) = self.modal.take() else {
            return;
        };
        let name = setup.name.clone();
        match outcome {
            Outcome::Save if self.manifest_refetching(&name) => {
                // The manifest is being fetched again: keep the screen, which
                // shows the new items when they arrive.
                self.notify(format!("{name}: manifest changed, review again"));
                self.modal = Some(Modal::PluginSetup(setup));
                return;
            }
            Outcome::Save => self.save_setup(&setup),
            _ if setup.first_sight => self.emit(Effect::Bridge(BridgeCall::Defer(name))),
            _ => {}
        }
        if let Some(back) = setup.back {
            self.modal = Some(Modal::ExtensionsMenu(back));
        }
        self.show_next_confirm();
        // A save reaches the addon with the effects: `--plain`'s session
        // starts at the next tick, with the choices.
        if outcome != Outcome::Save {
            self.plain_try_start();
        }
    }

    /// Whether plugin `name`'s manifest is being fetched again.
    fn manifest_refetching(&self, name: &str) -> bool {
        let state = self.bridge().and_then(|b| b.manifest(name));
        matches!(state, Some(ManifestState::Fetching))
    }

    /// Enter on `setup`: save the choices (the addon asks every host again).
    fn save_setup(&mut self, setup: &PluginSetup) {
        let Some(bridge) = self.bridge() else {
            return;
        };
        let name = setup.name.clone();
        let Some(ManifestState::Ready(m)) = bridge.manifest(&name) else {
            return;
        };
        let choices = choices_for(&m, &setup.off_ids(), bridge.choices(&name).as_ref());
        self.emit(Effect::Bridge(BridgeCall::SaveChoices { name, choices }));
    }

    /// What `profile`'s enabled plugins bring to a spawn on `host`, and the
    /// requests of those `host` has not answered (the spawn waits for them).
    pub(super) fn plugin_parts(
        &self,
        host: &str,
        profile: &str,
    ) -> (Vec<PluginPart>, Vec<Pending>) {
        self.bridge()
            .map(|b| b.plugin_parts(host, profile))
            .unwrap_or_default()
    }

    /// What the extensions screen says about plugin `name` of `profile`.
    pub(super) fn plugin_details(
        &self,
        name: &str,
        profile: &str,
        refused: Option<&str>,
    ) -> Vec<String> {
        self.bridge()
            .map(|b| b.plugin_details(name, profile, refused, self.now))
            .unwrap_or_default()
    }

    /// `r` on the extensions screen: every list, manifest and host answer
    /// again.
    pub(super) fn refresh_plugins(&mut self) {
        self.emit(Effect::Bridge(BridgeCall::Refresh));
        self.notify("refreshing plugins…");
    }

    /// The profiles sessions use: the one new sessions get, and every open
    /// session's.
    pub fn plugin_profiles(&self) -> BTreeSet<String> {
        let mut profiles: BTreeSet<String> = self
            .sessions
            .iter()
            .filter_map(|v| v.proxy.clone())
            .collect();
        if let Some(p) = self.proxy_override.pick(self.proxy_default.as_deref()) {
            profiles.insert(p.to_owned());
        }
        profiles
    }

    // ── --plain ───────────────────────────────────────────────────────────────

    /// Whether `--plain`'s session must wait for plugin setup: the addon has
    /// not read its lists yet, or its profile has an enabled plugin without
    /// saved choices whose manifest has not arrived (both before
    /// `deadline`) or whose setup screen is open or queued.
    fn plain_waits(&mut self, profile: &str, deadline: u64) -> bool {
        let now = self.now;
        let Some(bridge) = self.bridge() else {
            return false;
        };
        // The cached lists and the choices are read at start, off this thread.
        if !bridge.loaded() {
            return now < deadline;
        }
        let unsaved = bridge.unsaved(profile);
        unsaved
            .iter()
            .any(|(name, ended)| (!ended && now < deadline) || self.setup_shown(name))
    }

    /// Start `--plain`'s deferred session once nothing holds it.
    pub(super) fn plain_try_start(&mut self) {
        let Some((profile, deadline)) = self
            .plain_pending
            .as_ref()
            .map(|(_, proxy, deadline)| (proxy.clone(), *deadline))
        else {
            return;
        };
        if self.mode != Mode::Plain {
            return;
        }
        // No claude here: its install (or the question) comes first.
        if self.claude_install_pending("local") {
            return;
        }
        if let Some(profile) = profile {
            if self.plain_waits(&profile, deadline) {
                return;
            }
        }
        if let Some((start, proxy, _)) = self.plain_pending.take() {
            self.start_plain_now(start, proxy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::vocdoni::plugins::choices::PluginChoices;
    use crate::addons::vocdoni::plugins::state::ApplyState;
    use crate::proto::Msg;
    use crate::tui::confirm::{Choice, ConfirmPrompt};
    use crate::tui::test_support::{
        alt, app_with, bridge, changed, manifest_arrives, plugin_host, press, run_bridge, serve,
    };

    const TEAM: &str = r#"
format = 1
name = "team"
version = "1.0.0"
instructions = "Rules."
[[marketplace]]
name = "m"
source = "github:o/r"
[[plugin]]
name = "a"
marketplace = "m"
[[mcp]]
name = "kb"
type = "sse"
url = "https://kb.example.org"
[[agent]]
name = "reviewer"
description = "Reviews"
prompt = "Review."
"#;

    fn team() -> Manifest {
        Manifest::parse(TEAM).unwrap()
    }

    fn other(name: &str) -> Manifest {
        Manifest::parse(&format!(
            "format = 1\nname = \"{name}\"\nversion = \"2\"\nenv = {{ X = \"y\" }}"
        ))
        .unwrap()
    }

    /// An app (no modal open) with `local` connected and profile `work`
    /// serving `names`, whose manifests are being fetched.
    fn app_serving(names: &[&str]) -> App {
        let mut app = app_with(&[]);
        app.modal = None;
        plugin_host(&mut app, "local");
        serve(&mut app, "work", names);
        app.take_effects();
        app
    }

    fn setup(app: &App) -> &PluginSetup {
        match &app.modal {
            Some(Modal::PluginSetup(s)) => s,
            other => panic!("no setup screen: {}", other.is_some()),
        }
    }

    /// Wait out a first-sight screen's arming.
    fn arm(app: &mut App) {
        for _ in 0..ARM_TICKS {
            app.confirm_tick();
        }
    }

    /// What `local` was asked for plugin `name` (`None`: nothing).
    fn asked(app: &App, name: &str) -> Option<ApplyState> {
        bridge(app).state().apply_state("local", name).cloned()
    }

    /// The disabled ids of the apply request a spawn on `local` with
    /// profile `work` would wait for.
    fn disabled_asked(app: &App) -> Vec<Vec<String>> {
        let (_, pending) = bridge(app).plugin_parts("local", "work");
        pending
            .iter()
            .filter_map(|p| crate::addons::vocdoni::daemon::apply_request_of(&p.msg))
            .map(|r| r.disabled)
            .collect()
    }

    /// The bridge calls among `effects`.
    fn calls(effects: Vec<Effect>) -> Vec<BridgeCall> {
        effects
            .into_iter()
            .filter_map(|e| match e {
                Effect::Bridge(call) => Some(call),
                _ => None,
            })
            .collect()
    }

    /// Queue `calls` again for [`run_bridge`].
    fn emit_all(app: &mut App, calls: &[BridgeCall]) {
        for call in calls {
            app.emit(Effect::Bridge(call.clone()));
        }
    }

    fn setup_details(app: &App, name: &str) -> Option<String> {
        bridge(app).state().setup_details(name)
    }

    #[test]
    fn keys_move_toggle_and_answer() {
        let mut s = PluginSetup::new(&team(), &["mcp:kb".to_owned()], false);
        let ids: Vec<&str> = s.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            ["plugin:a@m", "mcp:kb", "agent:reviewer", "instructions"]
        );
        assert_eq!(s.off_ids(), ["mcp:kb"]);
        let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
        for code in [KeyCode::Down, KeyCode::Char('j'), KeyCode::Char('j')] {
            assert_eq!(s.on_key(&k(code)), Outcome::Stay);
        }
        assert_eq!(s.selected, 3);
        s.on_key(&k(KeyCode::Down));
        assert_eq!(s.selected, 3, "the last item");
        s.on_key(&k(KeyCode::Char('k')));
        s.on_key(&k(KeyCode::Char(' ')));
        assert_eq!(s.off_ids(), ["mcp:kb", "agent:reviewer"]);
        s.on_key(&k(KeyCode::Char('a')));
        assert!(s.off_ids().is_empty());
        s.on_key(&k(KeyCode::Char('n')));
        assert_eq!(s.off_ids().len(), 4);
        s.on_key(&k(KeyCode::Up));
        s.on_key(&k(KeyCode::Up));
        s.on_key(&k(KeyCode::Up));
        assert_eq!(s.selected, 0);
        // A chord is no answer; Enter saves, Esc closes.
        let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(s.on_key(&alt_enter), Outcome::Stay);
        assert_eq!(s.on_key(&k(KeyCode::Enter)), Outcome::Save);
        assert_eq!(s.on_key(&k(KeyCode::Esc)), Outcome::Close);
        // On first sight, keys count only once armed.
        let mut first = PluginSetup::new(&team(), &[], true);
        assert_eq!(first.on_key(&k(KeyCode::Enter)), Outcome::Stay);
        first.armed = 0;
        assert_eq!(first.on_key(&k(KeyCode::Enter)), Outcome::Save);
        assert_eq!(
            first.hint(),
            "Space toggle · a all on · n all off · Enter save · Esc later"
        );
    }

    #[test]
    fn first_sight_shows_every_item_and_nothing_is_asked_until_enter() {
        let mut app = app_serving(&["team"]);
        manifest_arrives(&mut app, "team", team());
        let s = setup(&app);
        assert!(s.first_sight && s.items.iter().all(|i| i.on));
        assert_eq!(asked(&app, "team"), None, "nothing before the setup closes");
        // Keys are swallowed while it arms, Alt+n included.
        app.on_terminal(press(KeyCode::Char(' ')));
        app.on_terminal(alt('n'));
        assert!(setup(&app).off_ids().is_empty());
        assert!(matches!(app.modal, Some(Modal::PluginSetup(_))));
        arm(&mut app);
        app.on_terminal(press(KeyCode::Down));
        app.on_terminal(press(KeyCode::Char(' ')));
        app.on_terminal(press(KeyCode::Enter));
        assert!(app.modal.is_none());
        let effects = app.take_effects();
        assert_eq!(effects.len(), 1, "{effects:?}");
        let ids = team().ids();
        let saved = BridgeCall::SaveChoices {
            name: "team".to_owned(),
            choices: PluginChoices {
                disabled: vec!["mcp:kb".into()],
                reviewed: "1.0.0".into(),
                seen: ids,
            },
        };
        let calls = calls(effects);
        assert_eq!(calls, [saved]);
        // The addon asks every host, with what is off.
        emit_all(&mut app, &calls);
        assert!(run_bridge(&mut app).is_empty());
        assert_eq!(asked(&app, "team"), Some(ApplyState::Pending));
        assert_eq!(disabled_asked(&app), [["mcp:kb"]]);
        assert_eq!(
            setup_details(&app, "team").as_deref(),
            Some("1 items disabled (Enter to change)")
        );
        assert!(app.modal.is_none(), "set up: no screen again");
    }

    #[test]
    fn esc_on_first_sight_defers_for_the_run() {
        let mut app = app_serving(&["team"]);
        manifest_arrives(&mut app, "team", team());
        arm(&mut app);
        app.on_terminal(press(KeyCode::Esc));
        assert!(app.modal.is_none());
        let effects = app.take_effects();
        assert_eq!(effects.len(), 1, "{effects:?}");
        let calls = calls(effects);
        assert_eq!(calls, [BridgeCall::Defer("team".into())]);
        emit_all(&mut app, &calls);
        run_bridge(&mut app);
        assert_eq!(
            setup_details(&app, "team").as_deref(),
            Some("setup pending (Enter)")
        );
        // Fetched again in this run: no screen, nothing asked.
        app.refresh_plugins();
        run_bridge(&mut app);
        manifest_arrives(&mut app, "team", team());
        assert!(app.modal.is_none());
        assert_eq!(asked(&app, "team"), None);
    }

    #[test]
    fn setup_screens_wait_their_turn_with_the_prompts() {
        let mut app = app_serving(&["team", "two"]);
        manifest_arrives(&mut app, "team", team());
        app.queue_confirm(ConfirmPrompt::new(
            "Q",
            "Update?",
            vec![Choice::new('n', "no", None)],
        ));
        manifest_arrives(&mut app, "two", other("two"));
        assert_eq!(setup(&app).name, "team");
        // A newer fetch of a queued screen's manifest replaces its items.
        let two_v3 = Manifest::parse("format = 1\nname = \"two\"\nversion = \"3\"").unwrap();
        manifest_arrives(&mut app, "two", two_v3);
        arm(&mut app);
        app.on_terminal(press(KeyCode::Enter));
        run_bridge(&mut app);
        assert!(
            matches!(app.modal, Some(Modal::Confirm(_))),
            "then the prompt"
        );
        app.confirm_tick();
        for _ in 0..4 {
            app.confirm_tick();
        }
        app.on_terminal(press(KeyCode::Char('n')));
        let s = setup(&app);
        assert_eq!((s.name.as_str(), s.version.as_str()), ("two", "3"));
        assert!(s.items.is_empty());
    }

    #[test]
    fn a_newer_manifest_keeps_the_toggles_of_the_ids_still_there() {
        let mut s = PluginSetup::new(&team(), &["mcp:kb".into(), "plugin:a@m".into()], true);
        s.selected = 3;
        let newer = TEAM
            .replace("name = \"a\"", "name = \"b\"")
            .replace("1.0.0", "1.1.0");
        s.replace(&Manifest::parse(&newer).unwrap());
        assert_eq!(s.version, "1.1.0");
        assert_eq!(s.off_ids(), ["mcp:kb"], "plugin:b@m is new: on");
        assert_eq!((s.selected, s.armed, s.first_sight), (3, ARM_TICKS, true));
    }

    /// Set up before: no screen, and `local` is asked with the saved
    /// choices. (The notice naming the new items is the addon's now:
    /// `vocdoni::state::tests::new_items_of_a_newer_version_are_named_once_and_on`.)
    #[test]
    fn new_items_of_a_newer_version_are_named_once_and_on() {
        let mut app = app_serving(&["team"]);
        let seen = vec!["plugin:a@m".to_owned(), "mcp:kb".to_owned()];
        bridge(&app).state().plugins.choices.insert(
            "team".into(),
            PluginChoices {
                disabled: vec!["mcp:kb".into()],
                reviewed: "0.9".into(),
                seen,
            },
        );
        manifest_arrives(&mut app, "team", team());
        assert!(app.modal.is_none(), "set up before: no screen");
        assert_eq!(asked(&app, "team"), Some(ApplyState::Pending));
        assert_eq!(disabled_asked(&app), [["mcp:kb"]]);
    }

    /// Off: no screen. (No notice either:
    /// `vocdoni::state::tests::a_plugin_turned_off_gets_no_new_items_notice`.)
    #[test]
    fn a_plugin_turned_off_gets_no_new_items_notice() {
        let mut app = app_serving(&["team"]);
        bridge(&app).set_enabled("team", false);
        changed(&mut app);
        manifest_arrives(&mut app, "team", team());
        assert!(app.notice.is_none(), "off: nothing to review");
        assert!(app.modal.is_none(), "off: no screen");
        let row = app.extensions.extensions.iter().find(|e| e.name == "team");
        assert!(row.is_some_and(|e| !e.enabled), "its row is off");
    }

    #[test]
    fn enter_on_a_plugin_row_opens_its_setup_and_esc_goes_back() {
        let mut app = app_serving(&["team"]);
        bridge(&app)
            .state()
            .plugins
            .choices
            .insert("team".into(), PluginChoices::default());
        manifest_arrives(&mut app, "team", team());
        app.take_effects();
        let row = app
            .extensions
            .extensions
            .iter()
            .position(|e| e.name == "team")
            .unwrap();
        app.on_terminal(alt('i'));
        for _ in 0..row {
            app.on_terminal(press(KeyCode::Down));
        }
        app.on_terminal(press(KeyCode::Enter));
        let s = setup(&app);
        assert!(!s.first_sight && s.back.is_some());
        assert_eq!(
            s.hint(),
            "Space toggle · a all on · n all off · Enter save · Esc cancel"
        );
        app.on_terminal(press(KeyCode::Char(' ')));
        app.on_terminal(press(KeyCode::Esc));
        assert!(
            matches!(&app.modal, Some(Modal::ExtensionsMenu(v)) if v.selected == row),
            "back on the extensions screen"
        );
        assert!(app.take_effects().is_empty(), "nothing saved");
        // Space still toggles the plugin.
        app.on_terminal(press(KeyCode::Char(' ')));
        let effects = app.take_effects();
        let saved = effects
            .iter()
            .any(|e| matches!(e, Effect::SaveExtension { name, on: false } if name == "team"));
        assert!(saved, "{effects:?}");
        let off = BridgeCall::SetEnabled {
            name: "team".into(),
            on: false,
        };
        assert_eq!(calls(effects), [off]);
        // Enter on a row that is not a plugin toggles it.
        app.on_terminal(press(KeyCode::Home));
        app.on_terminal(press(KeyCode::Enter));
        let effects = app.take_effects();
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::SaveExtension { name, on: false } if name == "latex")));
    }

    #[test]
    fn enter_on_a_manifest_being_refetched_reopens_with_a_notice() {
        let mut app = app_serving(&["team"]);
        manifest_arrives(&mut app, "team", team());
        arm(&mut app);
        app.on_terminal(press(KeyCode::Char(' ')));
        // The list changed: the manifest is fetched again under the screen.
        let b = bridge(&app);
        b.state().plugins.manifests.get_mut("team").unwrap().1 = ManifestState::Fetching;
        app.take_effects();
        app.on_terminal(press(KeyCode::Enter));
        assert_eq!(
            app.notice.as_ref().unwrap().text,
            "team: manifest changed, review again"
        );
        assert!(!setup(&app).items[0].on, "toggles kept");
        assert!(app.take_effects().is_empty(), "nothing saved");
        // The new items arrive: the open screen shows them, toggles kept.
        let newer = TEAM.replace("1.0.0", "1.1.0");
        manifest_arrives(&mut app, "team", Manifest::parse(&newer).unwrap());
        assert_eq!(setup(&app).version, "1.1.0");
        assert!(!setup(&app).items[0].on);
        app.on_terminal(press(KeyCode::Enter));
        assert!(app.modal.is_none());
    }

    /// A `--plain` app with profile `work` (serving `team`, cached).
    fn plain_app() -> App {
        use crate::addons::vocdoni::proxy::plugins::{sample, PluginList};
        use crate::tui::app::{AppConfig, Mode};
        let mut app = App::new(AppConfig {
            mode: Mode::Plain,
            proxy_default: Some("work".into()),
            proxy_profiles: vec!["work".into()],
            ..crate::tui::test_support::config()
        });
        app.now = 1000;
        let list = PluginList {
            profile: "work".into(),
            fetched_at: 1,
            plugins: vec![sample("team")],
        };
        let profiles = (vec!["work".to_owned()], Some("work".to_owned()));
        let b = bridge(&app);
        b.state()
            .load(profiles, Default::default(), Default::default(), vec![list]);
        b.state().host_up("local", true);
        changed(&mut app);
        app.take_effects();
        app
    }

    fn start(app: &mut App) {
        app.start_plain(crate::tui::plain::PlainStart {
            id: uuid::Uuid::new_v4(),
            args: vec![],
            cwd: "/w".into(),
        });
    }

    fn spawned(app: &mut App) -> Option<Vec<String>> {
        app.take_effects().into_iter().find_map(|e| match e {
            Effect::SpawnWithProxy { pending, .. } => {
                Some(pending.into_iter().map(|p| p.name).collect())
            }
            Effect::Request {
                msg: Msg::Spawn(_), ..
            } => Some(Vec::new()),
            _ => None,
        })
    }

    #[test]
    fn plain_waits_for_a_new_manifest_and_its_setup_then_spawns() {
        let mut app = plain_app();
        start(&mut app);
        assert_eq!(spawned(&mut app), None, "the manifest is on its way");
        manifest_arrives(&mut app, "team", team());
        assert_eq!(spawned(&mut app), None, "the setup is open");
        arm(&mut app);
        app.on_terminal(press(KeyCode::Enter));
        // The choices reach the addon with the effects; the tick starts it.
        assert!(run_bridge(&mut app).is_empty());
        app.plain_try_start();
        assert_eq!(
            spawned(&mut app),
            Some(vec!["team".to_owned()]),
            "waits for local's answer"
        );
    }

    #[test]
    fn plain_spawns_at_once_when_set_up_and_after_20_s_without_a_manifest() {
        let mut app = plain_app();
        bridge(&app)
            .state()
            .plugins
            .choices
            .insert("team".into(), PluginChoices::default());
        start(&mut app);
        assert!(spawned(&mut app).is_some());

        let mut app = plain_app();
        start(&mut app);
        app.now = 1019;
        app.plain_try_start();
        assert_eq!(spawned(&mut app), None);
        app.now = 1020;
        app.plain_try_start();
        assert_eq!(spawned(&mut app), Some(Vec::new()), "without the plugin");
    }

    /// Until the addon has read its lists, `--plain` waits (20 s at most).
    #[test]
    fn plain_waits_for_the_addon_to_read_its_lists() {
        use crate::tui::app::{AppConfig, Mode};
        let mut app = App::new(AppConfig {
            mode: Mode::Plain,
            proxy_default: Some("work".into()),
            ..crate::tui::test_support::config()
        });
        app.now = 1000;
        start(&mut app);
        assert_eq!(spawned(&mut app), None);
        app.now = 1020;
        app.plain_try_start();
        assert!(spawned(&mut app).is_some());
    }
}
