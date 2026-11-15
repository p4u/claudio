//! The TUI's edge of the custom env (`app::env`): it runs `App`'s
//! [`EnvCall`]s off the UI thread, on the files beside the addon runtime's
//! config.toml, and sends what a read or a write found back as
//! [`HostEvent::Env`]. A forget's or a prune's failure is not shown.

use std::path::Path;

use tokio::sync::mpsc;

use super::env_view::{EnvCall, EnvDone};
use super::HostEvent;
use claudio_core::app::env::{EnvError, Scope, Store};
use claudio_core::claude::env::shown;

/// Run `call` on the store beside `config_path` (each call one blocking
/// task, bounded by the files' lock wait).
pub(super) fn run(call: EnvCall, config_path: &Path, tx: &mpsc::Sender<HostEvent>) {
    let store = Store::beside(config_path);
    let tx = tx.clone();
    tokio::spawn(async move {
        let done = tokio::task::spawn_blocking(move || apply(&store, call)).await;
        if let Ok(Some(done)) = done {
            let _ = tx.send(HostEvent::Env(done)).await;
        }
    });
}

/// `call` on `store`: what a read or a write found (a forget or a prune
/// reports nothing).
fn apply(store: &Store, call: EnvCall) -> Option<EnvDone> {
    match call {
        EnvCall::Read { gen, session } => Some(read(store, gen, session.as_ref(), None)),
        EnvCall::Write {
            gen,
            scope,
            name,
            value,
            session,
        } => {
            let written = match &value {
                Some(value) => store.set(&scope, &name, value),
                None => store.unset(&scope, &name),
            };
            let failed = written.err().map(|e| match e {
                EnvError::Refused(why) => why,
                e => shown(&name, &e),
            });
            Some(read(store, gen, session.as_ref(), failed))
        }
        EnvCall::Forget { host, sid } => {
            let _ = store.forget_session(&host, sid);
            None
        }
        EnvCall::Prune { host, live } => {
            let _ = store.prune(&host, &live);
            None
        }
    }
}

/// Both scopes as they are now.
fn read(store: &Store, gen: u64, session: Option<&Scope>, failed: Option<String>) -> EnvDone {
    EnvDone {
        gen,
        global: store.list(&Scope::Global),
        session: session.map(|scope| store.list(scope)),
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claudio_core::claude::env::Value;
    use claudio_core::proto::SessionId;

    fn scratch() -> (std::path::PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!("claudio-env-edge-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::beside(&dir.join("config.toml"));
        (dir, store)
    }

    #[test]
    fn a_write_reads_both_scopes_again_and_a_refusal_names_the_key_only() {
        let (dir, store) = scratch();
        let session = Scope::Session {
            host: "local".into(),
            sid: SessionId::from_u128(1),
        };
        let set = |scope: Scope, name: &str, value: &str| EnvCall::Write {
            gen: 3,
            scope,
            name: name.into(),
            value: Some(Value::new(value)),
            session: Some(session.clone()),
        };
        let done = apply(&store, set(Scope::Global, "G", "1")).unwrap();
        assert_eq!(done.gen, 3);
        assert_eq!(done.global.vars, [("G".to_owned(), Value::new("1"))]);
        assert_eq!(done.session.map(|l| l.vars.len()), Some(0));
        let done = apply(&store, set(session.clone(), "S", "2")).unwrap();
        assert_eq!(done.session.unwrap().vars.len(), 1);
        let done = apply(&store, set(Scope::Global, "PATH", "sk-SECRET")).unwrap();
        let failed = done.failed.unwrap();
        assert!(failed.starts_with("PATH: refused: "), "{failed}");
        assert!(!failed.contains("SECRET"));
        let forget = EnvCall::Forget {
            host: "local".into(),
            sid: SessionId::from_u128(1),
        };
        assert!(apply(&store, forget).is_none());
        assert!(store.list(&session).vars.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
