# Autocompact window: implementation plan

Spec: inline below
Class: bounded
Repos and branches:
- /volumes/repos/claudio → fix/autocompact-window (base manager@71eebad; main and origin/main are the same commit)
- /volumes/repos/claude-proxy → fix/autocompact-window (base main@8dfe2f9)

claudio proxy sessions must stop setting `CLAUDE_CODE_AUTO_COMPACT_WINDOW` (today hardcoded to 1000000 in `session_env`). Verified against claude 2.1.312: the env var's value is always min-clamped to claude's *believed* model window, so it never raises a window; it only suppresses the honest "model isn't recognized, context assumed 200k" warning and locks the in-app /autocompact dialog. The believed window comes from claude's model DB or the `[1m]` suffix (pins 1M, works for unknown ids) — nothing else (CLAUDE_CODE_MAX_CONTEXT_TOKENS has no effect; context fields in the gateway's /v1/models are ignored). Therefore:
1. claudio: remove the env var from `session_env` (both gateway and first-party modes); update tests and docs.
2. claudio: the status-bar context gauge mirrors claude's believed window: a model id containing the `1m` token (e.g. `claude-opus-5-5[1m]`) → 1_000_000; anything else → the existing 200k default. The proxy model catalogue is NO LONGER consulted for the gauge (it keeps feeding the Alt+s proxy-stats Models page, so the fetch machinery stays).
3. claude-proxy: `/v1/claudio/config` (internal/claudioapi/claudioapi.go, env map around line 251) stops advertising `CLAUDE_CODE_AUTO_COMPACT_WINDOW`; update its test and any docs mentioning it.
Non-goal: making claude use >200k context on unrecognized models (claude-side limitation). Recording the real 480k window of the custom `claude-openai-qwen38-27b` model in the proxy's credential config is a post-merge ops step (see Merge and release), not code.

Acceptance criteria:
- AC1: `session_env` emits no `CLAUDE_CODE_AUTO_COMPACT_WINDOW` in gateway or first-party mode, with or without a proxy config. The order of every other pair is unchanged. `env_diff` does not scrub an inherited value.
- AC2: `context_window(model)` returns 1_000_000 when `model` has a whole `1m` token (case-insensitive, split on non-alphanumerics), else 200_000. A proxy catalogue in `App::proxy_status` does not change the rendered gauge.
- AC3: The catalogue still reaches the Alt+s Models page. The first-look prefetch (`request_models_once`) and its test stay.
- AC4: `GET /v1/claudio/config` has no `CLAUDE_CODE_AUTO_COMPACT_WINDOW` key in `env`.
- AC5: docs/manager-design.md and README.md describe the new behaviour.

## Global Constraints
- Commit convention: conventional commits, `type(scope): lowercase summary`. **claudio:** end the message with the trailer line `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`, which is the repo's convention in its last two commits. **claude-proxy:** no trailer of any kind and no AI attribution.
- Never push. Never `git add -A`; commit with explicit paths (`git commit … -- <paths>`).
- The controller creates the branches before dispatch: `git -C /volumes/repos/claudio switch -c fix/autocompact-window 71eebad` and `git -C /volumes/repos/claude-proxy switch -c fix/autocompact-window 8dfe2f9`. Workers do not create, switch or delete branches.
- Build output goes in the repo's own `target/` (claudio) and Go's default cache (claude-proxy). Scratch work goes under `~/.cache/claudio/`. Never use /tmp.
- claudio tests: `make test-unit` (= `cargo test --locked --bin claudio`), or `cargo test --locked --bin claudio <name>` for one test. Run them with `umask 022`: under umask 077, `upgrade::tests::install_binary_replaces_file` fails on the base commit too (mode 700 ≠ 755). That failure is unrelated to this work.
- Manager suite: `make test-manager` (single-threaded, fake claude, no API key). Do not set or gate on `CLAUDIO_E2E`, `CLAUDIO_PROXY_TEST` or `CLAUDIO_SSH_TEST_HOST`.
- claude-proxy tests: `go test ./...`. Keep `gofmt -l internal/claudioapi/` empty.
- Linux and macOS must both compile (CI builds and unit-tests both). Nothing here is platform-specific.
- No repo-wide `cargo fmt`, because the tree is not rustfmt-clean with this toolchain. `src/proxy/env.rs` is rustfmt-clean, so `rustfmt --edition 2021 --check src/proxy/env.rs` must stay clean. `src/tui/ui/status.rs` and `src/tui/app.rs` are not, so match the surrounding style there and do not reformat them.
- Tokens never go in argv, logs, state files, commit messages or test output. Test fixtures use the literal `"tok"`.
- Manager and PTY tests assert on the rendered screen model, never on raw bytes.
- Behaviour that must stay unchanged:
  - every other `session_env` pair and its order;
  - the first-party omission of `CLAUDE_CODE_USE_GATEWAY` and `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`;
  - `env_diff`'s remove list;
  - the Alt+s stats popup, including its Models page;
  - the `1m` token rule (a date like `20250805` is not a window);
  - the rest of the proxy's `defaultEnv` and the `[1m]` default-model selection.

## Review Focus
- src/proxy/env.rs:48-102 `session_env`: exactly one push is removed (line 63). The pinned pair list in the gateway test and the key list in the first-party test lose only that entry.
- src/proxy/env.rs:130-139 `env_diff`: body unchanged (ruling: an inherited `CLAUDE_CODE_AUTO_COMPACT_WINDOW` passes through).
- src/proxy/env.rs:79-94: the config `env` map is read only for the four `ANTHROPIC_DEFAULT_*_MODEL` keys, so an old proxy that still advertises the var cannot re-inject it. The new test pins this.
- src/tui/ui/status.rs:335-357 `context_window`: no catalogue lookup remains. The token test is byte-for-byte the old one.
- src/tui/ui/status.rs:248-252 and 311-317: callers drop the `models` argument.
- src/tui/app.rs:508-515: `session_models` is removed. It had only one caller, the gauge, so keeping it would leave a dead_code warning. `request_models_once` (app.rs:497), `models_requested` (app.rs:231), `on_proxy_stats`'s retry (app.rs:427-430) and the test `a_proxied_session_fetches_the_model_catalogue_once` (app.rs:2021) stay; only their doc comments change.
- src/tui/ui/stats.rs:465 and 715-751 (Models page): untouched, and must still render the catalogue.
- claude-proxy internal/claudioapi/claudioapi.go:246-255 `defaultEnv`: only the one key is removed.

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 | none | claudio: src/proxy/env.rs, docs/manager-design.md | 3 |
| 2 | none (same repo and branch as 1, so it runs after 1) | claudio: src/tui/ui/status.rs, src/tui/app.rs, README.md | 3 |
| 3 | none | claude-proxy: internal/claudioapi/claudioapi.go, internal/claudioapi/claudioapi_test.go | 1, 2 |

## Tasks

### Task 1: stop injecting CLAUDE_CODE_AUTO_COMPACT_WINDOW
Model: sonnet   Repo: /volumes/repos/claudio   Files: src/proxy/env.rs, docs/manager-design.md
Goal: AC1 and the design-doc half of AC5.

Step 1: write the failing test. Append inside `mod tests` in src/proxy/env.rs, after `env_diff_keeps_gateway_vars_for_gateway_envs`:
```rust
    #[test]
    fn neither_mode_sets_the_auto_compact_window() {
        // claude clamps it to the window it believes the model has, so it
        // can only shrink that window, and it hides claude's "model not
        // recognized" notice.
        for p in [
            profile("https://x.net", "tok"),
            first_party_profile("https://x.net", "tok"),
        ] {
            let env = session_env(&p, None);
            assert!(env_get(&env, "CLAUDE_CODE_AUTO_COMPACT_WINDOW").is_none());
        }
        // An older proxy that still advertises it is not followed either.
        let mut cfg = ConfigResponse::default();
        cfg.env
            .insert("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), "1000000".into());
        let env = session_env(&profile("https://x.net", "tok"), Some(&cfg));
        assert!(env_get(&env, "CLAUDE_CODE_AUTO_COMPACT_WINDOW").is_none());
        // A user's own setting is inherited untouched (claude clamps it too).
        assert!(!env_diff(&env)
            .remove
            .contains(&"CLAUDE_CODE_AUTO_COMPACT_WINDOW"));
    }
```
Run: `umask 022; cargo test --locked --bin claudio neither_mode_sets_the_auto_compact_window`
Expected: `panicked at src/proxy/env.rs:…: assertion failed: env_get(&env, "CLAUDE_CODE_AUTO_COMPACT_WINDOW").is_none()` and `test result: FAILED. 0 passed; 1 failed`

Step 2: implement. Exact diff for src/proxy/env.rs:
```diff
@@ -45,6 +45,11 @@
 ///   non-Anthropic base URL, in case the proxy drops `tool_reference` blocks.
 ///   claude-proxy passes them through, so it stays on and every request
 ///   carries only the tools it needs.
+///
+/// `CLAUDE_CODE_AUTO_COMPACT_WINDOW` is not set in either mode. claude clamps
+/// it to the window it believes the model has, so it can only lower that
+/// window, and it hides claude's notice that an unrecognized model is assumed
+/// to have 200k. A model id with the `[1m]` suffix is what gives a session 1M.
 pub fn session_env(profile: &Profile, config: Option<&ConfigResponse>) -> Vec<(String, String)> {
@@ -60,7 +65,6 @@
         "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY".into(),
         "1".into(),
     ));
-    env.push(("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), "1000000".into()));
     env.push(("CLAUDE_CODE_GATEWAY_HINT_HEADERS".into(), "1".into()));
@@ -126,7 +130,8 @@
 /// would turn the feature flags off again. An inherited
 /// `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` is kept, since it may be set on
 /// purpose; it turns the feature flags off too, so it has to be unset by hand
-/// for first-party sessions.
+/// for first-party sessions. An inherited `CLAUDE_CODE_AUTO_COMPACT_WINDOW`
+/// is kept as well: claude clamps it to the model's window anyway.
 pub fn env_diff(env: &[(String, String)]) -> EnvDiff<'_> {
@@ -264,8 +269,8 @@
     #[test]
-    fn gateway_output_is_unchanged() {
-        // Must match what origin/main produces, pair for pair and in order.
+    fn gateway_env_is_exactly_these_pairs_in_order() {
+        // The whole gateway env, pair for pair and in order.
         let env = session_env(&profile("https://x.net", "tok"), None);
@@ -275,7 +280,6 @@
                 pair("ANTHROPIC_AUTH_TOKEN", "tok"),
                 pair("CLAUDE_CODE_USE_GATEWAY", "1"),
                 pair("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY", "1"),
-                pair("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "1000000"),
                 pair("CLAUDE_CODE_GATEWAY_HINT_HEADERS", "1"),
@@ -297,7 +301,6 @@
                 "ANTHROPIC_BASE_URL",
                 "ANTHROPIC_AUTH_TOKEN",
                 "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
-                "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
                 "CLAUDE_CODE_GATEWAY_HINT_HEADERS",
```
docs/manager-design.md, in "#### Env injected at spawn" (bullet at line 302, paragraph ending at line 315):
```diff
@@ -299,7 +299,6 @@
 - `ANTHROPIC_AUTH_TOKEN`
 - `CLAUDE_CODE_USE_GATEWAY=1`
 - `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`
-- `CLAUDE_CODE_AUTO_COMPACT_WINDOW`
 - `CLAUDE_CODE_GATEWAY_HINT_HEADERS=1`
@@ -314,6 +313,13 @@
 `CLAUDE_CODE_USE_GATEWAY` from a first-party env; an inherited
 `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` is kept.
 
+`CLAUDE_CODE_AUTO_COMPACT_WINDOW` is not set. claude clamps it to the window
+it believes the model has (its own model table, or 1M for a `[1m]` id), so it
+can never raise that window. All it does is hide claude's notice that an
+unrecognized model is assumed to have 200k, and lock `/autocompact`. A 1M
+session comes from a `[1m]` model id, which the proxy's model aliasing maps to
+the upstream model.
+
 `ANTHROPIC_API_KEY` is unset. Note that `/proc/<pid>/environ` is readable by
```
Run: `umask 022; cargo test --locked --bin claudio proxy::env && rustfmt --edition 2021 --check src/proxy/env.rs && make test-unit`
Expected: `test result: ok. 12 passed; 0 failed` for `proxy::env`, no rustfmt output, then `test result: ok. 783 passed; 0 failed` (the base has 782). Also `grep -rn AUTO_COMPACT src docs/manager-design.md README.md` lists only the new test, the two doc comments and the design-doc paragraph.

Step 3: commit
`git commit -m "fix(proxy): stop setting CLAUDE_CODE_AUTO_COMPACT_WINDOW" -m "claude clamps it to the window it believes the model has, so it never raised a window; it only hid the unrecognized-model notice and locked /autocompact." -m "Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>" -- src/proxy/env.rs docs/manager-design.md`

Interfaces this task provides to later tasks: none (`session_env` and `env_diff` signatures unchanged).

### Task 2: size the context gauge by the window claude believes
Model: sonnet   Repo: /volumes/repos/claudio   Files: src/tui/ui/status.rs, src/tui/app.rs, README.md
Goal: AC2, AC3 and the README half of AC5.

Step 1: write the failing test. In src/tui/ui/status.rs `claude_tab_shows_every_segment_when_wide` (lines 1015-1024), replace the catalogue block:
```rust
        // The proxy's catalogue does not move the gauge: claude compacts
        // against the window it believes the model has, not the catalogue's.
        let mut app = app;
        app.proxy_status.insert(
            "work".into(),
            ProxyStatus {
                models: Some(ModelsResponse {
                    data: vec![ModelEntry {
                        id: "claude-opus-5-5[1m]".into(),
                        max_input_tokens: 800_000,
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert!(render(&app, 200).contains("62% 620k/1M"), "{}", render(&app, 200));
```
Run: `umask 022; cargo test --locked --bin claudio claude_tab_shows_every_segment_when_wide`
Expected: a panic at that `assert!` whose message contains `77% 620k/800k`, and `test result: FAILED. 0 passed; 1 failed`

Step 2: implement. Exact diff for src/tui/ui/status.rs (non-test part):
```diff
@@ -19,7 +19,7 @@
 use crate::proto::{GitStatus, SessionKind, SessionState};
-use crate::proxy::api::{ModelsResponse, SessionCredential};
+use crate::proxy::api::SessionCredential;
@@ -45,8 +45,8 @@
-/// The context window assumed when neither the proxy's catalogue nor the
-/// model id says otherwise.
+/// The context window claude assumes for a model it does not know, and so
+/// the gauge's window for any model id without a `1m` token.
 const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
@@ -245,11 +245,7 @@
     if claude {
-        segments.extend(context_segment(
-            v.context_tokens,
-            v.model.as_deref(),
-            app.session_models(v),
-        ));
+        segments.extend(context_segment(v.context_tokens, v.model.as_deref()));
     }
@@ -308,13 +304,9 @@
-fn context_segment(
-    used: Option<u64>,
-    model: Option<&str>,
-    models: Option<&ModelsResponse>,
-) -> Option<Segment> {
+fn context_segment(used: Option<u64>, model: Option<&str>) -> Option<Segment> {
     let used = used?;
-    let window = context_window(model.unwrap_or_default(), models);
+    let window = context_window(model.unwrap_or_default());
@@ -332,28 +324,19 @@
-/// A model's context window: the proxy's catalogue entry for exactly this
-/// model, else 1M for a `[1m]`/`1m` model id, else the longest catalogue id
-/// it starts with (a dated id), else [`DEFAULT_CONTEXT_WINDOW`].
-pub fn context_window(model: &str, models: Option<&ModelsResponse>) -> u64 {
-    let entries = models
-        .map(|m| m.data.as_slice())
-        .unwrap_or_default()
-        .iter()
-        .filter(|e| e.max_input_tokens > 0 && !e.id.is_empty());
-    if let Some(entry) = entries.clone().find(|e| e.id == model) {
-        return entry.max_input_tokens as u64;
-    }
+/// The window claude compacts against: 1M for a model id with a `1m` token
+/// (`claude-opus-5-5[1m]`), else [`DEFAULT_CONTEXT_WINDOW`]. The proxy's
+/// catalogue is not consulted: claude ignores the context sizes in
+/// `/v1/models`, so a larger catalogue window is one it never fills.
+pub fn context_window(model: &str) -> u64 {
     let one_million = model
         .split(|c: char| !c.is_ascii_alphanumeric())
         .any(|part| part.eq_ignore_ascii_case("1m"));
     if one_million {
-        return 1_000_000;
+        1_000_000
+    } else {
+        DEFAULT_CONTEXT_WINDOW
     }
-    entries
-        .filter(|e| model.starts_with(&e.id))
-        .max_by_key(|e| e.id.len())
-        .map_or(DEFAULT_CONTEXT_WINDOW, |e| e.max_input_tokens as u64)
 }
```
Test-module changes in the same file:
```diff
-    use crate::proxy::api::{CredentialInfo, CredentialUtilization, ModelEntry};
+    use crate::proxy::api::{CredentialInfo, CredentialUtilization, ModelEntry, ModelsResponse};
```
Delete the `catalogue` helper (lines 809-821) and replace `context_window_prefers_the_catalogue_then_the_model_id_then_200k` (lines 823-849) with:
```rust
    #[test]
    fn context_window_is_1m_for_a_1m_token_else_200k() {
        assert_eq!(context_window("claude-opus-5-5[1m]"), 1_000_000);
        assert_eq!(context_window("claude-sonnet-4-5-1m"), 1_000_000);
        assert_eq!(context_window("CLAUDE-X-1M"), 1_000_000);
        // Without the suffix claude assumes 200k, whatever the upstream allows.
        assert_eq!(context_window("claude-opus-5-5"), 200_000);
        assert_eq!(context_window("claude-openai-qwen38-27b"), 200_000);
        // `1m` has to be a whole token: a date is not a window.
        assert_eq!(context_window("claude-opus-4-1-20250805"), 200_000);
        assert_eq!(context_window("claude-haiku-4-5"), 200_000);
        assert_eq!(context_window(""), 200_000);
    }
```
In `context_segment_is_a_colored_gauge_with_bold_percent` (lines 852-874), drop the trailing `, None` argument from all five `context_segment(...)` calls. Replace the comment `// Over the window (a different model than the catalogue thinks): capped.` with `// Over the window: the gauge is capped, the percentage is not.`

src/tui/app.rs:
```diff
@@ -24,9 +24,7 @@
 use crate::proto::{Msg, SessionId, SessionState};
-use crate::proxy::api::{
-    ConfigResponse, ModelsResponse, SessionCredential, SwitchError, SwitchOutcome,
-};
+use crate::proxy::api::{ConfigResponse, SessionCredential, SwitchError, SwitchOutcome};
@@ -225,9 +223,9 @@
-    /// Profiles whose model catalogue has been requested once for the status
-    /// bar's context window (a failed fetch is not retried; Alt+s fetches it
-    /// again anyway).
+    /// Profiles whose model catalogue has been requested once for the stats
+    /// popup's Models page (a failed fetch is asked again on the next look;
+    /// Alt+s fetches it again anyway).
     models_requested: HashSet<String>,
@@ -493,7 +491,7 @@
     /// Fetch a profile's model catalogue (no stats windows) the first time a
-    /// session on it is looked at, so the context gauge knows the window.
+    /// session on it is looked at, so the stats popup's Models page has it.
     fn request_models_once(&mut self, profile: &str) {
@@ -505,15 +503,6 @@
-    /// The model catalogue of a session's proxy, once fetched (on the first
-    /// look at a session on it, or by the stats popup); the status bar reads
-    /// context windows from it.
-    pub fn session_models(&self, v: &SessionView) -> Option<&ModelsResponse> {
-        self.proxy_status
-            .get(v.proxy.as_deref()?)
-            .and_then(|s| s.models.as_ref())
-    }
-
```
README.md line 162:
```diff
-- `ctx`: the last turn's context against the model's window (the proxy's model catalogue when it knows the model, else 1M for a `[1m]` model, else 200k).
+- `ctx`: the last turn's context against the window claude compacts at: 1M for a `[1m]` model, else 200k.
```
Run: `umask 022; cargo test --locked --bin claudio context_ && cargo test --locked --bin claudio a_proxied_session_fetches_the_model_catalogue_once && make test-unit 2>&1 | grep -E "^warning|test result"`
Expected: `test result: ok. 3 passed` (the two status tests plus `claude::projects::tests::cursor_takes_model_and_context_from_the_last_assistant_record`), then `ok. 1 passed`, then `test result: ok. 783 passed; 0 failed` with no `warning:` lines. `grep -rn "session_models\|catalogue(" src` returns nothing.

Step 3: commit
`git commit -m "fix(tui): size the context gauge by the window claude believes" -m "claude ignores the proxy catalogue's context sizes and compacts at 1M only for a [1m] model id, else 200k; the gauge now shows that window. The catalogue still feeds the Alt+s Models page." -m "Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>" -- src/tui/ui/status.rs src/tui/app.rs README.md`

Interfaces this task provides to later tasks: `pub fn context_window(model: &str) -> u64` in src/tui/ui/status.rs (was `(model: &str, models: Option<&ModelsResponse>)`). `App::session_models` is removed.

### Task 3: stop recommending CLAUDE_CODE_AUTO_COMPACT_WINDOW from the proxy
Model: haiku   Repo: /volumes/repos/claude-proxy   Files: internal/claudioapi/claudioapi.go, internal/claudioapi/claudioapi_test.go
Goal: AC4.

Step 1: write the failing test. In internal/claudioapi/claudioapi_test.go, insert before `// TestConfigNoDefaultsWhenCatalogueEmpty` (line 222):
```go
// TestConfigOmitsAutoCompactWindow verifies that the config does not
// recommend CLAUDE_CODE_AUTO_COMPACT_WINDOW (see defaultEnv).
func TestConfigOmitsAutoCompactWindow(t *testing.T) {
	cat := &fakeCatalogue{ok: true, entries: fakeEntries(), refreshedAt: time.Now()}
	mux, _ := setupHandler(t, cat)

	req := httptest.NewRequest(http.MethodGet, "/v1/claudio/config", nil)
	rw := httptest.NewRecorder()
	mux.ServeHTTP(rw, req)

	var resp struct {
		Env map[string]string `json:"env"`
	}
	if err := json.NewDecoder(rw.Body).Decode(&resp); err != nil {
		t.Fatal(err)
	}
	if v, present := resp.Env["CLAUDE_CODE_AUTO_COMPACT_WINDOW"]; present {
		t.Errorf("CLAUDE_CODE_AUTO_COMPACT_WINDOW should be absent, got %q", v)
	}
}

```
Run: `go test ./internal/claudioapi/ -run TestConfigOmitsAutoCompactWindow`
Expected: `claudioapi_test.go:241: CLAUDE_CODE_AUTO_COMPACT_WINDOW should be absent, got "1000000"` then `FAIL`

Step 2: implement. internal/claudioapi/claudioapi.go:
```diff
@@ -245,12 +245,16 @@
 // defaultEnv is the fixed set of env vars recommended for every claudio session.
 // The ANTHROPIC_DEFAULT_*_MODEL values are filled from the live catalogue.
+//
+// CLAUDE_CODE_AUTO_COMPACT_WINDOW is deliberately absent: Claude Code clamps it
+// to the window it believes the model has (its model table, or 1M for a "[1m]"
+// id), so it can never raise a window. It only hides the "model not recognized,
+// 200k assumed" notice and locks /autocompact.
 var defaultEnv = map[string]string{
 	"CLAUDE_CODE_USE_GATEWAY":                    "1",
 	"CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY": "1",
 	"CLAUDE_CODE_GATEWAY_HINT_HEADERS":           "1",
 	"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":   "1",
-	"CLAUDE_CODE_AUTO_COMPACT_WINDOW":            "1000000",
 	"API_TIMEOUT_MS":                             "30000",
 }
```
No doc changes are needed. No test pins the full `defaultEnv` map, and README.md, docs/ and CLAUDE.md never name the variable: CLAUDE.md:1069 only says "Recommended client env vars". The header comments of internal/proxy/models1m.go and CLAUDE.md:231-236 about the `[1m]` rows stay accurate.
Run: `gofmt -l internal/claudioapi/; go vet ./internal/claudioapi/ && go test ./...`
Expected: no gofmt output, then `ok  github.com/p4u/claude-proxy/internal/claudioapi` and `ok` for every package (none FAIL). `grep -rn AUTO_COMPACT --exclude-dir=.git .` lists only the new comment and test.

Step 3: commit
`git commit -m "fix(claudioapi): stop recommending CLAUDE_CODE_AUTO_COMPACT_WINDOW" -m "Claude Code clamps it to the window it believes the model has, so it never raised a window; it only hid the unrecognized-model notice and locked /autocompact." -- internal/claudioapi/claudioapi.go internal/claudioapi/claudioapi_test.go`

Interfaces this task provides to later tasks: none.

## Merge and release
1. Full suites, all run with `umask 022`:
   - claudio: `make test-unit`, expecting `ok. 783 passed; 0 failed` and no warnings.
   - claudio: `make test-manager`, expecting `ok. 43 passed; 0 failed`. A scratch run with Tasks 1 and 2 applied passed in about 2 minutes.
   - claudio: `make`, expecting a clean release build.
   - claude-proxy: `go test ./...`, expecting every package `ok`.
2. Merge order does not matter. The repos are independent, with no version pin between them. An old claudio sets its own env whatever the proxy says. A new claudio ignores every config key except the four model defaults, so an old proxy's key has no effect.
   - claudio: fast-forward `fix/autocompact-window` into `main`. `manager`, `main` and `origin/main` are all 71eebad today.
   - claude-proxy: fast-forward `fix/autocompact-window` into `main`. Local `main` is 5 commits ahead of `origin/main` (ea987ea), with the unpushed `0cb759a`..`8dfe2f9`. Pushing `main` publishes those as well. That includes the model-list edit API (6f1009e, ac8b8e6), which step 5 needs.
3. Release:
   - claudio: patch release v0.3.7. Bump `Cargo.toml` `version`, run `cargo build --locked` to refresh Cargo.lock, commit `release v0.3.7` with the claudio trailer, then tag `v0.3.7`. The release workflow checks that the tag matches Cargo.toml.
   - claude-proxy: no tag needed. A push to `main` builds and publishes the GHCR image.
4. **Owner OK required before:**
   - pushing claudio `main`;
   - pushing tag `v0.3.7`;
   - pushing claude-proxy `main` (this includes the 5 unpushed commits);
   - deploying the proxy (`make pull && make up` on the proxy host, after the CI image for that `main` commit is published);
   - the ops step below.
5. Post-merge ops (owner-gated, not code): on the deployed proxy, set `context_window: 480000` on the `claude-openai-qwen38-27b` model of the custom-host credential. Use the web UI's *Edit models* or `POST /api/credentials/{id}/models` (docs/WEBUI.md, from claude-proxy ac8b8e6). The stored id carries no advertise prefix: the API rejects `claude-`/`claude-openai-` ids, so the entry is listed under its bare id. That endpoint replaces the whole list, so resend every other entry unchanged.
6. Post-release checks:
   - `curl -s -H "Authorization: Bearer <user token>" https://<proxy>/v1/claudio/config | jq .env` has no `CLAUDE_CODE_AUTO_COMPACT_WINDOW`. Type the token interactively and never paste it into logs.
   - The Alt+s Models page lists `claude-openai-qwen38-27b` with 480k.
   - A new claudio proxy session on `claude-opus-5-5[1m]` shows `ctx … /1M`.
   - A session on `claude-openai-qwen38-27b` shows `ctx … /200k`, and claude prints its "model not recognized, 200k assumed" notice.
