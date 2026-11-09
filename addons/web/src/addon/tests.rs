//! The web addon's lenses and rows, with a context whose `[extensions]`
//! the test sets (the runtime's `[claude] latex` alias is tested in
//! `app::addons`: here it is the `latex` row's `Some(false)`).

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::Value;

use super::*;
use claudio_core::addon::{AddonError, ConfigEdit, Frontend, LaunchOp, Routing, SessionSnap};

/// A context that answers `extension_on` from `on` and nothing else.
#[derive(Default)]
struct Ctx {
    on: Mutex<BTreeMap<String, bool>>,
}

impl Ctx {
    fn with(on: &[(&str, bool)]) -> Arc<Ctx> {
        let ctx = Ctx::default();
        *ctx.on.lock().unwrap() = on.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        Arc::new(ctx)
    }
}

impl AddonCtx for Ctx {
    fn spawn(&self, _task: BoxFuture<'static, ()>) {}
    fn request(
        &self,
        host: &str,
        _op: &str,
        _body: Value,
    ) -> BoxFuture<'static, Result<Value, AddonError>> {
        let host = host.to_owned();
        Box::pin(async move { Err(AddonError::NotConnected(host)) })
    }
    fn host_has(&self, _host: &str) -> bool {
        false
    }
    fn sessions(&self) -> Vec<SessionSnap> {
        Vec::new()
    }
    fn config(&self, _section: &str) -> toml::Table {
        toml::Table::new()
    }
    fn edit_config(
        &self,
        _section: &str,
        _edit: ConfigEdit,
    ) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Err("no config".to_owned()) })
    }
    fn extension_on(&self, name: &str) -> Option<bool> {
        self.on.lock().unwrap().get(name).copied()
    }
    fn core_extensions(&self) -> Vec<String> {
        Vec::new()
    }
    fn notify(&self, _text: String) {}
    fn changed(&self) {}
    fn offer_checklist(&self, _id: String) {}
}

fn request() -> LaunchRequest {
    LaunchRequest {
        op: LaunchOp::New,
        sid: uuid::Uuid::from_u128(1),
        host: "local".into(),
        cwd: "/w".into(),
        choice: None,
        routing: Routing::Optional,
        frontend: Frontend::Tui,
        deferred: Vec::new(),
    }
}

/// The lens names `ctx` gets a launch.
async fn lens_names(ctx: Arc<Ctx>) -> Vec<String> {
    let part = WebAddon.prepare(request(), ctx).await.unwrap();
    assert!(part.env.is_empty() && part.args.is_empty() && part.env_remove.is_empty());
    part.lenses.into_iter().map(|l| l.name).collect()
}

#[tokio::test]
async fn lenses_are_on_by_default_and_follow_their_rows() {
    assert_eq!(lens_names(Ctx::with(&[])).await, ["latex", "mermaid"]);
    // `[extensions] latex = false`, or `[claude] latex = false` as the
    // runtime maps it: the same answer.
    assert_eq!(
        lens_names(Ctx::with(&[("latex", false)])).await,
        ["mermaid"]
    );
    // The diagram row is independent of latex.
    assert_eq!(
        lens_names(Ctx::with(&[("diagram", false)])).await,
        ["latex"]
    );
    let none = Ctx::with(&[("latex", false), ("diagram", false)]);
    assert!(lens_names(none).await.is_empty());
    assert_eq!(
        lens_names(Ctx::with(&[("latex", true), ("diagram", true)])).await,
        ["latex", "mermaid"]
    );
    // The instructions travel in the lens specs.
    let part = WebAddon.prepare(request(), Ctx::with(&[])).await.unwrap();
    assert!(part.instructions.is_empty());
    assert_eq!(part.lenses[0].kind, LensKind::Tag);
    assert_eq!(
        part.lenses[0].instruction,
        claudio_core::claude::hooks::LATEX_CONTEXT
    );
    assert_eq!(part.lenses[1].kind, LensKind::Fence);
    assert_eq!(part.lenses[1].instruction, DIAGRAM_CONTEXT);
}

#[tokio::test]
async fn web_resolves_optional_and_its_rows_are_latex_and_diagram() {
    let ctx = Ctx::with(&[("latex", false)]);
    let select = LaunchSelect::default();
    assert_eq!(
        WebAddon.resolve(&select, ctx.as_ref()),
        Ok(Routing::Optional)
    );
    let rows = WebAddon.extension_rows(ctx.as_ref());
    let names: Vec<(&str, bool)> = rows
        .iter()
        .map(|r| (r.name.as_str(), r.default_on))
        .collect();
    assert_eq!(names, [("latex", false), ("diagram", true)]);
    assert_eq!(
        rows[0].description,
        "math rendered from <latex> tags in the web UI"
    );
    assert_eq!(rows[1].description, "Mermaid diagrams drawn in the web UI");
    assert!(rows
        .iter()
        .all(|r| r.refused.is_none() && r.setup.is_none()));
    // A toggle is the core's to write: the addon has nothing to add.
    let told = WebAddon.row_action("latex".into(), RowAction::Toggled(false), ctx);
    assert_eq!(told.await, Ok(None));
}

#[test]
fn web_claims_its_word_only() {
    let addons = Arc::new(Addons::empty());
    let argv = |w: &[&str]| -> Vec<String> {
        std::iter::once("claudio")
            .chain(w.iter().copied())
            .map(String::from)
            .collect()
    };
    assert!(WebAddon.dispatch(&argv(&[]), &addons).is_none());
    assert!(WebAddon.dispatch(&argv(&["webx"]), &addons).is_none());
    assert!(WebAddon
        .dispatch(&argv(&["--plain", "web"]), &addons)
        .is_none());
    // `claudio web --help` prints the usage and runs nothing.
    assert_eq!(
        WebAddon.dispatch(&argv(&["web", "--help"]), &addons),
        Some(ExitCode::SUCCESS)
    );
}
