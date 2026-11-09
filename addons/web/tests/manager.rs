//! The session manager with the web addon (features: web): what a TUI
//! session gets from the web addon, its `latex` and `diagram` extension rows
//! and its lenses' instructions (with an external extension's beside them).
//! It shares the manager suite's PTY harness (`common`).
//!
//! Run with:
//!   cargo test --test web_manager -- --test-threads=1

#[path = "../../../crates/tui/tests/common/mod.rs"]
mod common;

use std::time::{Duration, Instant};
use std::{fs, thread};

use common::{
    encode_cwd, wizard_pick_dir, ManagerHarness, Region, ALT_I, ALT_N, ALT_Q, ALT_U, ALT_Z,
    DAEMON_WAIT, DOWN_ARROW, ENTER, ESC, WAIT,
};

// ── LaTeX formulas ────────────────────────────────────────────────────────────

/// Tab, as the terminal sends it.
const TAB: &[u8] = b"\t";

/// A fake claude that answers with a formula, as real claude does by
/// default: it saves its `--settings`, writes the assistant record to its
/// transcript (under the test's `$HOME`), prints the answer with the tag,
/// then runs its own SessionStart hook so the daemon reads it.
fn write_latex_fake_claude(h: &ManagerHarness, settings: &std::path::Path) {
    let sid = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    // claude (and now the daemon) file the transcript under the resolved cwd.
    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
    let project = h.home.join(".claude/projects").join(encode_cwd(&cwd));
    let record = serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "m1",
            "model": "claude-fake",
            "content": [{"type": "text", "text": "Energy: <latex>E = mc^2</latex> as promised."}],
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
    });
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "claude 0.0.0-fake"; exit 0; fi
if [ "$1" = "--help" ]; then exit 0; fi
printf '%s' "$2" > "{settings}"
mkdir -p "{project}"
printf '%s\n' '{record}' > "{project}/{sid}.jsonl"
echo "FAKE_CLAUDE_BANNER pid=$$"
echo 'Energy: <latex>E = mc^2</latex> as promised.'
HOOK=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart[^"]*\)".*/\1/p')
printf '{{"session_id":"{sid}","source":"startup"}}' | eval "$HOOK"
exec cat
"#,
        settings = settings.display(),
        project = project.display(),
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(
        &h.fake_claude,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
}

/// G5: a formula in claude's answer stays in the pane as claude printed
/// it, `<latex>` tags and all (formulas render in the web UI only), also
/// after the daemon has sent its formula list and in a TUI started later
/// (which gets the list replayed on attach). The reader has no formulas
/// list any more: Tab changes nothing. The default config still gives the
/// session the context-file SessionStart hook (the web UI needs the tags).
#[test]
fn test_latex_formula_is_shown_as_claude_printed_it() {
    const PRINTED: &str = "Energy: <latex>E = mc^2</latex> as promised.";
    let h = ManagerHarness::new();
    // The default test config has no `latex` key: the hook is on by default.
    let settings = h.root.join("settings.json");
    write_latex_fake_claude(&h, &settings);

    {
        let mut tui = h.start_tui();
        wizard_pick_dir(&mut tui, &h.dirs[0]);
        tui.wait_for(PRINTED, Region::Pane, DAEMON_WAIT);
        let hooks = fs::read_to_string(&settings).unwrap();
        assert!(hooks.contains("__context file"), "{hooks}");
        // The daemon read the transcript (its formula list is sent by
        // now): the pane is unchanged.
        thread::sleep(Duration::from_millis(1500));
        assert!(tui.screen_text(Region::Pane).contains(PRINTED));
        assert!(!tui.screen_text(Region::Pane).contains("mc²"));

        // The transcript has no prompt, so no answer; Tab lists nothing.
        tui.send_keys(ALT_U);
        tui.wait_for("No answers yet", Region::Pane, WAIT);
        tui.send_keys(TAB);
        thread::sleep(Duration::from_millis(300));
        let pane = tui.screen_text(Region::Pane);
        assert!(pane.contains("No answers yet"), "{pane}");
        assert!(!pane.contains("formulas"), "{pane}");
        tui.send_keys(ALT_U);
        tui.wait_for(PRINTED, Region::Pane, WAIT);
        tui.send_keys(ALT_Q);
        tui.wait_exit(WAIT);
    }

    // A new TUI gets the daemon's replay on attach: still claude's text.
    let tui = h.start_tui();
    tui.wait_for(PRINTED, Region::Pane, DAEMON_WAIT);
    thread::sleep(Duration::from_millis(1000));
    assert!(tui.screen_text(Region::Pane).contains(PRINTED));
}

/// The external extension the extension tests install: a tag lens `keys`
/// drawn as `⌨ <source>`, and actions that paste (with the API key it
/// sees, which must be none), list, show a document, and hang.
const FAKE_EXTENSION: &str = r#"name = "fake"
description = "a fake extension"
command = ["./fake.sh"]
[[lens]]
tag = "keys"
instruction = "Write keys as <keys>Ctrl+C</keys>. EXT-INSTRUCTION"
[[action]]
key = "k"
title = "paste a stamp"
id = "stamp"
[[action]]
key = "l"
title = "pick one"
id = "pick"
[[action]]
key = "d"
title = "show a document"
id = "doc"
[[action]]
key = "h"
title = "hang"
id = "hang"
"#;

const FAKE_EXTENSION_PROGRAM: &str = r##"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed 's/^{"id":\([0-9]*\).*/\1/')
  case "$line" in
    *'"method":"render"'*)
      src=$(printf '%s' "$line" | sed 's/.*"source":"\([^"]*\)".*/\1/')
      printf '{"id":%s,"result":{"inline":"⌨ %s"}}\n' "$id" "$src" ;;
    *'"action":"stamp"'*)
      printf '{"id":%s,"result":{"kind":"paste","text":"PASTED-%s"}}\n' "$id" "${ANTHROPIC_API_KEY:-nokey}" ;;
    *'"action":"pick"'*)
      printf '{"id":%s,"result":{"kind":"list","title":"Fake list","items":[{"label":"first","result":{"kind":"paste","text":"PICKED-FIRST"}},{"label":"second","detail":"the other one","result":{"kind":"paste","text":"PICKED-SECOND"}}]}}\n' "$id" ;;
    *'"action":"doc"'*)
      printf '{"id":%s,"result":{"kind":"document","title":"Fake doc","markdown":"# Heading\\n\\nPress <keys>Esc</keys> now."}}\n' "$id" ;;
    *) ;;
  esac
done
"##;

/// Install the fake extension in the harness's config dir.
fn install_fake_extension(h: &ManagerHarness) {
    let dir = h.config_home.join("claudio/extensions/fake");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("extension.toml"), FAKE_EXTENSION).unwrap();
    let program = dir.join("fake.sh");
    fs::write(&program, FAKE_EXTENSION_PROGRAM).unwrap();
    fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
}

/// A fake claude whose answer has a `<keys>` tag: it saves its `--settings`
/// and argv, writes the transcript (a prompt and the answer), prints the
/// answer, turns bracketed paste on, runs its SessionStart hook, then
/// echoes lines; a line ending in `off` turns bracketed paste off.
fn write_keys_fake_claude(h: &ManagerHarness, settings: &std::path::Path, argv: &std::path::Path) {
    let sid = "cccccccc-dddd-4eee-8fff-000000000000";
    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
    let project = h.home.join(".claude/projects").join(encode_cwd(&cwd));
    let prompt = serde_json::json!({"type": "user", "message": {"role": "user", "content": "How do I stop it?"}});
    let answer = serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "m1",
            "model": "claude-fake",
            "content": [{"type": "text", "text": "Press <keys>Ctrl+C</keys> to stop."}],
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
    });
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "claude 0.0.0-fake"; exit 0; fi
if [ "$1" = "--help" ]; then exit 0; fi
printf '%s' "$2" > "{settings}"
printf '%s\n' "$@" > "{argv}"
mkdir -p "{project}"
printf '%s\n' '{prompt}' '{answer}' > "{project}/{sid}.jsonl"
echo "FAKE_CLAUDE_BANNER pid=$$"
echo 'Press <keys>Ctrl+C</keys> to stop.'
printf '\033[?2004h'
HOOK=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart[^"]*\)".*/\1/p')
printf '{{"session_id":"{sid}","source":"startup"}}' | eval "$HOOK"
while IFS= read -r line; do
  case "$line" in
    *off) printf '\033[?2004l' ;;
    *) printf 'GOT %s\n' "$line" ;;
  esac
done
"#,
        settings = settings.display(),
        argv = argv.display(),
        project = project.display(),
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(
        &h.fake_claude,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
}

/// AC2, AC6, AC7: an external extension's tag lens leaves the pane and the
/// reader alone (G5: lenses render in the web UI only, the tag text is
/// shown unchanged); its instruction reaches claude through the context
/// file only; Alt+z lists the built-in and external actions, Esc closes it
/// with nothing sent, Alt+z u is the reader; the program's paste reaches
/// claude without Enter (and is refused once bracketed paste is off), its
/// list picks, its document shows (its own tag as written), and a hanging
/// action times out with a notice.
#[test]
fn test_external_extension_lens_leader_and_actions() {
    let h = ManagerHarness::new();
    install_fake_extension(&h);
    let (settings, argv) = (h.root.join("settings.json"), h.root.join("argv.txt"));
    write_keys_fake_claude(&h, &settings, &argv);
    let mut tui = h.start_tui();
    wizard_pick_dir(&mut tui, &h.dirs[0]);

    // G5: the pane shows the tag text unchanged, also once the daemon has
    // sent its lens list.
    tui.wait_for(
        "Press <keys>Ctrl+C</keys> to stop.",
        Region::Pane,
        DAEMON_WAIT,
    );
    thread::sleep(Duration::from_millis(1500));
    let pane = tui.screen_text(Region::Pane);
    assert!(
        pane.contains("Press <keys>Ctrl+C</keys> to stop."),
        "{pane}"
    );
    assert!(!pane.contains('⌨'), "{pane}");

    // AC2: the hook has the context file's path; the text is only in it.
    let hooks = fs::read_to_string(&settings).unwrap();
    assert!(hooks.contains("__context file"), "{hooks}");
    assert!(!hooks.contains("EXT-INSTRUCTION"), "{hooks}");
    assert!(!fs::read_to_string(&argv)
        .unwrap()
        .contains("EXT-INSTRUCTION"));
    let context_dir = h.runtime_dir.join("claudio/context");
    let context: Vec<String> = fs::read_dir(&context_dir)
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    assert_eq!(context.len(), 1);
    assert!(context[0].contains("EXT-INSTRUCTION"), "{context:?}");
    assert!(context[0].contains("<latex>"), "the built-in lenses too");

    // G5: the reader shows it as written too.
    tui.send_keys(ALT_U);
    tui.wait_for("answer 1/1 · How do I stop it?", Region::Pane, DAEMON_WAIT);
    tui.wait_for("Press <keys>Ctrl+C</keys> to stop.", Region::Pane, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("answer 1/1", Region::Pane), WAIT);

    // AC6: the leader lists built-in and external actions; Esc sends nothing.
    tui.send_keys(ALT_Z);
    tui.wait_for("search conversations", Region::Screen, WAIT);
    let shown = tui.screen_text(Region::Screen);
    for row in [
        "u  reader view",
        "p  insert snippet",
        "k  paste a stamp  (fake)",
    ] {
        assert!(shown.contains(row), "missing {row:?} in:\n{shown}");
    }
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("paste a stamp", Region::Screen), WAIT);
    thread::sleep(Duration::from_millis(300));
    assert!(
        !tui.screen_text(Region::Pane).contains("^["),
        "nothing reached claude"
    );
    tui.send_keys(ALT_Z);
    tui.wait_for("paste a stamp", Region::Screen, WAIT);
    tui.send_keys(b"u");
    tui.wait_for("answer 1/1 · How do I stop it?", Region::Pane, DAEMON_WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("answer 1/1", Region::Pane), WAIT);

    // AC7: a paste reaches claude without Enter (no `GOT` line), and no API
    // key reaches the program.
    tui.send_keys(ALT_Z);
    tui.wait_for("paste a stamp", Region::Screen, WAIT);
    tui.send_keys(b"k");
    tui.wait_for("PASTED-nokey", Region::Pane, WAIT);
    thread::sleep(Duration::from_millis(300));
    let pane = tui.screen_text(Region::Pane);
    assert_eq!(pane.matches("PASTED-nokey").count(), 1, "{pane}");
    assert!(
        !pane.contains("GOT"),
        "no Enter followed the paste:\n{pane}"
    );

    // A list: Down, Enter pastes the second item's text.
    tui.send_keys(ALT_Z);
    tui.wait_for("pick one", Region::Screen, WAIT);
    tui.send_keys(b"l");
    tui.wait_for("Fake list", Region::Screen, WAIT);
    tui.wait_for("the other one", Region::Screen, WAIT);
    tui.send_keys(DOWN_ARROW);
    tui.send_keys(ENTER);
    tui.wait_for("PICKED-SECOND", Region::Pane, WAIT);

    // A document, its own tag as written.
    tui.send_keys(ALT_Z);
    tui.wait_for("show a document", Region::Screen, WAIT);
    tui.send_keys(b"d");
    tui.wait_for("Fake doc", Region::Pane, WAIT);
    tui.wait_for("Press <keys>Esc</keys> now.", Region::Pane, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("Fake doc", Region::Pane), WAIT);

    // Bracketed paste off: the paste is refused with a notice.
    tui.send_keys(b"off\r");
    thread::sleep(Duration::from_millis(300));
    tui.send_keys(ALT_Z);
    tui.wait_for("paste a stamp", Region::Screen, WAIT);
    tui.send_keys(b"k");
    tui.wait_for("bracketed paste is off", Region::StatusBar, WAIT);

    // A hanging action times out (15 s) with a notice.
    tui.send_keys(ALT_Z);
    tui.wait_for("hang", Region::Screen, WAIT);
    tui.send_keys(b"h");
    tui.wait_for(
        "extension fake timed out",
        Region::StatusBar,
        Duration::from_secs(25),
    );
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}

/// The extensions screen (Alt+i) lists the built-ins and a loaded external
/// extension with their keys and lenses; Space turns the selected one off
/// at once (`[ ]`) and saves it in config.toml, the file's comment kept;
/// Esc closes it; the leader no longer offers a turned-off extension's
/// actions.
#[test]
fn test_extensions_screen_turns_extensions_off_and_saves_it() {
    let h = ManagerHarness::new();
    install_fake_extension(&h);
    let config = h.config_home.join("claudio/config.toml");
    let prior = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("# my settings, keep me\n{prior}")).unwrap();
    let mut tui = h.start_tui();
    wizard_pick_dir(&mut tui, &h.dirs[0]);

    tui.send_keys(ALT_I);
    tui.wait_for(
        "Extensions (Space toggle · Enter plugin setup · r refresh · Esc close)",
        Region::Screen,
        WAIT,
    );
    // The built-ins, the external one, then the web addon's rows (P3.18:
    // latex and diagram are the web addon's, so they follow and have no
    // lens column of their own).
    tui.wait_for("[x] diagram   web", Region::Screen, WAIT);
    let shown = tui.screen_text(Region::Screen);
    for row in [
        "[x] search    built-in  Alt+z s",
        "[x] snippets  built-in  Alt+z p/P",
        "[x] fake      external  Alt+z k/l/d/h  <keys>",
        "[x] latex     web       —",
        "[x] diagram   web       —",
        "search claude conversations",
        "changes apply now",
    ] {
        assert!(shown.contains(row), "missing {row:?} in:\n{shown}");
    }

    // Space on diagram: off now, and saved.
    for _ in 0..4 {
        tui.send_keys(DOWN_ARROW);
    }
    tui.wait_for("Mermaid diagrams drawn in the web UI", Region::Screen, WAIT);
    tui.send_keys(b" ");
    tui.wait_for("[ ] diagram", Region::Screen, WAIT);
    tui.wait_for("extension diagram off (saved)", Region::StatusBar, WAIT);
    let saved = fs::read_to_string(&config).unwrap();
    assert!(saved.starts_with("# my settings, keep me\n"), "{saved}");
    assert!(saved.contains(&prior), "the rest is kept:\n{saved}");
    assert!(
        saved.ends_with("[extensions]\ndiagram = false\n"),
        "{saved}"
    );

    // Snippets off: its keys leave the leader.
    tui.send_keys(b"\x1b[H"); // Home
    tui.send_keys(DOWN_ARROW);
    tui.wait_for("saved prompts", Region::Screen, WAIT);
    tui.send_keys(b" ");
    tui.wait_for("[ ] snippets", Region::Screen, WAIT);
    tui.wait_for("extension snippets off (saved)", Region::StatusBar, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);
    tui.send_keys(ALT_Z);
    tui.wait_for("search conversations", Region::Screen, WAIT);
    let leader = tui.screen_text(Region::Screen);
    assert!(leader.contains("paste a stamp"), "{leader}");
    assert!(leader.contains("e  manage extensions"), "{leader}");
    assert!(
        !leader.contains("insert snippet"),
        "snippets is off:\n{leader}"
    );
    let saved = fs::read_to_string(&config).unwrap();
    assert!(
        saved.ends_with("[extensions]\ndiagram = false\nsnippets = false\n"),
        "{saved}"
    );

    // `e` opens the screen too.
    tui.send_keys(b"e");
    tui.wait_for(
        "Extensions (Space toggle · Enter plugin setup · r refresh · Esc close)",
        Region::Screen,
        WAIT,
    );
    tui.send_keys(ALT_I);
    tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}

/// The lens names of the journaled session in `dir`, once it has some.
fn journaled_lenses(h: &ManagerHarness, dir: &std::path::Path) -> Vec<String> {
    let dir = dir.to_str().unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        let entry = h
            .journal_sessions()
            .into_iter()
            .find(|e| e["cwd"] == dir && e["extensions"].is_object());
        if let Some(entry) = entry {
            return entry["extensions"]["lenses"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["name"].as_str().unwrap().to_owned())
                .collect();
        }
        assert!(Instant::now() < deadline, "{:?}", h.journal_sessions());
        thread::sleep(Duration::from_millis(100));
    }
}

/// AC6: a session started from the TUI asks the daemon for the web addon's
/// `latex` and `mermaid` lenses (the web UI renders them); turning `latex`
/// off on the extensions screen drops it for the next session.
#[test]
fn test_tui_session_requests_the_latex_lens_from_the_web_addon() {
    let h = ManagerHarness::new();
    let mut tui = h.start_tui();
    wizard_pick_dir(&mut tui, &h.dirs[0]);
    assert_eq!(journaled_lenses(&h, &h.dirs[0]), ["latex", "mermaid"]);

    tui.send_keys(ALT_I);
    tui.wait_for("[x] latex     web", Region::Screen, WAIT);
    tui.send_keys(DOWN_ARROW);
    tui.send_keys(DOWN_ARROW);
    tui.wait_for("math rendered from <latex> tags", Region::Screen, WAIT);
    tui.send_keys(b" ");
    tui.wait_for("[ ] latex", Region::Screen, WAIT);
    tui.wait_for("extension latex off (saved)", Region::StatusBar, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);

    tui.send_keys(ALT_N);
    wizard_pick_dir(&mut tui, &h.dirs[1]);
    assert_eq!(journaled_lenses(&h, &h.dirs[1]), ["mermaid"]);
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}
