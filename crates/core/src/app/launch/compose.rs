//! Compose: the user's custom env and the addons' parts of a launch into
//! its `Spawn` or `Respawn` (docs/addons.md, "Launches", step 4). Pure; the
//! one implementation for every front-end. The env it writes is a secret:
//! nothing here logs or formats it, and its notices name keys only.

use std::collections::BTreeSet;

use crate::addon::{line, LaunchArg, LaunchPart, TEXT_BYTES};
use crate::claude::env::{check, name_ok, Value};
use crate::proto::{
    lens_name_ok, Envelope, Frame, LensSpec, Msg, SessionExtensions, MAX_FRAME, MAX_INSTRUCTION,
    MAX_INSTRUCTIONS, MAX_LENSES, MAX_PLUGIN_INSTRUCTIONS,
};

/// The most bytes of a lens name quoted in a notice.
const NAME_BYTES: usize = 64;

/// How the notice naming the custom env an addon overrode starts (a
/// recovery drops it: `super::send`).
pub(super) const OVERRIDDEN: &str = "custom env overridden by ";

/// `base` (a `Spawn` or `Respawn` without env, env_remove, optional or
/// extensions) with the user's custom env (`app::env::Store::effective`),
/// the `parts`, in registry order, and the core's lenses:
///
/// - env: the user's first (an entry the validator refuses is left out with
///   a notice), then each part's in order, a later key replacing an earlier
///   one, no key twice; a user key that a part's `env_remove` names and no
///   part sets is dropped (an addon's removal wins too); one notice names
///   the user keys a part replaced or removed; `env_remove` the sorted
///   union, always `Some`;
/// - a `Spawn`'s args appended in order (`Optional { flag, value }` as
///   `flag value`, with `flag` in `optional`); a `Respawn` gets none (its
///   journal has them);
/// - lenses: the core's first, then the addons', merged by name (first
///   wins), within the caps (one breaking a cap is left out with a notice);
/// - instructions joined by a blank line, cut at the last whole one that
///   fits [`MAX_PLUGIN_INSTRUCTIONS`], with a notice naming those left out;
/// - the parts' notices, cleaned, each prefixed with its addon.
///
/// Fails when the framed request would exceed [`MAX_FRAME`].
pub fn compose(
    base: Msg,
    core: SessionExtensions,
    user: Vec<(String, Value)>,
    parts: Vec<(&'static str, LaunchPart)>,
) -> Result<(Msg, Vec<String>), String> {
    let mut notices = Vec::new();
    let mut env = Env::new(user, &mut notices);
    let mut args = Vec::new();
    let mut optional: Vec<String> = Vec::new();
    let mut lenses = Lenses::new(core.lenses);
    let mut instructions = Instructions::default();
    instructions.add("core", &core.instructions);
    for (addon, part) in parts {
        env.add(addon, part.env, part.env_remove);
        for LaunchArg::Optional { flag, value } in part.args {
            if !optional.contains(&flag) {
                optional.push(flag.clone());
            }
            args.extend([flag, value]);
        }
        lenses.add(addon, part.lenses, &mut notices);
        instructions.add(addon, &part.instructions);
        let own = part
            .notices
            .iter()
            .map(|n| format!("{addon}: {}", line(n, TEXT_BYTES)));
        notices.extend(own);
    }
    let ext = SessionExtensions {
        lenses: lenses.list,
        instructions: instructions.finish(&mut notices),
    };
    ext.validate()
        .map_err(|e| format!("launch extensions: {e}"))?;
    let (env, env_remove) = env.finish(&mut notices);
    let env_remove = Some(env_remove.into_iter().collect());
    let msg = match base {
        Msg::Spawn(mut spec) => {
            spec.args.extend(args);
            for flag in optional {
                if !spec.optional.contains(&flag) {
                    spec.optional.push(flag);
                }
            }
            spec.env = env;
            spec.env_remove = env_remove;
            spec.extensions = Some(ext);
            Msg::Spawn(spec)
        }
        Msg::Respawn(mut spec) => {
            spec.env = env;
            spec.env_remove = env_remove;
            spec.extensions = Some(ext);
            Msg::Respawn(spec)
        }
        _ => return Err("only a claude spawn or respawn is composed".to_owned()),
    };
    if !frame_fits(&msg) {
        return Err("launch too large".to_owned());
    }
    Ok((msg, notices))
}

/// Whether `msg`, framed as a request under the largest request id, fits
/// [`MAX_FRAME`].
fn frame_fits(msg: &Msg) -> bool {
    let frame = Frame::Control(Envelope::request(u64::MAX, msg.clone()));
    frame.encode().len().saturating_sub(4) <= MAX_FRAME
}

/// The env as it is layered: the user's custom env, then each part's.
struct Env {
    list: Vec<(String, String)>,
    /// The user's keys.
    user: BTreeSet<String>,
    /// The keys a part set.
    set: BTreeSet<String>,
    /// The union of the parts' `env_remove`.
    remove: BTreeSet<String>,
    /// The addons that replaced or removed a user key, and those keys, in
    /// order, each once.
    by: Vec<&'static str>,
    overridden: Vec<String>,
}

impl Env {
    /// The user's entries the validator takes; a refused one is a notice
    /// (naming a well-formed key only).
    fn new(user: Vec<(String, Value)>, notices: &mut Vec<String>) -> Env {
        let mut env = Env {
            list: Vec::new(),
            user: BTreeSet::new(),
            set: BTreeSet::new(),
            remove: BTreeSet::new(),
            by: Vec::new(),
            overridden: Vec::new(),
        };
        for (key, value) in user {
            match check(&key, value.as_str()) {
                Ok(()) if !env.user.contains(&key) => {
                    env.user.insert(key.clone());
                    env.list.push((key, value.into_string()));
                }
                Ok(()) => {}
                Err(why) if name_ok(&key) => notices.push(format!("custom env skipped: {why}")),
                Err(_) => notices.push("custom env: an entry with a bad name skipped".to_owned()),
            }
        }
        env
    }

    /// `addon`'s env (its keys replace earlier ones) and removals.
    fn add(&mut self, addon: &'static str, env: Vec<(String, String)>, remove: Vec<String>) {
        for (key, value) in env {
            if self.user.contains(&key) {
                self.overrode(addon, &key);
            }
            self.set.insert(key.clone());
            match self.list.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => self.list.push((key, value)),
            }
        }
        for key in remove {
            if self.user.contains(&key) && !self.set.contains(&key) {
                self.overrode(addon, &key);
            }
            self.remove.insert(key);
        }
    }

    fn overrode(&mut self, addon: &'static str, key: &str) {
        if !self.by.contains(&addon) {
            self.by.push(addon);
        }
        if !self.overridden.iter().any(|k| k == key) {
            self.overridden.push(key.to_owned());
        }
    }

    /// The env (a user key a part removed and none set dropped) and
    /// `env_remove`; one notice names the user keys the parts overrode.
    fn finish(self, notices: &mut Vec<String>) -> (Vec<(String, String)>, BTreeSet<String>) {
        if !self.overridden.is_empty() {
            let text = format!(
                "{OVERRIDDEN}{}: {}",
                self.by.join(", "),
                self.overridden.join(", ")
            );
            notices.push(line(&text, TEXT_BYTES));
        }
        let (user, set, remove) = (&self.user, &self.set, &self.remove);
        let dropped = |k: &String| user.contains(k) && remove.contains(k) && !set.contains(k);
        let list = self.list.into_iter().filter(|(k, _)| !dropped(k)).collect();
        (list, self.remove)
    }
}

/// The session's lenses as they are merged.
struct Lenses {
    list: Vec<LensSpec>,
    /// Their instructions' bytes after `sanitize`.
    total: usize,
}

impl Lenses {
    /// The core's lenses, as its registry fitted them.
    fn new(core: Vec<LensSpec>) -> Lenses {
        let total = core
            .iter()
            .map(|l| crate::text::sanitize(&l.instruction).len())
            .sum();
        Lenses { list: core, total }
    }

    /// `addon`'s lenses: a name already taken is merged (the first wins);
    /// one that breaks a cap is left out with a notice.
    fn add(&mut self, addon: &str, lenses: Vec<LensSpec>, notices: &mut Vec<String>) {
        for lens in lenses {
            if self.list.iter().any(|l| l.name == lens.name) {
                continue;
            }
            let size = crate::text::sanitize(&lens.instruction).len();
            let why = if !lens_name_ok(&lens.name) {
                Some("bad name".to_owned())
            } else if size > MAX_INSTRUCTION {
                Some(format!("instruction over {MAX_INSTRUCTION} bytes"))
            } else if self.list.len() == MAX_LENSES {
                Some(format!("more than {MAX_LENSES} lenses"))
            } else if self.total + size > MAX_INSTRUCTIONS {
                Some(format!("instructions over {MAX_INSTRUCTIONS} bytes"))
            } else {
                None
            };
            match why {
                Some(why) => {
                    let name = line(&lens.name, NAME_BYTES);
                    notices.push(format!("{addon}: lens {name} left out: {why}"));
                }
                None => {
                    self.total += size;
                    self.list.push(lens);
                }
            }
        }
    }
}

/// The instructions as they are joined.
#[derive(Default)]
struct Instructions {
    kept: Vec<String>,
    size: usize,
    /// The addons whose instructions did not fit (and every one after).
    cut: Vec<&'static str>,
}

impl Instructions {
    /// `addon`'s instructions, sanitized and trimmed, if they and all
    /// before them fit.
    fn add(&mut self, addon: &'static str, text: &str) {
        let text = crate::text::sanitize(text);
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let sep = if self.kept.is_empty() { 0 } else { 2 };
        if !self.cut.is_empty() || self.size + sep + text.len() > MAX_PLUGIN_INSTRUCTIONS {
            self.cut.push(addon);
            return;
        }
        self.size += sep + text.len();
        self.kept.push(text.to_owned());
    }

    /// The joined text; a notice names those left out.
    fn finish(self, notices: &mut Vec<String>) -> String {
        if !self.cut.is_empty() {
            notices.push(format!(
                "instructions over {MAX_PLUGIN_INSTRUCTIONS} bytes: {} left out",
                self.cut.join(", ")
            ));
        }
        self.kept.join("\n\n")
    }
}
