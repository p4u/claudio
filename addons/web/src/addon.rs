//! `WebAddon`: the web UI as an addon. Its `Commands` claim `claudio web …`;
//! its `Launch` asks every claude session, from any front-end, for the
//! `<latex>` and ```` ```mermaid ```` lenses the page renders (G5), while
//! `[extensions] latex`/`diagram` are on; its `Ui` lists those two rows on
//! the extensions screens. The toggles are the core's (D10).

use std::process::ExitCode;
use std::sync::Arc;

use claudio_core::addon::{
    Addon, AddonCtx, Addons, BoxFuture, Commands, ExtensionRow, Launch, LaunchError, LaunchPart,
    LaunchRequest, LaunchSelect, Notice, Routing, RowAction, Ui,
};
use claudio_core::proto::{LensKind, LensSpec};

/// The diagram lens's instruction (spec §7).
pub const DIAGRAM_CONTEXT: &str = "This terminal draws Mermaid diagrams. When a diagram \
helps (architecture, data or control flow, state machines, message sequences), write it as \
a fenced code block with the language `mermaid`, using `flowchart TD`/`flowchart LR` or \
`sequenceDiagram`. Keep labels short (under 30 characters) and diagrams small (under 20 \
nodes). Do not draw box diagrams with characters yourself.";

/// The `[extensions]` row of the `<latex>` lens.
const LATEX: &str = "latex";
/// The `[extensions]` row of the mermaid lens.
const DIAGRAM: &str = "diagram";

/// The web addon.
pub struct WebAddon;

impl Addon for WebAddon {
    fn name(&self) -> &'static str {
        "web"
    }

    fn commands(&self) -> Option<&dyn Commands> {
        Some(self)
    }

    fn launch(&self) -> Option<&dyn Launch> {
        Some(self)
    }

    fn ui(&self) -> Option<&dyn Ui> {
        Some(self)
    }
}

impl Commands for WebAddon {
    fn dispatch(&self, argv: &[String], addons: &Arc<Addons>) -> Option<ExitCode> {
        (argv.get(1).map(String::as_str) == Some("web"))
            .then(|| super::run(&argv[2..], Arc::clone(addons)))
    }
}

/// Whether row `name` is on: unless `[extensions] <name>` is false (the
/// runtime maps `[claude] latex = false` onto `latex`).
fn on(ctx: &dyn AddonCtx, name: &str) -> bool {
    ctx.extension_on(name) != Some(false)
}

/// The lenses the rows that are on ask for.
fn lenses(ctx: &dyn AddonCtx) -> Vec<LensSpec> {
    let mut lenses = Vec::new();
    if on(ctx, LATEX) {
        lenses.push(LensSpec {
            name: "latex".to_owned(),
            kind: LensKind::Tag,
            instruction: claudio_core::claude::hooks::LATEX_CONTEXT.to_owned(),
        });
    }
    if on(ctx, DIAGRAM) {
        lenses.push(LensSpec {
            name: "mermaid".to_owned(),
            kind: LensKind::Fence,
            instruction: DIAGRAM_CONTEXT.to_owned(),
        });
    }
    lenses
}

impl Launch for WebAddon {
    fn resolve(&self, _select: &LaunchSelect, _ctx: &dyn AddonCtx) -> Result<Routing, String> {
        Ok(Routing::Optional)
    }

    fn prepare<'a>(
        &'a self,
        _req: LaunchRequest,
        ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<LaunchPart, LaunchError>> {
        let part = LaunchPart {
            lenses: lenses(ctx.as_ref()),
            ..LaunchPart::default()
        };
        Box::pin(async move { Ok(part) })
    }
}

impl Ui for WebAddon {
    fn extension_rows(&self, ctx: &dyn AddonCtx) -> Vec<ExtensionRow> {
        let row = |name: &str, description: &str, lens: &str| ExtensionRow {
            name: name.to_owned(),
            description: description.to_owned(),
            // `[claude] latex = false` with no `[extensions] latex` shows off.
            default_on: on(ctx, name),
            refused: None,
            details: vec![format!("{lens}  rendered by the web UI")],
            setup: None,
        };
        vec![
            row(
                LATEX,
                "math rendered from <latex> tags in the web UI",
                "<latex>",
            ),
            row(
                DIAGRAM,
                "Mermaid diagrams drawn in the web UI",
                "```mermaid",
            ),
        ]
    }

    fn row_action<'a>(
        &'a self,
        _row: String,
        _action: RowAction,
        _ctx: Arc<dyn AddonCtx>,
    ) -> BoxFuture<'a, Result<Option<Notice>, String>> {
        // The core wrote `[extensions]`; the next launch reads it.
        Box::pin(async { Ok(None) })
    }
}

#[cfg(test)]
mod tests;
