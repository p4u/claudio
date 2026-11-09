//! The addons' extension rows (docs/addons.md, "Extension rows"): read for
//! the front-ends' registries, toggled and refreshed through the runtime,
//! every string cleaned (C2) and every call bounded (D13).

use std::time::Duration;

use super::AddonRuntime;
use crate::addon::{self, line, ExtensionRow, Notice, RowAction, TEXT_BYTES};

/// A row action's bound (D13).
const ROW_ACTION: Duration = Duration::from_secs(15);

impl AddonRuntime {
    /// Every addon's extension rows, cleaned, in registry order.
    pub fn extension_rows(&self) -> Vec<(&'static str, ExtensionRow)> {
        let mut rows = Vec::new();
        for entry in self.addons().iter() {
            let Some(ui) = entry.ui() else {
                continue;
            };
            let ctx = self.ctx(entry.name());
            let own = ui.extension_rows(ctx.as_ref()).into_iter();
            rows.extend(own.map(|r| (entry.name(), addon::row(r))));
        }
        rows
    }

    /// Tell `addon` what happened to its row `row`, within 15 s: its
    /// notice (cleaned), or why not.
    pub async fn row_action(
        &self,
        addon: &'static str,
        row: String,
        action: RowAction,
    ) -> Result<Option<Notice>, String> {
        let entry = self.addons().get(addon).cloned();
        let entry = entry.ok_or_else(|| format!("no addon {addon}"))?;
        let ctx = self.ctx(addon);
        let mut job = tokio::spawn(async move {
            let ui = entry.ui().ok_or("no extension rows")?;
            ui.row_action(row, action, ctx).await
        });
        match tokio::time::timeout(ROW_ACTION, &mut job).await {
            Ok(Ok(Ok(notice))) => Ok(notice.map(addon::notice)),
            Ok(Ok(Err(why))) => Err(format!("{addon}: {}", line(&why, TEXT_BYTES))),
            Ok(Err(_)) => Err(format!("{addon}: the row action failed")),
            Err(_) => {
                job.abort();
                Err(format!("{addon}: the row action timed out"))
            }
        }
    }

    /// The core's toggle of `addon`'s row `name` (D10): `[extensions]
    /// <name> = on` written (the process-wide config lock), the config
    /// re-read, then the addon told. The addon's notice, if any.
    pub async fn toggle_row(
        &self,
        addon: &'static str,
        name: String,
        on: bool,
    ) -> Result<Option<Notice>, String> {
        let (path, own) = (self.inner.opts.config_path.clone(), name.clone());
        let saved =
            tokio::task::spawn_blocking(move || crate::config::save_extension(&path, &own, on))
                .await
                .map_err(|_| "the save stopped".to_owned())?;
        saved.map_err(|e| format!("could not save: {e}"))?;
        self.config_changed();
        self.row_action(addon, name, RowAction::Toggled(on)).await
    }

    /// `Refresh` to every addon with rows (named by its first row), at
    /// once: each one's notice, or why not.
    pub async fn refresh_rows(&self) -> Vec<Result<Option<Notice>, String>> {
        let mut seen: Vec<&'static str> = Vec::new();
        let mut asks = Vec::new();
        for (addon, row) in self.extension_rows() {
            if !seen.contains(&addon) {
                seen.push(addon);
                asks.push(self.row_action(addon, row.name, RowAction::Refresh));
            }
        }
        futures::future::join_all(asks).await
    }
}
