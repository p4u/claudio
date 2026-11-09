//! [`Ctx`]: the one [`AddonCtx`] implementation, one per addon (D2). It holds
//! the runtime weakly, so an addon keeping its context never keeps the
//! runtime alive.

use std::sync::{Arc, Weak};

use serde_json::Value;

use super::{Inner, RuntimeOut};
use crate::addon::{self, AddonCtx, AddonError, BoxFuture, ConfigEdit, Level, Notice, SessionSnap};
use crate::proto::{self, Msg};

/// The cap of an offered checklist id.
const CHECKLIST_ID_BYTES: usize = 64;

pub(super) struct Ctx {
    inner: Weak<Inner>,
    addon: &'static str,
}

impl Ctx {
    pub(super) fn new(inner: &Arc<Inner>, addon: &'static str) -> Ctx {
        Ctx {
            inner: Arc::downgrade(inner),
            addon,
        }
    }

    /// Whether `section` is one this addon declared (A3).
    fn declared(&self, inner: &Inner, section: &str) -> bool {
        inner
            .addons
            .get(self.addon)
            .is_some_and(|a| a.config_sections().contains(&section))
    }
}

/// `[extensions] <name>` as `config` has it. `[claude] latex` is the
/// documented alias of `[extensions] latex`, mapped here once: an explicit
/// `[extensions] latex` wins, else `[claude] latex`, else `None` (the
/// extension's default).
pub(super) fn extension_choice(config: &toml::Table, name: &str) -> Option<bool> {
    let explicit = config
        .get("extensions")
        .and_then(|t| t.get(name)?.as_bool());
    let alias = || config.get("claude")?.get("latex")?.as_bool();
    explicit.or_else(|| (name == "latex").then(alias).flatten())
}

/// A finished request: `err` now.
fn failed<T: Send + 'static>(err: T) -> BoxFuture<'static, T> {
    Box::pin(async move { err })
}

impl AddonCtx for Ctx {
    fn spawn(&self, task: BoxFuture<'static, ()>) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let (limit, timeout) = (inner.limits.tasks, inner.limits.task_timeout);
        // A task of the addon's start holds the start's guard until it ends.
        let start = super::events::start_guard();
        let mut tasks = inner.tasks();
        if tasks.stopped {
            return;
        }
        let set = tasks.addons.entry(self.addon).or_default();
        while set.try_join_next().is_some() {}
        if set.len() >= limit {
            tracing::warn!(
                addon = self.addon,
                limit,
                "addon task dropped: too many running"
            );
            return;
        }
        let addon = self.addon;
        let run = async move {
            let _start = start;
            if tokio::time::timeout(timeout, task).await.is_err() {
                tracing::warn!(addon, "addon task timed out");
            }
        };
        set.spawn_on(run, &inner.runtime);
    }

    fn request(
        &self,
        host: &str,
        op: &str,
        body: Value,
    ) -> BoxFuture<'static, Result<Value, AddonError>> {
        let not_connected = || AddonError::NotConnected(host.to_owned());
        let Some(inner) = self.inner.upgrade() else {
            return failed(Err(not_connected()));
        };
        let (client, has) = match inner.state().hosts.get(host) {
            Some(h) => (
                h.client.clone(),
                h.features.contains(&addon::feature(self.addon)),
            ),
            None => (None, false),
        };
        let Some(client) = client else {
            return failed(Err(not_connected()));
        };
        if !has {
            return failed(Err(AddonError::Missing(host.to_owned())));
        }
        if proto::addon_body_ok(&body).is_err() {
            return failed(Err(AddonError::TooLarge));
        }
        let msg = Msg::AddonOp {
            addon: self.addon.to_owned(),
            op: op.to_owned(),
            body,
        };
        let timeout = inner.limits.request_timeout;
        let reply = client.request(msg);
        Box::pin(async move {
            match tokio::time::timeout(timeout, reply).await {
                Err(_) => Err(AddonError::Timeout),
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::TimedOut => Err(AddonError::Timeout),
                Ok(Err(e)) => Err(AddonError::Daemon(e.to_string())),
                Ok(Ok(Msg::AddonReply { body })) => match proto::addon_body_ok(&body) {
                    Ok(()) => Ok(body),
                    Err(_) => Err(AddonError::TooLarge),
                },
                Ok(Ok(_)) => Err(AddonError::Daemon("unexpected reply".to_owned())),
            }
        })
    }

    fn host_has(&self, host: &str) -> bool {
        let Some(inner) = self.inner.upgrade() else {
            return false;
        };
        let feature = addon::feature(self.addon);
        let state = inner.state();
        state
            .hosts
            .get(host)
            .is_some_and(|h| h.client.is_some() && h.features.contains(&feature))
    }

    fn sessions(&self) -> Vec<SessionSnap> {
        self.inner
            .upgrade()
            .map(|i| i.state().sessions.clone())
            .unwrap_or_default()
    }

    fn config(&self, section: &str) -> toml::Table {
        let Some(inner) = self.inner.upgrade() else {
            return toml::Table::new();
        };
        if !self.declared(&inner, section) {
            return toml::Table::new();
        }
        let state = inner.state();
        match state.config.get(section) {
            Some(toml::Value::Table(t)) => t.clone(),
            _ => toml::Table::new(),
        }
    }

    fn edit_config(
        &self,
        section: &str,
        edit: ConfigEdit,
    ) -> BoxFuture<'static, Result<(), String>> {
        let Some(inner) = self.inner.upgrade() else {
            return failed(Err("claudio is stopping".to_owned()));
        };
        if !self.declared(&inner, section) {
            return failed(Err(format!("[{section}] is not {}'s", self.addon)));
        }
        let section = section.to_owned();
        Box::pin(async move {
            let _edit = inner.edit.lock().await;
            let path = inner.opts.config_path.clone();
            let name = section.clone();
            let edited = tokio::task::spawn_blocking(move || {
                // The section as written: the file's `[section]` parses to
                // exactly this (edit_section checks it).
                let mut after = toml::Table::new();
                let edited = crate::app::config_edit::edit_section(&path, &name, |t| {
                    edit(t);
                    after = t.clone();
                });
                edited.map(|edited| edited.then_some(after))
            })
            .await
            .map_err(|_| "the config edit stopped".to_owned())?
            .map_err(|e| e.to_string())?;
            // Applied as written, with no second read of the file: a read
            // in flight is older and is dropped (generation check).
            if let Some(after) = edited {
                let mut state = inner.state();
                state.config.insert(section, toml::Value::Table(after));
                state.reload.edited();
                drop(state);
                inner.event(addon::AddonEvent::ConfigChanged);
            }
            Ok(())
        })
    }

    fn extension_on(&self, name: &str) -> Option<bool> {
        let inner = self.inner.upgrade()?;
        let state = inner.state();
        extension_choice(&state.config, name)
    }

    fn core_extensions(&self) -> Vec<String> {
        self.inner
            .upgrade()
            .map(|i| i.state().core_extensions.clone())
            .unwrap_or_default()
    }

    fn notify(&self, text: String) {
        if let Some(inner) = self.inner.upgrade() {
            let notice = addon::notice(Notice {
                text,
                level: Level::Info,
            });
            inner.outbox.push(RuntimeOut::Notice {
                addon: self.addon,
                notice,
            });
        }
    }

    fn changed(&self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.outbox.changed(self.addon);
        }
    }

    fn offer_checklist(&self, id: String) {
        if let Some(inner) = self.inner.upgrade() {
            inner.outbox.push(RuntimeOut::Offer {
                addon: self.addon,
                checklist: addon::line(&id, CHECKLIST_ID_BYTES),
            });
        }
    }
}
