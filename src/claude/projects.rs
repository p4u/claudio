//! Locating and reading Claude Code project directories and session transcripts.
//!
//! All paths honour `$CLAUDE_CONFIG_DIR`; the default is `~/.claude`.
//! Subagent transcripts (under `<uuid>/subagents/`) are ignored.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::claude::latex::{extract_latex, lens_tags, sanitize, MAX_FORMULAS};
use crate::proto::{Answer, ClaudeSession, LensItem};

// ── Constants ─────────────────────────────────────────────────────────────────

/// How many bytes to read from the tail of a transcript when looking for
/// `ai-title` and `last-prompt`. 256 KiB is plenty for the last few records.
const TAIL_READ_BYTES: u64 = 256 * 1024;

/// Maximum number of `type:"user"` records we'll count before giving up.
const MAX_USER_SCAN: usize = 10_000;

/// `~/.claude` (honours `$CLAUDE_CONFIG_DIR`).
pub fn config_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::paths::home().join(".claude"))
}

/// `~/.claude/projects`.
pub fn projects_root() -> PathBuf {
    config_dir().join("projects")
}

/// The project directory for a given working directory.
///
/// Claude encodes the path by replacing every non-alphanumeric character with
/// `-`. The mapping is lossy, so the `cwd` field inside each transcript is the
/// canonical source of truth. The `cwd` is resolved first (blocking I/O):
/// claude files transcripts under its resolved working directory, so a
/// symlinked path would miss them.
pub fn project_dir(cwd: &Path) -> PathBuf {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let encoded = encode_cwd(&cwd);
    projects_root().join(encoded)
}

/// The absolute path of a transcript file for a given (cwd, claude_session_id).
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
    list_sessions_in(&projects_root(), cwd)
}

/// [`list_sessions`] under an explicit projects root.
///
/// claude files a transcript under its resolved working directory: a session
/// started in a symlinked path (`~/repos/x` → `/srv/x`) records and
/// is filed under the target. So `cwd` is resolved the same way first.
fn list_sessions_in(root: &Path, cwd: &Path) -> Vec<ClaudeSession> {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let dir = root.join(encode_cwd(&cwd));
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
    let cwd_ok =
        verify_cwd_in_tail(&content, expected_cwd) || verify_cwd_in_head(path, expected_cwd);
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

    Some(ClaudeSession {
        id: session_id.to_owned(),
        title,
        last_prompt,
        modified,
        messages,
    })
}

/// Read up to the last `TAIL_READ_BYTES` bytes of a file as a UTF-8 string.
///
/// When seeking into the middle of a file, the seek position may fall inside a
/// multi-byte UTF-8 sequence, causing `read_to_string` to fail and the whole
/// session to silently disappear. We instead read raw bytes and skip the first
/// (potentially partial) line so every subsequent line is complete.
pub(crate) fn read_file_tail(path: &Path) -> Option<String> {
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

// ── Transcript cursor ─────────────────────────────────────────────────────────

/// Most bytes one [`TranscriptCursor::advance`] reads. A long transcript
/// opened for the first time (a resume) is caught up over several refreshes
/// instead of one long read.
const CURSOR_READ_CAP: u64 = 8 * 1024 * 1024;

/// What a transcript says about its conversation so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptTotals {
    /// The model of the last assistant record.
    pub model: Option<String>,
    /// Context of the last assistant turn: `input_tokens +
    /// cache_creation_input_tokens + cache_read_input_tokens`.
    pub context_tokens: Option<u64>,
    /// Sum of every assistant record's `output_tokens`.
    pub output_tokens: u64,
    /// User prompts (tool results excluded).
    pub turns: u32,
}

/// Incremental reader of one session's transcript: each [`advance`] parses
/// only what was appended since the last one, so the totals cover the whole
/// conversation at the cost of the new records.
///
/// [`advance`]: TranscriptCursor::advance
#[derive(Debug)]
pub struct TranscriptCursor {
    session_id: String,
    path: PathBuf,
    /// Bytes consumed so far.
    offset: u64,
    /// The unterminated last line of the previous read.
    partial: Vec<u8>,
    /// `message.id` of the last assistant record: one API response is written
    /// as one record per content block, all with the same usage, which must
    /// be counted once.
    last_message_id: Option<String>,
    totals: TranscriptTotals,
    /// The `<latex>` formulas of the conversation's answers, oldest first:
    /// the last [`MAX_FORMULAS`].
    latex: VecDeque<String>,
    /// The tags of the session's tag lenses ([`TranscriptCursor::with_tags`]).
    tags: Vec<String>,
    /// Their items in the conversation's answers, all tags together, oldest
    /// first: the last [`MAX_FORMULAS`].
    lens: VecDeque<LensItem>,
}

impl TranscriptCursor {
    /// A cursor at the start of the transcript of `(cwd, session_id)`.
    pub fn new(cwd: &Path, session_id: &str) -> Self {
        Self {
            session_id: session_id.to_owned(),
            path: transcript_path(cwd, session_id),
            offset: 0,
            partial: Vec::new(),
            last_message_id: None,
            totals: TranscriptTotals::default(),
            latex: VecDeque::new(),
            tags: Vec::new(),
            lens: VecDeque::new(),
        }
    }

    /// The cursor, also gathering the items of tag lenses `tags`.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// The claude session id this cursor follows.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn totals(&self) -> &TranscriptTotals {
        &self.totals
    }

    /// The conversation's most recent formulas, oldest first.
    pub fn latex(&self) -> &VecDeque<String> {
        &self.latex
    }

    /// The conversation's most recent tag-lens items, oldest first.
    pub fn lens(&self) -> &VecDeque<LensItem> {
        &self.lens
    }

    /// Whether the cursor has seen an assistant record.
    pub fn has_assistant(&self) -> bool {
        self.totals.model.is_some()
    }

    /// Read what the transcript gained since the last call (at most
    /// [`CURSOR_READ_CAP`] bytes). A file that shrank (rewritten) is read
    /// again from the start. Returns whether the totals changed.
    pub fn advance(&mut self) -> bool {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(&self.path) else {
            return false;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            self.offset = 0;
            self.partial.clear();
            self.last_message_id = None;
            self.totals = TranscriptTotals::default();
            self.latex.clear();
            self.lens.clear();
        }
        if len == self.offset {
            return false;
        }
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let want = (len - self.offset).min(CURSOR_READ_CAP);
        let mut buf = Vec::with_capacity(want as usize);
        if f.take(want).read_to_end(&mut buf).is_err() {
            return false;
        }
        self.offset += buf.len() as u64;
        let before = self.totals.clone();
        self.feed(&buf);
        before != self.totals
    }

    /// Consume appended bytes, parsing every complete line.
    fn feed(&mut self, bytes: &[u8]) {
        let mut data = std::mem::take(&mut self.partial);
        data.extend_from_slice(bytes);
        let mut start = 0;
        while let Some(nl) = data[start..].iter().position(|&b| b == b'\n') {
            self.apply_record(&data[start..start + nl]);
            start += nl + 1;
        }
        self.partial = data.split_off(start);
    }

    /// Fold one transcript record into the totals.
    fn apply_record(&mut self, line: &[u8]) {
        // Cheap pre-filter: the records of interest name their type.
        let looks_relevant = line.windows(11).any(|w| w == b"\"assistant\"")
            || line.windows(6).any(|w| w == b"\"user\"");
        if !looks_relevant {
            return;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            return;
        };
        let totals = &mut self.totals;
        match v.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                let Some(msg) = v.get("message") else { return };
                // A subagent's work is not the answer on screen.
                if v.get("isSidechain").and_then(|s| s.as_bool()) != Some(true) {
                    push_latex(&mut self.latex, msg.get("content"));
                    push_lens(&mut self.lens, &self.tags, msg.get("content"));
                }
                let Some(usage) = msg.get("usage") else { return };
                let count = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
                if let Some(model) = msg.get("model").and_then(|m| m.as_str()) {
                    // What claude asked for keeps the `[1m]` the context
                    // window depends on; the API's name drops it.
                    let requested = v.get("requestedModel").and_then(|m| m.as_str());
                    totals.model = Some(requested.unwrap_or(model).to_owned());
                    totals.context_tokens = Some(
                        count("input_tokens")
                            + count("cache_creation_input_tokens")
                            + count("cache_read_input_tokens"),
                    );
                }
                let message_id = msg.get("id").and_then(|i| i.as_str()).map(str::to_owned);
                if message_id.is_none() || message_id != self.last_message_id {
                    totals.output_tokens =
                        totals.output_tokens.saturating_add(count("output_tokens"));
                }
                self.last_message_id = message_id;
            }
            Some("user") => {
                if v.get("isMeta").and_then(|m| m.as_bool()) != Some(true) && is_real_prompt(&v) {
                    totals.turns = totals.turns.saturating_add(1);
                }
            }
            _ => {}
        }
    }
}

/// Append the items of tag lenses `tags` in an assistant message to `lens`,
/// keeping the last [`MAX_FORMULAS`]; only its `text` blocks count, as for
/// [`push_latex`].
fn push_lens(lens: &mut VecDeque<LensItem>, tags: &[String], content: Option<&serde_json::Value>) {
    if tags.is_empty() {
        return;
    }
    let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
    let mut add = |text: &str| {
        for (_, tag, text) in lens_tags(text, &tags) {
            if lens.len() == MAX_FORMULAS {
                lens.pop_front();
            }
            lens.push_back(LensItem { tag, text });
        }
    };
    match content {
        Some(serde_json::Value::String(text)) => add(text),
        Some(serde_json::Value::Array(blocks)) => {
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        add(text);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Append the formulas of an assistant message to `latex`, keeping the last
/// [`MAX_FORMULAS`]. `content` is a string or an array of blocks, of which
/// only `text` blocks are the answer (not `thinking` or `tool_use`).
fn push_latex(latex: &mut VecDeque<String>, content: Option<&serde_json::Value>) {
    let mut add = |text: &str| {
        for formula in extract_latex(text) {
            if latex.len() == MAX_FORMULAS {
                latex.pop_front();
            }
            latex.push_back(formula);
        }
    };
    match content {
        Some(serde_json::Value::String(text)) => add(text),
        Some(serde_json::Value::Array(blocks)) => {
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        add(text);
                    }
                }
            }
        }
        _ => {}
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
pub(crate) fn is_real_prompt(record: &serde_json::Value) -> bool {
    let Some(msg) = record.get("message") else {
        return false;
    };
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
            if items
                .iter()
                .any(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
            {
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

// ── Recent answers ────────────────────────────────────────────────────────────

/// How many of a conversation's answers the reader view gets.
pub const MAX_ANSWERS: usize = 10;
/// How much of a transcript's end [`read_answers`] reads.
const ANSWERS_TAIL: u64 = 4 * 1024 * 1024;
/// The longest prompt and answer text sent; longer ones are cut.
const MAX_PROMPT_BYTES: usize = 2 * 1024;
const MAX_ANSWER_BYTES: usize = 64 * 1024;
/// What is appended to a prompt or answer that was cut.
const CUT_MARK: &str = " …";
/// How Claude Code's own `type:"user"` records start: slash commands, their
/// output and caveats, and reminders. None of them is a prompt.
const NOT_PROMPTS: [&str; 5] = [
    "<command-name>",
    "<command-message>",
    "<local-command-stdout>",
    "<local-command-caveat>",
    "<system-reminder>",
];

/// The [`recent_answers`] of the transcript at `path`, from its last
/// [`ANSWERS_TAIL`] bytes ([`read_tail`]). A missing or unreadable
/// transcript has none. Blocking I/O.
pub fn read_answers(path: &Path) -> Vec<Answer> {
    recent_answers(&read_tail(path))
}

/// The [`last_prompt`] of the transcript at `path`, from its last
/// [`ANSWERS_TAIL`] bytes ([`read_tail`]). Blocking I/O.
pub fn read_last_prompt(path: &Path) -> Option<(String, bool)> {
    last_prompt(&read_tail(path))
}

/// The whole lines of the last [`ANSWERS_TAIL`] bytes of the transcript at
/// `path`: a tail's first (partial) line is skipped. Empty when the file is
/// missing or unreadable.
fn read_tail(path: &Path) -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(ANSWERS_TAIL);
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(start)).is_err()
        || f.take(ANSWERS_TAIL).read_to_end(&mut buf).is_err()
    {
        return Vec::new();
    }
    match buf.iter().position(|&b| b == b'\n') {
        Some(nl) if start > 0 => buf.split_off(nl + 1),
        None if start > 0 => Vec::new(),
        _ => buf,
    }
}

/// The answers of a transcript's lines, oldest first: the last
/// [`MAX_ANSWERS`], each a prompt and the text claude answered it with.
///
/// An answer starts at a real prompt: a `type:"user"` record that
/// [`is_real_prompt`] accepts, without `isMeta`, `isCompactSummary` or
/// `isVisibleInTranscriptOnly`, and not one of Claude Code's own records
/// ([`NOT_PROMPTS`]). Its text is the `text` blocks (or string content) of
/// the `type:"assistant"` records that follow, joined by blank lines;
/// records with `isApiErrorMessage` are not claude's answer, and tool
/// results in between do not end it. A compaction boundary ends it. Sidechain
/// records (subagents) are skipped, and so is an answer with no text.
///
/// `bytes` may start or end mid-line (a tail read): a line that is not a
/// JSON record is skipped. Prompt and text are [`sanitize`]d and cut to
/// [`MAX_PROMPT_BYTES`] and [`MAX_ANSWER_BYTES`].
pub fn recent_answers(bytes: &[u8]) -> Vec<Answer> {
    let mut answers = VecDeque::with_capacity(MAX_ANSWERS + 1);
    // The answer being read: its prompt and its text blocks so far.
    let mut current: Option<(String, Vec<String>)> = None;
    for line in bytes.split(|&b| b == b'\n') {
        // Cheap pre-filter: the records of interest name their type.
        let looks_relevant = line.windows(11).any(|w| w == b"\"assistant\"")
            || line.windows(6).any(|w| w == b"\"user\"")
            || line.windows(16).any(|w| w == b"compact_boundary");
        if !looks_relevant {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if flag(&v, "isSidechain") {
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("user") => {
                if let Some(prompt) = answer_prompt(&v) {
                    finish(&mut answers, current.replace((prompt, Vec::new())));
                }
            }
            Some("assistant") if !flag(&v, "isApiErrorMessage") => {
                if let Some((_, texts)) = current.as_mut() {
                    let content = v.get("message").and_then(|m| m.get("content"));
                    push_texts(texts, content);
                }
            }
            Some("system") => {
                if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") {
                    finish(&mut answers, current.take());
                }
            }
            _ => {}
        }
    }
    finish(&mut answers, current);
    answers.into()
}

/// Whether `record` has `key: true`.
pub(crate) fn flag(record: &serde_json::Value, key: &str) -> bool {
    record.get(key).and_then(|f| f.as_bool()) == Some(true)
}

/// The prompt of a `type:"user"` record that starts an answer (see
/// [`recent_answers`]): its string content, or its text blocks joined by
/// newlines.
pub(crate) fn answer_prompt(record: &serde_json::Value) -> Option<String> {
    let own = ["isMeta", "isCompactSummary", "isVisibleInTranscriptOnly"];
    if own.iter().any(|key| flag(record, key)) || !is_real_prompt(record) {
        return None;
    }
    let content = record.get("message")?.get("content")?;
    let mut texts = Vec::new();
    push_texts(&mut texts, Some(content));
    let prompt = texts.join("\n");
    let start = prompt.trim_start();
    (!NOT_PROMPTS.iter().any(|tag| start.starts_with(tag))).then_some(prompt)
}

/// Append the text of a message's `content` to `texts`: the string itself,
/// or its `text` blocks (not `thinking`, `tool_use` or `tool_result`).
pub(crate) fn push_texts(texts: &mut Vec<String>, content: Option<&serde_json::Value>) {
    match content {
        Some(serde_json::Value::String(text)) => texts.push(text.clone()),
        Some(serde_json::Value::Array(blocks)) => {
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        texts.push(text.to_owned());
                    }
                }
            }
        }
        _ => {}
    }
}

/// Add the answer `done` (if any, and if it has text) to `answers`, keeping
/// the last [`MAX_ANSWERS`].
fn finish(answers: &mut VecDeque<Answer>, done: Option<(String, Vec<String>)>) {
    let Some((prompt, texts)) = done else { return };
    let texts: Vec<&str> = texts
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    if texts.is_empty() {
        return;
    }
    if answers.len() == MAX_ANSWERS {
        answers.pop_front();
    }
    answers.push_back(Answer {
        prompt: cut(&sanitize(prompt.trim()), MAX_PROMPT_BYTES),
        text: cut(&sanitize(&texts.join("\n\n")), MAX_ANSWER_BYTES),
    });
}

/// `s` cut to at most `max` bytes on a char boundary, with [`CUT_MARK`]
/// appended when it was cut.
fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{CUT_MARK}", &s[..end])
}

/// The longest prompt [`last_prompt`] returns whole.
pub const MAX_LAST_PROMPT: usize = 64 * 1024;

/// The newest real prompt of a transcript's lines, answered or not (the
/// prompts of [`recent_answers`]; sidechain records are skipped):
/// [`sanitize`]d and trimmed, and cut to [`MAX_LAST_PROMPT`] bytes on a char
/// boundary, with no mark, when longer (then `true`). `None` when there is
/// none.
pub fn last_prompt(bytes: &[u8]) -> Option<(String, bool)> {
    bytes.split(|&b| b == b'\n').rev().find_map(|line| {
        if !line.windows(6).any(|w| w == b"\"user\"") {
            return None;
        }
        let record: serde_json::Value = serde_json::from_slice(line).ok()?;
        let user = record.get("type").and_then(|t| t.as_str()) == Some("user");
        if !user || flag(&record, "isSidechain") {
            return None;
        }
        let prompt = sanitize(answer_prompt(&record)?.trim());
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return None;
        }
        if prompt.len() <= MAX_LAST_PROMPT {
            return Some((prompt.to_owned(), false));
        }
        let mut end = MAX_LAST_PROMPT;
        while !prompt.is_char_boundary(end) {
            end -= 1;
        }
        Some((prompt[..end].to_owned(), true))
    })
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
            // Skip cwds that no longer exist so stale test temp dirs don't
            // pollute the recent-dirs list in the wizard's LOCAL section.
            if cwd.exists() {
                dirs.push((newest_mtime, cwd));
            }
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
        assert_eq!(
            encode_cwd(Path::new("/home/user/src/claudio")),
            "-home-user-src-claudio"
        );
        assert_eq!(encode_cwd(Path::new("/home/user")), "-home-user");
        assert_eq!(encode_cwd(Path::new("/tmp")), "-tmp");
    }

    #[test]
    fn project_dir_uses_encoded_cwd() {
        let root = projects_root();
        let p = project_dir(Path::new("/home/user"));
        assert_eq!(p, root.join("-home-user"));
    }

    #[test]
    fn transcript_path_correct() {
        let p = transcript_path(Path::new("/home/user"), "abc-123");
        assert!(p.to_string_lossy().ends_with("/-home-user/abc-123.jsonl"));
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

    /// Prompts starting with `<` are real prompts, not tool output.
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
        assert!(
            !is_real_prompt(&v),
            "tool_result array should not be a real prompt"
        );
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
        assert!(
            is_real_prompt(&v),
            "text block starting with < should be real"
        );
    }

    /// A `toolUseResult` field marks a tool-result record.
    #[test]
    fn is_real_prompt_tool_use_result_field() {
        let v = serde_json::json!({
            "type": "user",
            "message": {
                "toolUseResult": {"tool_use_id": "x"},
                "content": [{"type": "text", "text": "some text"}]
            }
        });
        assert!(
            !is_real_prompt(&v),
            "toolUseResult field should mark as tool result"
        );
    }

    // ── read_file_tail ────────────────────────────────────────────────────────

    /// The tail read must not fail when the seek lands in a multi-byte
    /// UTF-8 sequence.
    #[test]
    fn read_file_tail_multibyte_boundary() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("claudio-tail-test-{}", uuid::Uuid::new_v4()));
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
        f.write_all("{\"type\":\"user\",\"message\":{\"content\":\"café\"}}\n".as_bytes())
            .unwrap();
        drop(f);

        // Must return Some (not fail on UTF-8 boundary).
        let result = read_file_tail(&path);
        assert!(
            result.is_some(),
            "read_file_tail should not fail at multibyte boundary"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A directory opened through a symlink finds the sessions claude filed
    /// under the symlink's target.
    #[cfg(unix)]
    #[test]
    fn list_sessions_follows_symlinks_like_claude() {
        let base = std::env::temp_dir().join(format!("claudio-proj-{}", uuid::Uuid::new_v4()));
        let real = base.join("volumes/repo");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(base.join("volumes"), base.join("home")).unwrap();
        let real = std::fs::canonicalize(&real).unwrap();

        let root = base.join("projects");
        let dir = root.join(encode_cwd(&real));
        std::fs::create_dir_all(&dir).unwrap();
        let record = serde_json::json!({
            "type": "user",
            "cwd": real.to_string_lossy(),
            "message": {"role": "user", "content": "hello"},
        });
        std::fs::write(dir.join("conv-1.jsonl"), format!("{record}\n")).unwrap();

        let via_link = list_sessions_in(&root, &base.join("home/repo"));
        assert_eq!(via_link.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["conv-1"]);
        assert_eq!(list_sessions_in(&root, &real).len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn transcript_path_follows_symlinks_like_claude() {
        let base = std::env::temp_dir().join(format!("claudio-proj-{}", uuid::Uuid::new_v4()));
        let real = base.join("volumes/repo");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(base.join("volumes"), base.join("home")).unwrap();
        let real = std::fs::canonicalize(&real).unwrap();

        // Compare the project-dir/file tail only: another test briefly sets
        // `$CLAUDE_CONFIG_DIR`, which moves the root under parallel runs.
        let via_link = transcript_path(&base.join("home/repo"), "sid");
        assert!(via_link.ends_with(format!("{}/sid.jsonl", encode_cwd(&real))));
        assert!(transcript_path(&real, "sid").ends_with(format!("{}/sid.jsonl", encode_cwd(&real))));

        // A path that does not exist cannot be resolved: it is encoded as given.
        let missing = base.join("home/missing");
        assert!(transcript_path(&missing, "sid").ends_with(format!("{}/sid.jsonl", encode_cwd(&missing))));
        let _ = std::fs::remove_dir_all(&base);
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

    // ── TranscriptCursor ─────────────────────────────────────────────────────

    fn cursor() -> TranscriptCursor {
        TranscriptCursor::new(Path::new("/nonexistent"), "sid")
    }

    fn assistant(id: &str, model: &str, input: u64, cache_create: u64, cache_read: u64, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{input},"cache_creation_input_tokens":{cache_create},"cache_read_input_tokens":{cache_read},"output_tokens":{out}}}}}}}"#
        ) + "\n"
    }

    #[test]
    fn cursor_takes_model_and_context_from_the_last_assistant_record() {
        let mut c = cursor();
        assert!(!c.has_assistant());
        c.feed(assistant("m1", "claude-old", 100, 0, 0, 5).as_bytes());
        // Context sums input and both cache figures.
        c.feed(assistant("m2", "claude-opus-4-5", 100, 200, 300, 10).as_bytes());
        assert!(c.has_assistant());
        assert_eq!(c.totals().model.as_deref(), Some("claude-opus-4-5"));
        assert_eq!(c.totals().context_tokens, Some(600));
        assert_eq!(c.totals().output_tokens, 15);
    }

    /// The API names the model without the `[1m]` claude asked for (and
    /// recorded as `requestedModel`); the context window needs it.
    #[test]
    fn cursor_prefers_the_requested_model() {
        let mut c = cursor();
        let record = assistant("m1", "claude-opus-5-5", 1, 0, 0, 1)
            .replacen('{', r#"{"requestedModel":"claude-opus-5-5[1m]","#, 1);
        c.feed(record.as_bytes());
        assert_eq!(c.totals().model.as_deref(), Some("claude-opus-5-5[1m]"));
    }

    #[test]
    fn cursor_counts_output_once_per_api_response_and_prompts_without_tool_results() {
        let mut c = cursor();
        let mut jsonl = String::new();
        jsonl += r#"{"type":"user","message":{"content":"hello"}}"#;
        jsonl += "\n";
        // One response, two content blocks: same message id and usage.
        jsonl += &assistant("m1", "claude-x", 10, 0, 0, 40);
        jsonl += &assistant("m1", "claude-x", 10, 0, 0, 40);
        jsonl += r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t"}]}}"#;
        jsonl += "\n";
        jsonl += &assistant("m2", "claude-x", 20, 0, 0, 8);
        jsonl += r#"{"type":"user","isMeta":true,"message":{"content":"<system-reminder>x</system-reminder>"}}"#;
        jsonl += "\n";
        jsonl += r#"{"type":"user","message":{"content":[{"type":"text","text":"again"}]}}"#;
        jsonl += "\n";
        jsonl += r#"{"type":"summary","summary":"not a turn"}"#;
        jsonl += "\n";
        c.feed(jsonl.as_bytes());
        assert_eq!(c.totals().output_tokens, 48);
        assert_eq!(c.totals().turns, 2);
        assert_eq!(c.totals().context_tokens, Some(20));
    }

    #[test]
    fn cursor_reassembles_lines_split_across_reads() {
        let mut c = cursor();
        let line = assistant("m1", "claude-x", 1, 0, 0, 7);
        let (a, b) = line.as_bytes().split_at(30);
        c.feed(a);
        assert!(!c.has_assistant(), "an unterminated line is not a record yet");
        c.feed(b);
        assert_eq!(c.totals().output_tokens, 7);
        // Garbage lines are skipped, not fatal.
        c.feed(b"not json \"assistant\"\n{\"type\":\"assistant\"}\n");
        assert_eq!(c.totals().output_tokens, 7);
    }

    #[test]
    fn cursor_reads_appended_bytes_and_restarts_on_a_rewritten_file() {
        let dir = std::env::temp_dir().join(format!("claudio-cursor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let mut c = TranscriptCursor::new(Path::new("/x"), "sid");
        c.path = path.clone();
        assert!(!c.advance(), "missing file");
        std::fs::write(&path, assistant("m1", "claude-x", 5, 0, 0, 3)).unwrap();
        assert!(c.advance());
        assert!(!c.advance(), "nothing new");
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(assistant("m2", "claude-x", 9, 0, 0, 4).as_bytes()).unwrap();
        }
        assert!(c.advance());
        assert_eq!(c.totals().output_tokens, 7);
        assert_eq!(c.totals().context_tokens, Some(9));
        // A shorter file is a new one.
        std::fs::write(&path, assistant("m9", "claude-y", 1, 0, 0, 1)).unwrap();
        assert!(c.advance());
        assert_eq!(c.totals().output_tokens, 1);
        assert_eq!(c.totals().model.as_deref(), Some("claude-y"));
        assert_eq!(c.session_id(), "sid");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An assistant record whose `message.content` is `content` (JSON).
    fn said(content: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"m","model":"claude-x","content":{content},"usage":{{"output_tokens":1}}}}}}"#
        ) + "\n"
    }

    #[test]
    fn cursor_collects_formulas_from_the_answer_text_only() {
        let mut c = cursor();
        let mut jsonl = String::new();
        jsonl += &said(
            r#"[{"type":"thinking","thinking":"<latex>t</latex>"},{"type":"text","text":"so <latex>a^2</latex>"},{"type":"tool_use","input":{"x":"<latex>u</latex>"}}]"#,
        );
        jsonl += &said(r#""a string <latex>b^2</latex>""#);
        // A subagent's record and the user's own text are not the answer.
        jsonl += &said(r#"[{"type":"text","text":"<latex>side</latex>"}]"#)
            .replacen('{', r#"{"isSidechain":true,"#, 1);
        jsonl += r#"{"type":"user","message":{"content":"<latex>user</latex>"}}"#;
        jsonl += "\n";
        // A record without usage is still read for formulas.
        jsonl += r#"{"type":"assistant","message":{"content":[{"type":"text","text":"<latex>c</latex>"}]}}"#;
        jsonl += "\n";
        c.feed(jsonl.as_bytes());
        assert_eq!(c.latex().iter().collect::<Vec<_>>(), ["a^2", "b^2", "c"]);
    }

    #[test]
    fn cursor_keeps_the_last_64_formulas() {
        let mut c = cursor();
        for i in 0..70 {
            c.feed(said(&format!(r#""<latex>x_{{{i}}}</latex>""#)).as_bytes());
        }
        assert_eq!(c.latex().len(), crate::claude::latex::MAX_FORMULAS);
        assert_eq!(c.latex().front().map(String::as_str), Some("x_{6}"));
        assert_eq!(c.latex().back().map(String::as_str), Some("x_{69}"));
    }

    #[test]
    fn cursor_forgets_the_formulas_of_a_rewritten_file() {
        let dir = std::env::temp_dir().join(format!("claudio-cursor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let mut c = TranscriptCursor::new(Path::new("/x"), "sid");
        c.path = path.clone();
        std::fs::write(&path, said(r#""<latex>old</latex>""#)).unwrap();
        assert!(c.advance());
        assert_eq!(c.latex().iter().collect::<Vec<_>>(), ["old"]);
        // A shorter file is a new one.
        std::fs::write(&path, said(r#""<latex>n</latex>""#)).unwrap();
        c.advance();
        assert_eq!(c.latex().iter().collect::<Vec<_>>(), ["n"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn recent_project_dirs_no_panic_wrapper() {
        // This just must not panic; we don't care about the actual result.
        let _ = recent_project_dirs(10);
    }

    /// `recent_project_dirs` must skip cwds whose directories no longer exist.
    ///
    /// This prevents stale test temp dirs (e.g. `/tmp/cl-e2e-*`) from appearing
    /// in the wizard's LOCAL section after the test cleans up its temp root.
    #[test]
    fn recent_project_dirs_skips_nonexistent_cwd() {
        use std::io::Write;
        // Build a fake projects root.
        let tmp = std::env::temp_dir()
            .join(format!("claudio-rpd-test-{}", uuid::Uuid::new_v4()));
        let projects = tmp.join("projects");
        // A "deleted" project: the transcript's cwd points at a dir that doesn't exist.
        let deleted_cwd = tmp.join("deleted-session-dir");
        // DO NOT create deleted_cwd — it must not exist.
        let encoded = deleted_cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>();
        let proj_dir = projects.join(&encoded);
        std::fs::create_dir_all(&proj_dir).unwrap();
        let transcript = proj_dir.join("abc123.jsonl");
        let mut f = std::fs::File::create(&transcript).unwrap();
        writeln!(
            f,
            r#"{{"type":"summary","cwd":"{}","sessionId":"abc123"}}"#,
            deleted_cwd.display()
        )
        .unwrap();
        // Also write a line with a user message so the transcript isn't skipped.
        writeln!(
            f,
            r#"{{"type":"user","message":{{"content":"hello"}}}}"#
        )
        .unwrap();
        drop(f);

        // A "live" project whose cwd actually exists.
        let live_cwd = tmp.join("live-session-dir");
        std::fs::create_dir_all(&live_cwd).unwrap();
        let encoded_live = live_cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>();
        let live_dir = projects.join(&encoded_live);
        std::fs::create_dir_all(&live_dir).unwrap();
        let live_transcript = live_dir.join("def456.jsonl");
        let mut f = std::fs::File::create(&live_transcript).unwrap();
        writeln!(
            f,
            r#"{{"type":"summary","cwd":"{}","sessionId":"def456"}}"#,
            live_cwd.display()
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"user","message":{{"content":"hello"}}}}"#
        )
        .unwrap();
        drop(f);

        // Override CLAUDE_CONFIG_DIR to point at our fake root.
        // SAFETY: single-threaded test process for env mutation.
        std::env::set_var("CLAUDE_CONFIG_DIR", tmp.to_str().unwrap());
        let result = recent_project_dirs(100);
        std::env::remove_var("CLAUDE_CONFIG_DIR");

        let _ = std::fs::remove_dir_all(&tmp);

        let paths: Vec<String> = result
            .iter()
            .map(|(p, _)| p.to_string_lossy().to_string())
            .collect();
        assert!(
            !paths.iter().any(|p| p == &deleted_cwd.to_string_lossy().to_string()),
            "deleted cwd must not appear in results; got: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p == &live_cwd.to_string_lossy().to_string()),
            "live cwd must appear in results; got: {paths:?}"
        );
    }

    // ── recent_answers ────────────────────────────────────────────────────────

    /// A transcript of `records`, one JSON line each.
    fn jsonl(records: &[serde_json::Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for record in records {
            out.extend_from_slice(record.to_string().as_bytes());
            out.push(b'\n');
        }
        out
    }

    fn prompt_record(content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type": "user", "message": {"role": "user", "content": content}})
    }

    fn answer_record(content: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": content}})
    }

    fn answer(prompt: &str, text: &str) -> Answer {
        Answer {
            prompt: prompt.into(),
            text: text.into(),
        }
    }

    #[test]
    fn answers_are_a_prompt_and_the_text_blocks_after_it() {
        use serde_json::json;
        let transcript = jsonl(&[
            prompt_record(json!("first question")),
            answer_record(json!([
                {"type": "thinking", "thinking": "hmm"},
                {"type": "text", "text": "Part one."},
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {}},
            ])),
            // A tool result does not end the answer.
            prompt_record(json!([{"type": "tool_result", "tool_use_id": "t1", "content": "file"}])),
            answer_record(json!([{"type": "text", "text": "Part two: <latex>x^2</latex>"}])),
            prompt_record(
                json!([{"type": "text", "text": "second"}, {"type": "text", "text": "question"}]),
            ),
            answer_record(json!("A string answer.")),
        ]);
        assert_eq!(
            recent_answers(&transcript),
            [
                answer(
                    "first question",
                    "Part one.\n\nPart two: <latex>x^2</latex>"
                ),
                answer("second\nquestion", "A string answer."),
            ]
        );
    }

    #[test]
    fn answers_skip_claude_codes_own_records() {
        use serde_json::json;
        let mut meta = prompt_record(json!("caveat text"));
        meta["isMeta"] = json!(true);
        let mut summary = prompt_record(json!(
            "This session is being continued from a previous conversation"
        ));
        summary["isCompactSummary"] = json!(true);
        let mut visible = prompt_record(json!("only in the transcript"));
        visible["isVisibleInTranscriptOnly"] = json!(true);
        let mut api_error =
            answer_record(json!([{"type": "text", "text": "API Error: stream error"}]));
        api_error["isApiErrorMessage"] = json!(true);
        let mut side_prompt = prompt_record(json!("a subagent's task"));
        side_prompt["isSidechain"] = json!(true);
        let mut side_text = answer_record(json!("a subagent's answer"));
        side_text["isSidechain"] = json!(true);
        let transcript = jsonl(&[
            prompt_record(json!("the question")),
            meta,
            prompt_record(json!(
                "<command-name>/model</command-name>\n<command-args></command-args>"
            )),
            prompt_record(json!("<command-message>model</command-message>")),
            prompt_record(json!(
                "<local-command-stdout>Set model</local-command-stdout>"
            )),
            prompt_record(json!("<local-command-caveat>Caveat</local-command-caveat>")),
            prompt_record(json!("  <system-reminder>remember</system-reminder>")),
            visible,
            side_prompt,
            side_text,
            api_error,
            answer_record(json!([{"type": "text", "text": "The answer."}])),
            // A compaction ends the answer; what follows has no prompt.
            json!({"type": "system", "subtype": "compact_boundary"}),
            summary,
            answer_record(json!("after the summary")),
            // A prompt that got no text is no answer.
            prompt_record(json!("unanswered")),
            answer_record(json!([{"type": "tool_use", "id": "t", "name": "Bash", "input": {}}])),
        ]);
        assert_eq!(
            recent_answers(&transcript),
            [answer("the question", "The answer.")]
        );
    }

    #[test]
    fn answers_are_the_last_ten_capped_and_sanitized() {
        use serde_json::json;
        let mut records = Vec::new();
        for i in 0..12 {
            records.push(prompt_record(json!(format!("q{i}"))));
            records.push(answer_record(json!(format!("a{i}"))));
        }
        let answers = recent_answers(&jsonl(&records));
        assert_eq!(answers.len(), MAX_ANSWERS);
        assert_eq!(answers[0], answer("q2", "a2"));
        assert_eq!(answers[9], answer("q11", "a11"));

        // A long prompt and a long text are cut on a char boundary.
        let prompt = "p".repeat(MAX_PROMPT_BYTES + 100);
        let text = format!("{}é{}", "t".repeat(MAX_ANSWER_BYTES - 1), "t".repeat(10));
        let answers = recent_answers(&jsonl(&[
            prompt_record(json!(prompt)),
            answer_record(json!(text)),
        ]));
        assert_eq!(
            answers[0].prompt,
            format!("{} …", "p".repeat(MAX_PROMPT_BYTES))
        );
        assert_eq!(
            answers[0].text,
            format!("{} …", "t".repeat(MAX_ANSWER_BYTES - 1))
        );

        // Escapes, bidi controls and private-use characters never leave.
        let dirty = jsonl(&[
            prompt_record(json!("why\u{1b}[2J\u{202e}?")),
            answer_record(json!("because\u{e000}\u{9b}31m\u{200b}.")),
        ]);
        assert_eq!(recent_answers(&dirty), [answer("why[2J?", "because31m.")]);
    }

    #[test]
    fn answers_skip_lines_cut_by_a_tail_read() {
        use serde_json::json;
        let transcript = jsonl(&[
            prompt_record(json!("old question")),
            answer_record(json!("old answer")),
            prompt_record(json!("new question")),
            answer_record(json!("new answer")),
        ]);
        // Cut inside the first line, and inside a multi-byte character of
        // the last one.
        let last = answer_record(json!("cut ééé")).to_string();
        let mut bytes = transcript[10..].to_vec();
        bytes.extend_from_slice(&last.as_bytes()[..last.find('é').unwrap() + 1]);
        assert_eq!(
            recent_answers(&bytes),
            [answer("new question", "new answer")]
        );
        assert!(recent_answers(b"").is_empty());
        assert!(recent_answers(b"not json\n{\"type\":\"user\"").is_empty());
    }

    #[test]
    fn answers_are_read_from_the_tail_of_a_long_transcript() {
        use serde_json::json;
        let dir = std::env::temp_dir().join(format!("claudio-answers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        // An old answer whose text alone is longer than the tail read: its
        // prompt is not read, and the rest of its line is skipped.
        let mut transcript = jsonl(&[
            prompt_record(json!("old")),
            answer_record(json!("x".repeat(ANSWERS_TAIL as usize))),
        ]);
        transcript.extend(jsonl(&[
            prompt_record(json!("new")),
            answer_record(json!("fresh")),
        ]));
        std::fs::write(&path, &transcript).unwrap();
        assert_eq!(read_answers(&path), [answer("new", "fresh")]);
        // A short transcript is read whole.
        std::fs::write(
            &path,
            jsonl(&[prompt_record(json!("q")), answer_record(json!("a"))]),
        )
        .unwrap();
        assert_eq!(read_answers(&path), [answer("q", "a")]);
        assert!(read_answers(&dir.join("missing.jsonl")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── last_prompt ───────────────────────────────────────────────────────

    #[test]
    fn the_last_prompt_is_the_newest_real_one_answered_or_not() {
        use serde_json::json;
        let mut meta = prompt_record(json!("caveat"));
        meta["isMeta"] = json!(true);
        let mut side = prompt_record(json!("a subagent's task"));
        side["isSidechain"] = json!(true);
        let transcript = jsonl(&[
            prompt_record(json!("old question")),
            answer_record(json!("old answer")),
            prompt_record(json!([{"type": "text", "text": "  the newest\nquestion  "}])),
            // Still unanswered: tool calls and their results only.
            answer_record(json!([{"type": "tool_use", "id": "t", "name": "Bash", "input": {}}])),
            prompt_record(json!([{"type": "tool_result", "tool_use_id": "t", "content": "out"}])),
            prompt_record(json!("<command-name>/model</command-name>")),
            meta,
            side,
            json!({"type": "system", "subtype": "compact_boundary"}),
        ]);
        assert_eq!(
            last_prompt(&transcript),
            Some(("the newest\nquestion".into(), false))
        );
        assert_eq!(last_prompt(b""), None);
        assert_eq!(
            last_prompt(&jsonl(&[answer_record(json!("no prompt"))])),
            None
        );
    }

    #[test]
    fn the_last_prompt_is_sanitized_and_cut_without_a_mark() {
        use serde_json::json;
        let dirty = jsonl(&[prompt_record(json!("why\u{1b}[2J\u{202e}?"))]);
        assert_eq!(last_prompt(&dirty), Some(("why[2J?".into(), false)));
        let long = format!("{}é{}", "p".repeat(MAX_LAST_PROMPT - 1), "q".repeat(10));
        let (text, cut) = last_prompt(&jsonl(&[prompt_record(json!(long))])).unwrap();
        assert!(cut);
        assert_eq!(text, "p".repeat(MAX_LAST_PROMPT - 1));
        let exact = "x".repeat(MAX_LAST_PROMPT);
        assert_eq!(
            last_prompt(&jsonl(&[prompt_record(json!(exact.clone()))])),
            Some((exact, false))
        );
    }

    #[test]
    fn the_last_prompt_is_read_from_the_tail_of_a_transcript() {
        use serde_json::json;
        let dir = std::env::temp_dir().join(format!("claudio-last-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let mut transcript = jsonl(&[prompt_record(json!("x".repeat(ANSWERS_TAIL as usize)))]);
        transcript.extend(jsonl(&[answer_record(json!("a"))]));
        std::fs::write(&path, &transcript).unwrap();
        // The only prompt starts before the tail: none is read.
        assert_eq!(read_last_prompt(&path), None);
        transcript.extend(jsonl(&[prompt_record(json!("again"))]));
        std::fs::write(&path, &transcript).unwrap();
        assert_eq!(read_last_prompt(&path), Some(("again".into(), false)));
        assert_eq!(read_last_prompt(&dir.join("missing.jsonl")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cursor_collects_the_items_of_its_tag_lenses_in_order() {
        let mut c = cursor().with_tags(vec!["latex".into(), "keys".into()]);
        let mut jsonl = String::new();
        jsonl += &said(
            r#"[{"type":"thinking","thinking":"<keys>t</keys>"},{"type":"text","text":"press <keys>Ctrl+C</keys> for <latex>a^2</latex>, not <kbd>x</kbd>"}]"#,
        );
        jsonl += &said(r#"[{"type":"text","text":"<keys>side</keys>"}]"#)
            .replacen('{', r#"{"isSidechain":true,"#, 1);
        jsonl += &said(r#""then <keys>Esc</keys>""#);
        c.feed(jsonl.as_bytes());
        let items: Vec<(&str, &str)> = c
            .lens()
            .iter()
            .map(|i| (i.tag.as_str(), i.text.as_str()))
            .collect();
        assert_eq!(items, [("keys", "Ctrl+C"), ("latex", "a^2"), ("keys", "Esc")]);
        // The latex list is kept as before.
        assert_eq!(c.latex().iter().collect::<Vec<_>>(), ["a^2"]);
        // Without tags, no items.
        let mut plain = cursor();
        plain.feed(jsonl.as_bytes());
        assert!(plain.lens().is_empty());
    }

    #[test]
    fn cursor_keeps_the_last_64_lens_items() {
        let mut c = cursor().with_tags(vec!["keys".into()]);
        for i in 0..70 {
            c.feed(said(&format!(r#""<keys>k{i}</keys>""#)).as_bytes());
        }
        assert_eq!(c.lens().len(), 64);
        assert_eq!(c.lens().front().map(|i| i.text.as_str()), Some("k6"));
        assert_eq!(c.lens().back().map(|i| i.text.as_str()), Some("k69"));
    }
}
