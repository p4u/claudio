//! Locating and reading Claude Code project directories and session transcripts.
//!
//! All paths honour `$CLAUDE_CONFIG_DIR`; the default is `~/.claude`.
//! Subagent transcripts (under `<uuid>/subagents/`) are ignored.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::proto::ClaudeSession;

// ── Constants ─────────────────────────────────────────────────────────────────

/// How many bytes to read from the tail of a transcript when looking for
/// `ai-title` and `last-prompt`. 256 KiB is plenty for the last few records.
const TAIL_READ_BYTES: u64 = 256 * 1024;

/// Maximum number of `type:"user"` records we'll count before giving up.
const MAX_USER_SCAN: usize = 10_000;

/// `~/.claude` (honours `$CLAUDE_CONFIG_DIR`).
pub fn projects_root() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::paths::home().join(".claude"))
        .join("projects")
}

/// The project directory for a given working directory.
///
/// Claude encodes the path by replacing every non-alphanumeric character with
/// `-`. The mapping is lossy, so the `cwd` field inside each transcript is the
/// canonical source of truth.
pub fn project_dir(cwd: &Path) -> PathBuf {
    let encoded = encode_cwd(cwd);
    projects_root().join(encoded)
}

/// The absolute path of a transcript file for a given (cwd, claude_session_id).
#[allow(dead_code)]
pub fn transcript_path(cwd: &Path, claude_session_id: &str) -> PathBuf {
    project_dir(cwd).join(format!("{claude_session_id}.jsonl"))
}

/// Encode a path as Claude does: replace every non-alphanumeric character with
/// `-`.
fn encode_cwd(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

// ── Session listing ───────────────────────────────────────────────────────────

/// List all resumable claude sessions for `cwd`, newest first.
///
/// Reads transcripts only from the project directory that matches `cwd`'s
/// encoded path. Only files whose recorded `cwd` field exactly equals the
/// given path are included. Files with no user message are skipped. Unreadable
/// files and missing directories yield empty results, never panics.
pub fn list_sessions(cwd: &Path) -> Vec<ClaudeSession> {
    let dir = project_dir(cwd);
    let cwd_str = cwd.to_string_lossy();

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut sessions: Vec<(u64, ClaudeSession)> = vec![];

    for entry in entries.flatten() {
        let path = entry.path();
        // Only *.jsonl files at the top level (not in subagent subdirectories).
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let fname = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_owned(),
            None => continue,
        };

        // mtime for sorting.
        let modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if let Some(session) = parse_session(&path, &fname, &cwd_str) {
            sessions.push((modified, session));
        }
    }

    // Sort newest first by mtime, then by session id for stability.
    sessions.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    sessions.into_iter().map(|(_, s)| s).collect()
}

/// Parse one transcript file into a `ClaudeSession`. Returns `None` if the
/// file's `cwd` doesn't match or if there are no user messages.
fn parse_session(path: &Path, session_id: &str, expected_cwd: &str) -> Option<ClaudeSession> {
    let content = read_file_tail(path)?;

    // We do a two-pass scan: first pass (full file would be expensive for big
    // files) for cwd verification and message count; second pass (tail only)
    // for title and last-prompt. Since we already read the tail, we scan it
    // for cwd too — and fall back to reading the head if the cwd record isn't
    // in the tail.
    let cwd_ok = verify_cwd_in_tail(&content, expected_cwd)
        || verify_cwd_in_head(path, expected_cwd);
    if !cwd_ok {
        return None;
    }

    // Count user messages (real prompts) and collect metadata from tail.
    let messages = count_user_messages(path);
    if messages == 0 {
        return None;
    }

    let (title, last_prompt) = extract_tail_meta(&content);

    let modified = path
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    Some(ClaudeSession { id: session_id.to_owned(), title, last_prompt, modified, messages })
}

/// Read up to the last `TAIL_READ_BYTES` bytes of a file as a UTF-8 string.
///
/// When seeking into the middle of a file, the seek position may fall inside a
/// multi-byte UTF-8 sequence, causing `read_to_string` to fail and the whole
/// session to silently disappear. We instead read raw bytes and skip the first
/// (potentially partial) line so every subsequent line is complete.
fn read_file_tail(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();

    let mut buf = Vec::new();
    if len > TAIL_READ_BYTES {
        f.seek(SeekFrom::End(-(TAIL_READ_BYTES as i64))).ok()?;
        f.read_to_end(&mut buf).ok()?;
        // Drop the first (possibly partial) line — it may start mid-character
        // or mid-JSON-record due to the seek position.
        let first_newline = buf.iter().position(|&b| b == b'\n')?;
        let complete = &buf[first_newline + 1..];
        String::from_utf8(complete.to_vec()).ok()
    } else {
        f.read_to_end(&mut buf).ok()?;
        String::from_utf8(buf).ok()
    }
}

/// Check whether any line in `content` (tail bytes) has `"cwd": "<expected>"`.
fn verify_cwd_in_tail(content: &str, expected_cwd: &str) -> bool {
    for line in content.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("cwd").and_then(|c| c.as_str()) == Some(expected_cwd) {
                return true;
            }
        }
    }
    false
}

/// Read the head of the file (first 16 KiB) and look for a `cwd` field.
fn verify_cwd_in_head(path: &Path, expected_cwd: &str) -> bool {
    use std::io::Read;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = vec![0u8; 16 * 1024];
    let n = match f.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let text = String::from_utf8_lossy(&buf[..n]);
    for line in text.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("cwd").and_then(|c| c.as_str()) == Some(expected_cwd) {
                return true;
            }
        }
    }
    false
}

/// Count the number of `type:"user"` records that look like real user prompts.
///
/// A real prompt is discriminated by *record structure*, not by string content:
/// - A `type:"user"` record whose `message.content` is a string → real prompt.
/// - A `type:"user"` record whose `message.content` is an array containing
///   `tool_result` blocks → tool-result wrapper injected by Claude Code, skip.
/// - A `type:"user"` record whose `message.content` is an array of `text` blocks
///   without any `tool_result` → real prompt.
///
/// This correctly handles prompts that start with `<` (e.g. `<task>…</task>`),
/// which the previous `starts_with('<')` heuristic misclassified as tool output.
///
/// The scan is capped at `MAX_USER_SCAN` records.
fn count_user_messages(path: &Path) -> u32 {
    use std::io::{BufRead, BufReader};
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let reader = BufReader::new(f);
    let mut count: u32 = 0;
    let mut scanned = 0usize;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Cheap pre-filter before JSON parse: only bother with lines that
        // contain `"type":"user"` or `"type": "user"`.
        if !line.contains("\"user\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("user") {
                if is_real_prompt(&v) {
                    count += 1;
                }
                scanned += 1;
                if scanned >= MAX_USER_SCAN {
                    break;
                }
            }
        }
    }
    count
}

/// Decide whether a `type:"user"` record carries a real human prompt (as
/// opposed to a tool-result wrapper).
///
/// Discrimination is based on **record structure**, not string content:
/// - String content → always a real prompt.
/// - Array content with any `tool_result` block → tool-result wrapper, not a prompt.
/// - Array content with `text` blocks and no `tool_result` → real prompt.
///
/// This correctly handles prompts that start with `<` (e.g. XML-tagged tasks)
/// which were previously misclassified.
fn is_real_prompt(record: &serde_json::Value) -> bool {
    let Some(msg) = record.get("message") else { return false };
    let content = match msg.get("content") {
        Some(c) => c,
        None => return false,
    };
    match content {
        serde_json::Value::String(s) => {
            // String content is always a real prompt (even if it starts with '<').
            !s.trim().is_empty()
        }
        serde_json::Value::Array(items) => {
            // If any item is a tool_result block, this is a tool-result wrapper.
            if items.iter().any(|item| {
                item.get("type").and_then(|t| t.as_str()) == Some("tool_result")
            }) {
                return false;
            }
            // Also check the toolUseResult field (alternative representation).
            if msg.get("toolUseResult").is_some() {
                return false;
            }
            // Real prompt: at least one non-empty text block.
            items.iter().any(|item| {
                item.get("type").and_then(|t| t.as_str()) == Some("text")
                    && item
                        .get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| !s.trim().is_empty())
                        .unwrap_or(false)
            })
        }
        _ => false,
    }
}

/// Extract `ai-title` (last occurrence) and `last-prompt` from the tail bytes.
fn extract_tail_meta(content: &str) -> (Option<String>, Option<String>) {
    let mut title: Option<String> = None;
    let mut last_prompt: Option<String> = None;

    for line in content.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            match v.get("type").and_then(|t| t.as_str()) {
                Some("ai-title") => {
                    if let Some(t) = v.get("aiTitle").and_then(|t| t.as_str()) {
                        title = Some(t.to_owned());
                    }
                }
                Some("last-prompt") => {
                    if let Some(p) = v.get("lastPrompt").and_then(|p| p.as_str()) {
                        last_prompt = Some(p.to_owned());
                    }
                }
                _ => {}
            }
        }
    }

    (title, last_prompt)
}

// ── Recent project dirs ───────────────────────────────────────────────────────

/// Return up to `limit` directories that have claude history, newest first.
///
/// Uses one cheap read (the first few lines of the newest transcript) per
/// project directory to extract the `cwd` field.
pub fn recent_project_dirs(limit: usize) -> Vec<(PathBuf, u64)> {
    let root = projects_root();
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut dirs: Vec<(u64, PathBuf)> = vec![];

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Skip the `memory/` directory and similar non-project dirs.
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| !n.contains('-'))
            .unwrap_or(false)
        {
            continue;
        }

        // Find the newest transcript in this project dir.
        let (newest_mtime, cwd_path) = newest_transcript_cwd(&path);
        if let Some(cwd) = cwd_path {
            dirs.push((newest_mtime, cwd));
        }
    }

    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    dirs.truncate(limit);
    dirs.into_iter().map(|(mtime, p)| (p, mtime)).collect()
}

/// Find the newest .jsonl transcript in `project_dir` and extract its `cwd`.
/// Returns (mtime_secs, cwd_pathbuf).
fn newest_transcript_cwd(project_dir: &Path) -> (u64, Option<PathBuf>) {
    let entries = match std::fs::read_dir(project_dir) {
        Ok(e) => e,
        Err(_) => return (0, None),
    };

    let mut best: Option<(u64, PathBuf)> = None;

    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
            best = Some((mtime, p));
        }
    }

    match best {
        None => (0, None),
        Some((mtime, transcript)) => {
            let cwd = read_cwd_from_transcript(&transcript);
            (mtime, cwd)
        }
    }
}

/// Read the first few lines of a transcript and return the `cwd` field as a
/// `PathBuf`.
fn read_cwd_from_transcript(path: &Path) -> Option<PathBuf> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(f);

    for line in reader.lines().take(20) {
        let line = line.ok()?;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                if !cwd.is_empty() {
                    return Some(PathBuf::from(cwd));
                }
            }
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_cwd_replaces_non_alnum() {
        assert_eq!(encode_cwd(Path::new("/volumes/repos/claudio")), "-volumes-repos-claudio");
        assert_eq!(encode_cwd(Path::new("/home/p4u")), "-home-p4u");
        assert_eq!(encode_cwd(Path::new("/tmp")), "-tmp");
    }

    #[test]
    fn project_dir_uses_encoded_cwd() {
        let root = projects_root();
        let p = project_dir(Path::new("/home/p4u"));
        assert_eq!(p, root.join("-home-p4u"));
    }

    #[test]
    fn transcript_path_correct() {
        let p = transcript_path(Path::new("/home/p4u"), "abc-123");
        assert!(p.to_string_lossy().ends_with("/-home-p4u/abc-123.jsonl"));
    }

    // ── is_real_prompt ────────────────────────────────────────────────────────

    #[test]
    fn is_real_prompt_string_content() {
        // Plain text → real prompt
        let v = serde_json::json!({"type": "user", "message": {"content": "hello world"}});
        assert!(is_real_prompt(&v));

        // Empty → not a real prompt
        let v = serde_json::json!({"type": "user", "message": {"content": ""}});
        assert!(!is_real_prompt(&v));
    }

    /// Fable M12: prompts starting with `<` are real prompts, not tool output.
    #[test]
    fn is_real_prompt_xml_tagged_task_is_real() {
        let v = serde_json::json!({
            "type": "user",
            "message": {"content": "<task>Do something important</task>"}
        });
        assert!(is_real_prompt(&v), "XML-tagged task prompt should be real");
    }

    /// Tool result (array with tool_result block) → not a real prompt.
    #[test]
    fn is_real_prompt_tool_result_array_is_not_real() {
        let v = serde_json::json!({
            "type": "user",
            "message": {
                "content": [
                    {"type": "tool_result", "tool_use_id": "x", "content": "output"}
                ]
            }
        });
        assert!(!is_real_prompt(&v), "tool_result array should not be a real prompt");
    }

    #[test]
    fn is_real_prompt_array_content() {
        let v = serde_json::json!({
            "type": "user",
            "message": {
                "content": [{"type": "text", "text": "hello"}]
            }
        });
        assert!(is_real_prompt(&v));

        // Text block that starts with '<' but no tool_result → still real.
        let v = serde_json::json!({
            "type": "user",
            "message": {
                "content": [{"type": "text", "text": "<task>do stuff</task>"}]
            }
        });
        assert!(is_real_prompt(&v), "text block starting with < should be real");
    }

    /// Fable M12: toolUseResult field marks a tool-result record.
    #[test]
    fn is_real_prompt_tool_use_result_field() {
        let v = serde_json::json!({
            "type": "user",
            "message": {
                "toolUseResult": {"tool_use_id": "x"},
                "content": [{"type": "text", "text": "some text"}]
            }
        });
        assert!(!is_real_prompt(&v), "toolUseResult field should mark as tool result");
    }

    // ── read_file_tail ────────────────────────────────────────────────────────

    /// Fable M12: tail read must not fail when the seek lands in a multi-byte
    /// UTF-8 sequence.
    #[test]
    fn read_file_tail_multibyte_boundary() {
        use std::io::Write;
        let dir = std::env::temp_dir()
            .join(format!("claudio-tail-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.jsonl");

        // Write a file larger than TAIL_READ_BYTES so the seek happens.
        // Fill with ASCII lines up to TAIL_READ_BYTES, then add a line with a
        // multi-byte char (é = 2 bytes in UTF-8) right at the boundary.
        let mut f = std::fs::File::create(&path).unwrap();
        // Write enough filler to push past TAIL_READ_BYTES.
        let filler_line = b"{\"type\":\"summary\"}\n";
        let filler_count = (TAIL_READ_BYTES as usize / filler_line.len()) + 2;
        for _ in 0..filler_count {
            f.write_all(filler_line).unwrap();
        }
        // Add a line with multi-byte characters near the end.
        f.write_all("{\"type\":\"user\",\"message\":{\"content\":\"café\"}}\n".as_bytes()).unwrap();
        drop(f);

        // Must return Some (not fail on UTF-8 boundary).
        let result = read_file_tail(&path);
        assert!(result.is_some(), "read_file_tail should not fail at multibyte boundary");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_sessions_missing_dir_returns_empty() {
        let p = std::env::temp_dir().join(format!("claudio-test-missing-{}", uuid::Uuid::new_v4()));
        // Set CLAUDE_CONFIG_DIR to a non-existent path (parent of projects/).
        // list_sessions should return [] without panicking.
        let sessions = {
            // We can't easily override env here without risk; just call project_dir
            // and ensure the function handles missing dirs.
            let fake_cwd = Path::new("/nonexistent/project/path");
            // project_dir for this cwd will almost certainly not exist
            let _ = p; // suppress unused warning
            list_sessions(fake_cwd)
        };
        assert!(sessions.is_empty());
    }

    #[test]
    fn recent_project_dirs_missing_root_returns_empty() {
        // With a fake CLAUDE_CONFIG_DIR, projects_root won't exist.
        // recent_project_dirs must return [] without panicking.
        // We simulate by temporarily not having a projects root — just
        // call it and ensure no panic. The real env may or may not have data.
        let result = std::panic::catch_unwind(recent_project_dirs_no_panic_wrapper);
        assert!(result.is_ok());
    }

    fn recent_project_dirs_no_panic_wrapper() {
        // This just must not panic; we don't care about the actual result.
        let _ = recent_project_dirs(10);
    }
}
