//! `app::env` against real files in a scratch dir.

use std::os::unix::fs::PermissionsExt;

use super::*;

/// A scratch dir with `config.toml` holding `text`, and its store.
fn scratch(text: &str) -> (PathBuf, Store) {
    let dir = std::env::temp_dir().join(format!("claudio-env-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), text).unwrap();
    let store = Store::beside(&dir.join("config.toml"));
    (dir, store)
}

fn sid(n: u128) -> SessionId {
    SessionId::from_u128(n)
}

fn session(host: &str, n: u128) -> Scope {
    Scope::Session {
        host: host.to_owned(),
        sid: sid(n),
    }
}

fn v(text: &str) -> Value {
    Value::new(text)
}

fn names(listed: &Listed) -> Vec<(&str, &str)> {
    listed
        .vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn global_set_and_unset_round_trip_keeping_other_sections_and_comments() {
    let text = "# my settings\n[ui]\nnotify = true # yes\n\n[env]\n# models\nA = \"1\"\n";
    let (dir, store) = scratch(text);
    store
        .set(&Scope::Global, "BASH_DEFAULT_TIMEOUT_MS", &v("300000"))
        .unwrap();
    store.set(&Scope::Global, "A", &v("2")).unwrap();
    let out = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert_eq!(
        out,
        "# my settings\n[ui]\nnotify = true # yes\n\n[env]\n# models\nA = \"2\"\n\
         BASH_DEFAULT_TIMEOUT_MS = \"300000\"\n"
    );
    let listed = store.list(&Scope::Global);
    assert_eq!(
        names(&listed),
        [("A", "2"), ("BASH_DEFAULT_TIMEOUT_MS", "300000")]
    );
    assert_eq!((listed.skipped, listed.notices.len()), (0, 0));
    store.unset(&Scope::Global, "A").unwrap();
    store.unset(&Scope::Global, "NOT_THERE").unwrap();
    let out = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert_eq!(
        out,
        "# my settings\n[ui]\nnotify = true # yes\n\n[env]\n# models\n\
         BASH_DEFAULT_TIMEOUT_MS = \"300000\"\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_global_value_with_a_newline_stays_on_one_line_through_set_change_and_unset() {
    let (dir, store) = scratch("[claude]\ninstall = \"off\"\n");
    let headers = "X-One: 1\nX-Two: 2";
    store
        .set(&Scope::Global, "ANTHROPIC_CUSTOM_HEADERS", &v(headers))
        .unwrap();
    let out = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert!(
        out.ends_with("ANTHROPIC_CUSTOM_HEADERS = \"X-One: 1\\nX-Two: 2\"\n"),
        "{out}"
    );
    assert_eq!(out.lines().count(), 5, "{out}");
    assert_eq!(
        names(&store.list(&Scope::Global)),
        [("ANTHROPIC_CUSTOM_HEADERS", headers)]
    );
    store
        .set(&Scope::Global, "ANTHROPIC_CUSTOM_HEADERS", &v("X: 3\n\tY"))
        .unwrap();
    assert_eq!(
        names(&store.list(&Scope::Global)),
        [("ANTHROPIC_CUSTOM_HEADERS", "X: 3\n\tY")]
    );
    store
        .unset(&Scope::Global, "ANTHROPIC_CUSTOM_HEADERS")
        .unwrap();
    assert!(store.list(&Scope::Global).vars.is_empty());
    let out = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert_eq!(out, "[claude]\ninstall = \"off\"\n\n[env]\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_multi_line_toml_string_is_listed_changed_and_removed() {
    // What a whole-file save by another writer (or a hand edit) leaves.
    let text = "[env]\nH = \"\"\"\nline\n\"\"\"\nK = \"k\"\n";
    let (dir, store) = scratch(text);
    assert_eq!(
        names(&store.list(&Scope::Global)),
        [("H", "line\n"), ("K", "k")]
    );
    store.set(&Scope::Global, "H", &v("x\ny")).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("config.toml")).unwrap(),
        "[env]\nH = \"x\\ny\"\nK = \"k\"\n"
    );
    std::fs::write(dir.join("config.toml"), text).unwrap();
    store.unset(&Scope::Global, "H").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("config.toml")).unwrap(),
        "[env]\nK = \"k\"\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_global_entry_is_skipped_with_one_notice_and_the_config_still_loads() {
    let text = "[ui]\nnotify = true\n[env]\nX = 1\nPATH = \"/bin\"\n\"1BAD\" = \"v\"\nCTL = \"a\\u0000b\"\nOK = \"yes\"\n";
    let (dir, store) = scratch(text);
    let listed = store.list(&Scope::Global);
    assert_eq!(names(&listed), [("OK", "yes")]);
    assert_eq!(listed.skipped, 4);
    assert_eq!(
        listed.notices,
        ["[env]: 4 skipped: CTL (bad value), PATH (refused), X (not a string), 1 with a bad name"]
    );
    assert!(
        !listed.notices[0].contains("1BAD"),
        "a bad key is never quoted"
    );
    let config = crate::config::load_from(&dir.join("config.toml"));
    assert!(config.ui.notify, "the rest of config.toml still loads");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_set_unset_and_forget_in_a_private_file() {
    let (dir, store) = scratch("");
    let file = dir.join(SESSION_FILE);
    store.set(&session("local", 1), "A", &v("1")).unwrap();
    store
        .set(&session("local", 1), "B", &v("two\nlines"))
        .unwrap();
    store.set(&session("box", 2), "A", &v("x")).unwrap();
    assert_eq!(mode(&file), 0o600);
    assert_eq!(
        names(&store.list(&session("local", 1))),
        [("A", "1"), ("B", "two\nlines")]
    );
    // Lookups are by session id: the host is kept for pruning only.
    assert_eq!(
        names(&store.list(&session("box", 1))),
        [("A", "1"), ("B", "two\nlines")]
    );
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["sessions"][sid(1).to_string()]["host"], "local");
    store.unset(&session("local", 1), "A").unwrap();
    assert_eq!(
        names(&store.list(&session("local", 1))),
        [("B", "two\nlines")]
    );
    store.forget_session("local", sid(1)).unwrap();
    assert!(store.list(&session("local", 1)).vars.is_empty());
    assert_eq!(names(&store.list(&session("box", 2))), [("A", "x")]);
    // The last var of a session takes its entry with it.
    store.unset(&session("box", 2), "A").unwrap();
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(json["sessions"], serde_json::json!({}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn effective_overlays_the_session_on_the_global_scope() {
    let (dir, store) = scratch("[env]\nA = \"global\"\nB = \"global\"\n");
    store.set(&session("local", 1), "B", &v("mine")).unwrap();
    store.set(&session("local", 1), "C", &v("mine")).unwrap();
    let (vars, notices) = store.effective("local", sid(1));
    let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(vars, [("A", "global"), ("B", "mine"), ("C", "mine")]);
    assert!(notices.is_empty());
    let (other, _) = store.effective("local", sid(2));
    assert_eq!(other.len(), 2, "a new session gets the global scope");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_or_oversize_session_file_reads_empty_with_a_notice_and_refuses_writes() {
    for bad in [
        b"not json".to_vec(),
        br#"{"version":2,"sessions":{}}"#.to_vec(),
        vec![b' '; MAX_SESSION_FILE + 1],
    ] {
        let (dir, store) = scratch("[env]\nA = \"1\"\n");
        let file = dir.join(SESSION_FILE);
        std::fs::write(&file, &bad).unwrap();
        let listed = store.list(&session("local", 1));
        assert!(listed.vars.is_empty());
        assert_eq!(listed.notices, [UNREADABLE]);
        let (vars, notices) = store.effective("local", sid(1));
        assert_eq!(vars.len(), 1, "the global scope still applies");
        assert_eq!(notices, [UNREADABLE]);
        let err = store.set(&session("local", 1), "B", &v("2")).unwrap_err();
        assert_eq!(err.to_string(), UNREADABLE);
        assert!(store.unset(&session("local", 1), "B").is_err());
        assert!(store.forget_session("local", sid(1)).is_err());
        assert!(store.prune("local", &[]).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), bad, "never overwritten");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn an_unreadable_session_file_is_left_untouched() {
    let (dir, store) = scratch("");
    let file = dir.join(SESSION_FILE);
    std::fs::write(&file, br#"{"version":1,"sessions":{}}"#).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    // root reads anything: the check is meaningful for a normal user only.
    if std::fs::read(&file).is_err() {
        assert_eq!(store.list(&session("local", 1)).notices, [UNREADABLE]);
        assert!(store.set(&session("local", 1), "A", &v("1")).is_err());
        assert_eq!(mode(&file), 0o000);
    }
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prune_drops_the_hosts_gone_sessions_and_keeps_other_hosts() {
    let (dir, store) = scratch("");
    store.set(&session("local", 1), "A", &v("1")).unwrap();
    store.set(&session("local", 2), "A", &v("2")).unwrap();
    store.set(&session("box", 3), "A", &v("3")).unwrap();
    store.prune("local", &[sid(2), sid(9)]).unwrap();
    assert!(store.list(&session("local", 1)).vars.is_empty());
    assert_eq!(store.list(&session("local", 2)).vars.len(), 1);
    assert_eq!(store.list(&session("box", 3)).vars.len(), 1);
    // A missing file is never created by a prune or a forget.
    let (dir2, store2) = scratch("");
    store2.prune("local", &[]).unwrap();
    store2.forget_session("local", sid(1)).unwrap();
    assert!(!dir2.join(SESSION_FILE).exists());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

#[test]
fn two_writers_under_the_lock_lose_no_update() {
    let (dir, store) = scratch("");
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let store = store.clone();
            std::thread::spawn(move || {
                for k in 0..5 {
                    let scope = session("local", 1 + (i % 2));
                    store.set(&scope, &format!("V{i}_{k}"), &v("x")).unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let total =
        store.list(&session("local", 1)).vars.len() + store.list(&session("local", 2)).vars.len();
    assert_eq!(total, 40, "no write lost");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn writes_are_checked_and_capped() {
    let (dir, store) = scratch("");
    for scope in [Scope::Global, session("local", 1)] {
        let err = store.set(&scope, "PATH", &v("/x")).unwrap_err();
        assert!(err.to_string().starts_with("PATH: refused: "), "{err}");
        let err = store.set(&scope, "TOKEN", &v("sk-SECRET\r")).unwrap_err();
        assert!(err.to_string().starts_with("TOKEN: value holds"), "{err}");
        assert!(!err.to_string().contains("SECRET"));
        let err = store.set(&scope, "sk-SECRET", &v("x")).unwrap_err();
        assert!(!err.to_string().contains("SECRET"), "{err}");
        for i in 0..MAX_VARS {
            store.set(&scope, &format!("V{i}"), &v("x")).unwrap();
        }
        // Replacing one is fine; a new one is past the cap.
        store.set(&scope, "V0", &v("y")).unwrap();
        let err = store.set(&scope, "NEW", &v("x")).unwrap_err();
        assert!(err.to_string().starts_with("too many variables"), "{err}");
        for i in 0..MAX_VARS {
            store.unset(&scope, &format!("V{i}")).unwrap();
        }
        let big = "x".repeat(8000);
        for i in 0..16 {
            store.set(&scope, &format!("BIG{i:02}"), &v(&big)).unwrap();
        }
        let err = store.set(&scope, "BIG16", &v(&big)).unwrap_err();
        assert!(
            err.to_string().starts_with("environment too large"),
            "{err}"
        );
        for i in 0..16 {
            store.unset(&scope, &format!("BIG{i:02}")).unwrap();
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_hand_edited_scope_past_a_cap_keeps_the_first_entries_in_name_order() {
    let mut text = String::from("[env]\n");
    for i in 0..(MAX_VARS + 3) {
        text.push_str(&format!("V{i:03} = \"x\"\n"));
    }
    let (dir, store) = scratch(&text);
    let listed = store.list(&Scope::Global);
    assert_eq!(listed.vars.len(), MAX_VARS);
    assert_eq!(listed.vars[0].0, "V000");
    assert_eq!(listed.skipped, 3);
    assert_eq!(
        listed.notices,
        ["[env]: 3 skipped: 3 past the caps (128 variables, 128 KiB)"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_value_reaches_debug() {
    let (dir, store) = scratch("[env]\nTOKEN = \"sk-SECRET-1\"\n");
    let listed = store.list(&Scope::Global);
    let shown = format!(
        "{listed:?} {store:?} {:?}",
        store.effective("local", sid(1))
    );
    assert!(!shown.contains("SECRET"), "{shown}");
    let _ = std::fs::remove_dir_all(&dir);
}
