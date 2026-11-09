//! How a session is routed when it is launched: the wizard's preselected
//! route, its Enter, and a restart's or recovery's select (`impl App`).

use crate::addon::LaunchSelect;

use super::SessionView;
use crate::tui::app::App;

impl App {
    /// How session `v` is launched again (a restart, a recovery): as it was
    /// (a run's switches added), or `Err` for a saved routing no addon
    /// adopts, which never becomes direct (F4).
    pub(in crate::tui) fn relaunch_select(&self, v: &SessionView) -> Result<LaunchSelect, String> {
        if let Some(legacy) = &v.unadopted {
            let why = crate::app::launch::adopt_legacy(&self.addons, legacy).err();
            return Err(why.unwrap_or_else(|| "its saved routing is unknown".to_owned()));
        }
        let mut select = LaunchSelect::of(v.launch.as_ref());
        if v.launch.is_some() {
            for flag in &self.launch_default.flags {
                if !select.flags.contains(flag) {
                    select.flags.push(flag.clone());
                }
            }
        }
        Ok(select)
    }

    /// The route the wizard preselects: the active session's, else the
    /// run's (flags, else the addons' default, which the route row and the
    /// composer resolve; never direct unless asked), with the run's switches.
    pub(in crate::tui) fn preselected(&self) -> LaunchSelect {
        let pick = match self.active_view().and_then(|v| v.launch.as_ref()) {
            Some(choice) => LaunchSelect::of(Some(choice)).pick,
            None => self.launch_default.pick.clone(),
        };
        let flags = self.launch_default.flags.clone();
        LaunchSelect { pick, flags }
    }

    /// The wizard's Enter: refused while its route is an option the
    /// options lack (the wizard stays open to pick another), else spawned.
    pub(in crate::tui) fn wizard_spawn(
        &mut self,
        host: String,
        cwd: String,
        resume: Option<String>,
        select: LaunchSelect,
    ) {
        if let Some(why) = self.wizard_mut().and_then(|w| w.route.unavailable()) {
            return self.notify(why);
        }
        self.modal = None;
        self.spawn(host, cwd, resume, select);
    }
}
