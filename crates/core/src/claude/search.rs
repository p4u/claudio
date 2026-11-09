//! Substring search over this host's claude conversations, for the
//! `search` extension (`Msg::Search`).
//!
//! Blocking and bounded: transcripts are streamed line by line through a
//! 64 KiB buffer, a line longer than [`Budget::line`] is skipped without
//! being kept, only the last [`Budget::tail`] bytes of a file are read, and
//! the whole search stops at [`Budget::time`], [`Budget::bytes`] or its hit
//! limit. Only real prompts and claude's answer text are searched (the
//! rules of [`projects::recent_answers`]); never tool output, thinking,
//! Claude Code's own records or subagents. The query is plain text: it never
//! reaches a shell or a regex.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::claude::projects::{answer_prompt, flag, push_texts};
use crate::proto::SearchHit;
use crate::text::sanitize;

/// A query's length after trimming and [`sanitize`], in bytes.
pub const MIN_QUERY: usize = 2;
pub const MAX_QUERY: usize = 200;
/// The most words a query may have; a message must contain all of them.
pub const MAX_TERMS: usize = 8;
/// Hits when the request asks for 0, and the most it may ask for.
pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 100;
/// A hit's title and snippet, in characters.
const TITLE_CHARS: usize = 80;
const SNIPPET_CHARS: usize = 160;
/// Characters of the snippet before the first term.
const SNIPPET_LEAD: usize = 60;
/// A longer `cwd` is not a path claude ran in; it is dropped.
const MAX_CWD: usize = 4096;
/// The read buffer.
const BUFFER: usize = 64 * 1024;

/// When a search stops.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Wall time for the whole search.
    pub time: Duration,
    /// Bytes read, all files together.
    pub bytes: u64,
    /// Bytes read from the end of each file.
    pub tail: u64,
    /// The longest line examined; longer ones are skipped.
    pub line: usize,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            time: Duration::from_secs(3),
            bytes: 1 << 30,
            tail: 64 << 20,
            line: 1 << 20,
        }
    }
}

/// A search's result: hits newest conversation first, how many transcripts
/// were read to the end of `total`, and whether it stopped early.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub hits: Vec<SearchHit>,
    pub scanned: u32,
    pub total: u32,
    pub truncated: bool,
}

/// Why a query is refused; the daemon answers `Error "bad query: <why>"`.
#[derive(Debug, Clone, PartialEq)]
pub struct BadQuery(pub String);

/// The words of `query`, lowercase: trimmed and [`sanitize`]d, it must
/// have [`MIN_QUERY`]..=[`MAX_QUERY`] bytes and at most [`MAX_TERMS`] words.
pub fn terms(query: &str) -> Result<Vec<String>, BadQuery> {
    let query = sanitize(query.trim());
    let query = query.trim();
    if query.len() < MIN_QUERY {
        return Err(BadQuery("too short".into()));
    }
    if query.len() > MAX_QUERY {
        return Err(BadQuery(format!("too long (max {MAX_QUERY} bytes)")));
    }
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.len() > MAX_TERMS {
        return Err(BadQuery(format!("too many words (max {MAX_TERMS})")));
    }
    Ok(terms)
}

/// Search the transcripts `<projects>/*/*.jsonl`, newest first, for
/// messages that contain every word of `query` (case-insensitive). One hit
/// per conversation; at most `limit` hits (0: [`DEFAULT_LIMIT`]; capped at
/// [`MAX_LIMIT`]).
pub fn search(
    projects: &Path,
    query: &str,
    limit: usize,
    budget: Budget,
) -> Result<Outcome, BadQuery> {
    let terms = terms(query)?;
    let limit = match limit {
        0 => DEFAULT_LIMIT,
        n => n.min(MAX_LIMIT),
    };
    let files = transcripts(projects);
    let mut spent = Spent {
        started: Instant::now(),
        bytes: 0,
    };
    let mut out = Outcome {
        hits: Vec::new(),
        scanned: 0,
        total: u32::try_from(files.len()).unwrap_or(u32::MAX),
        truncated: false,
    };
    for (modified, path, conversation) in &files {
        if out.hits.len() >= limit || spent.over(&budget) {
            out.truncated = true;
            break;
        }
        let (found, finished) = scan(path, &terms, &budget, &mut spent);
        if let Some(found) = found {
            out.hits.push(found.hit(conversation, *modified));
        }
        if !finished {
            out.truncated = true;
            break;
        }
        out.scanned += 1;
    }
    Ok(out)
}

/// What was used of the budget.
struct Spent {
    started: Instant,
    bytes: u64,
}

impl Spent {
    fn over(&self, budget: &Budget) -> bool {
        self.bytes >= budget.bytes || self.started.elapsed() >= budget.time
    }
}

/// Every `<root>/<dir>/<id>.jsonl` (no symlinks, no deeper levels) whose
/// `<id>` is a plain conversation id: (mtime, path, id), newest first.
fn transcripts(root: &Path) -> Vec<(u64, PathBuf, String)> {
    let mut files = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root) else {
        return files;
    };
    for dir in dirs.flatten() {
        if !dir.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let id = match (path.extension(), path.file_stem()) {
                (Some(ext), Some(stem)) if ext == "jsonl" => stem.to_string_lossy().into_owned(),
                _ => continue,
            };
            let safe = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
            if id.is_empty() || !id.chars().all(safe) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            files.push((modified, path, id));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files
}

/// What one transcript has for the query.
#[derive(Default)]
struct Found {
    matches: u32,
    /// The newest matching message, whitespace collapsed.
    newest: String,
    title: Option<String>,
    cwd: Option<String>,
}

impl Found {
    fn hit(self, conversation: &str, modified: u64) -> SearchHit {
        SearchHit {
            conversation: conversation.to_owned(),
            cwd: self.cwd.unwrap_or_default(),
            modified,
            title: self.title.unwrap_or_default(),
            snippet: self.newest,
            matches: self.matches,
        }
    }
}

/// Scan the tail of the transcript at `path`. Returns what matched (if
/// anything did) and whether the file was read to its end within the
/// budget. An unreadable file is read to its end, with nothing in it.
fn scan(
    path: &Path,
    terms: &[String],
    budget: &Budget,
    spent: &mut Spent,
) -> (Option<Found>, bool) {
    let Ok(mut file) = File::open(path) else {
        return (None, true);
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let start = len.saturating_sub(budget.tail);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return (None, true);
    }
    let mut reader = BufReader::with_capacity(BUFFER, file.take(budget.tail));
    // ASCII words can be looked for in the raw line first: JSON leaves
    // them as they are. Lines without them are not parsed.
    let ascii: Vec<&[u8]> = (terms.iter())
        .filter(|t| {
            t.bytes()
                .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
        })
        .map(|t| t.as_bytes())
        .collect();
    let mut found = Found::default();
    let (mut line, mut lower) = (Vec::new(), Vec::new());
    // A tail starts inside a line: that line is skipped.
    let mut partial = start > 0;
    let mut finished = true;
    loop {
        if spent.over(budget) {
            // Out of budget, unless there was nothing left anyway.
            finished = reader.fill_buf().map_or(true, |rest| rest.is_empty());
            break;
        }
        let Ok((used, fits)) = read_line(&mut reader, &mut line, budget.line) else {
            break;
        };
        if used == 0 {
            break;
        }
        spent.bytes += used as u64;
        if std::mem::take(&mut partial) || !fits {
            continue;
        }
        let wanted = contains(&line, b"\"user\"") || contains(&line, b"\"assistant\"");
        let meta_missing = found.title.is_none() || found.cwd.is_none();
        if !wanted {
            continue;
        }
        if !meta_missing {
            lower.clear();
            lower.extend(line.iter().map(u8::to_ascii_lowercase));
            if !ascii.iter().all(|t| contains(&lower, t)) {
                continue;
            }
        }
        examine(&line, terms, &mut found);
    }
    ((found.matches > 0).then_some(found), finished)
}

/// Read the next line into `line` (without its `\n`), unless it is longer
/// than `max`: then its bytes are read and dropped, never kept. Returns the
/// bytes read (0 at the end) and whether `line` holds the line.
fn read_line(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    max: usize,
) -> io::Result<(usize, bool)> {
    line.clear();
    let (mut used, mut fits) = (0, true);
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok((used, fits));
        }
        let newline = buf.iter().position(|&b| b == b'\n');
        let chunk = &buf[..newline.unwrap_or(buf.len())];
        if fits && line.len() + chunk.len() > max {
            fits = false;
            line.clear();
        } else if fits {
            line.extend_from_slice(chunk);
        }
        let n = newline.map_or(chunk.len(), |i| i + 1);
        reader.consume(n);
        used += n;
        if newline.is_some() {
            return Ok((used, fits));
        }
    }
}

/// Whether `haystack` contains `needle`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// Fold one transcript record into `found`.
fn examine(line: &[u8], terms: &[String], found: &mut Found) {
    let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
        return;
    };
    if flag(&record, "isSidechain") {
        return;
    }
    if found.cwd.is_none() {
        let cwd = record.get("cwd").and_then(|c| c.as_str());
        found.cwd = cwd
            .map(sanitize)
            .filter(|c| c.len() <= MAX_CWD && !c.contains('\n'));
    }
    let text = match record.get("type").and_then(|t| t.as_str()) {
        Some("user") => {
            let prompt = answer_prompt(&record);
            if found.title.is_none() {
                found.title = prompt
                    .as_deref()
                    .map(|p| one_line(&sanitize(p), TITLE_CHARS));
            }
            prompt
        }
        Some("assistant") if !flag(&record, "isApiErrorMessage") => {
            let mut texts = Vec::new();
            push_texts(
                &mut texts,
                record.get("message").and_then(|m| m.get("content")),
            );
            Some(texts.join("\n\n"))
        }
        _ => None,
    };
    let Some(text) = text else { return };
    let text = sanitize(&text);
    let lower = text.to_lowercase();
    if terms.iter().all(|t| lower.contains(t.as_str())) {
        found.matches = found.matches.saturating_add(1);
        found.newest = snippet(&text, &terms[0]);
    }
}

/// `text` on one line (whitespace runs as one space), cut to `max`
/// characters with `…`.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// At most [`SNIPPET_CHARS`] characters of `text` on one line, around the
/// first place `term` (lowercase) appears, `…` where cut.
fn snippet(text: &str, term: &str) -> String {
    let flat: Vec<char> = one_line(text, usize::MAX).chars().collect();
    if flat.len() <= SNIPPET_CHARS {
        return flat.into_iter().collect();
    }
    let lower: String = flat.iter().collect::<String>().to_lowercase();
    // Lowercasing can change a character's length: the position is a
    // character count of the lowercase text, close enough for a window.
    let at = lower.find(term).map_or(0, |i| lower[..i].chars().count());
    let start = at
        .saturating_sub(SNIPPET_LEAD)
        .min(flat.len() - SNIPPET_CHARS);
    let end = start + SNIPPET_CHARS;
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    let from = start + usize::from(start > 0);
    let to = end - usize::from(end < flat.len());
    out.extend(&flat[from..to]);
    if end < flat.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A fresh projects root with one project dir.
    fn root() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("claudio-search-{}", uuid::Uuid::new_v4()));
        let project = root.join("-home-u-p");
        std::fs::create_dir_all(&project).unwrap();
        (root, project)
    }

    fn prompt(text: &str) -> serde_json::Value {
        json!({"type": "user", "cwd": "/home/u/p", "message": {"role": "user", "content": text}})
    }

    fn answer(text: &str) -> serde_json::Value {
        json!({"type": "assistant", "message": {"role": "assistant",
            "content": [{"type": "text", "text": text}]}})
    }

    /// Write transcript `<id>.jsonl` with `records`, modified `age` seconds
    /// before a fixed time.
    fn transcript(dir: &Path, id: &str, age: u64, records: &[serde_json::Value]) -> PathBuf {
        let path = dir.join(format!("{id}.jsonl"));
        let mut bytes = Vec::new();
        for r in records {
            bytes.extend_from_slice(r.to_string().as_bytes());
            bytes.push(b'\n');
        }
        std::fs::write(&path, bytes).unwrap();
        let when = UNIX_EPOCH + Duration::from_secs(2_000_000_000 - age);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
        path
    }

    fn find(root: &Path, query: &str) -> Outcome {
        search(root, query, 0, Budget::default()).unwrap()
    }

    #[test]
    fn queries_are_trimmed_sanitized_and_bounded() {
        assert_eq!(terms("  Foo BAR "), Ok(vec!["foo".into(), "bar".into()]));
        assert_eq!(terms("a\u{1b}"), Err(BadQuery("too short".into())));
        assert_eq!(terms(" x "), Err(BadQuery("too short".into())));
        assert!(terms(&"x".repeat(MAX_QUERY)).is_ok());
        assert_eq!(
            terms(&"x".repeat(MAX_QUERY + 1)),
            Err(BadQuery("too long (max 200 bytes)".into()))
        );
        assert_eq!(
            terms("a b c d e f g h i"),
            Err(BadQuery("too many words (max 8)".into()))
        );
        assert_eq!(terms("\u{1b}[2Jab"), Ok(vec!["[2jab".into()]));
    }

    #[test]
    fn every_word_matches_in_any_case_and_one_hit_per_conversation() {
        let (root, dir) = root();
        transcript(
            &dir,
            "c1",
            10,
            &[
                prompt("How do I parse TOML in Rust?"),
                answer("Use the toml crate."),
                prompt("and write toml?"),
                answer("Rust's toml crate writes it too: `toml::to_string`."),
            ],
        );
        transcript(
            &dir,
            "c2",
            5,
            &[prompt("only rust here"), answer("Ünïcode ÉCOLE")],
        );
        let out = find(&root, "rust TOML");
        assert_eq!((out.scanned, out.total, out.truncated), (2, 2, false));
        assert_eq!(out.hits.len(), 1);
        let hit = &out.hits[0];
        assert_eq!(hit.conversation, "c1");
        assert_eq!(hit.matches, 2);
        assert_eq!(hit.title, "How do I parse TOML in Rust?");
        assert_eq!(hit.cwd, "/home/u/p");
        assert_eq!(
            hit.snippet,
            "Rust's toml crate writes it too: `toml::to_string`."
        );
        assert_eq!(hit.modified, 2_000_000_000 - 10);
        // Unicode lowercase on both sides.
        assert_eq!(find(&root, "école").hits[0].conversation, "c2");
        assert!(find(&root, "rust python").hits.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tool_output_thinking_and_claude_codes_own_records_are_not_searched() {
        let (root, dir) = root();
        let mut meta = prompt("secret in meta");
        meta["isMeta"] = json!(true);
        let mut side = answer("secret in a subagent");
        side["isSidechain"] = json!(true);
        let mut api = answer("secret in an api error");
        api["isApiErrorMessage"] = json!(true);
        transcript(
            &dir,
            "c1",
            1,
            &[
                prompt("the question"),
                json!({"type": "assistant", "message": {"content": [
                    {"type": "thinking", "thinking": "secret in thinking"},
                    {"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": "secret"}},
                ]}}),
                json!({"type": "user", "message": {"content": [
                    {"type": "tool_result", "tool_use_id": "t", "content": "secret in a tool result"},
                ]}}),
                prompt("<command-name>/secret</command-name>"),
                meta,
                side,
                api,
                json!({"type": "summary", "summary": "secret summary"}),
            ],
        );
        assert!(find(&root, "secret").hits.is_empty());
        assert_eq!(find(&root, "question").hits.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn newest_first_and_only_plain_transcripts() {
        let (root, dir) = root();
        transcript(&dir, "old", 100, &[prompt("needle old")]);
        transcript(&dir, "new", 1, &[prompt("needle new")]);
        let other = root.join("-home-u-q");
        std::fs::create_dir_all(other.join("sub")).unwrap();
        transcript(&other, "mid", 50, &[prompt("needle mid")]);
        // Deeper levels, odd names, other extensions and symlinks are not read.
        transcript(&other.join("sub"), "deep", 0, &[prompt("needle deep")]);
        transcript(&other, "bad name", 0, &[prompt("needle bad")]);
        std::fs::write(other.join("notes.txt"), "needle").unwrap();
        std::os::unix::fs::symlink(dir.join("new.jsonl"), other.join("link.jsonl")).unwrap();
        let out = find(&root, "needle");
        let ids: Vec<&str> = out.hits.iter().map(|h| h.conversation.as_str()).collect();
        assert_eq!(ids, ["new", "mid", "old"]);
        assert_eq!(out.total, 3);
        assert!(
            search(&root.join("missing"), "needle", 0, Budget::default())
                .unwrap()
                .hits
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_budget_and_the_limit_stop_a_search() {
        let (root, dir) = root();
        for i in 0..3 {
            transcript(
                &dir,
                &format!("c{i}"),
                i,
                &[prompt(&format!("needle {i} {}", "x".repeat(100)))],
            );
        }
        let limited = search(&root, "needle", 2, Budget::default()).unwrap();
        assert_eq!(
            (limited.hits.len(), limited.scanned, limited.truncated),
            (2, 2, true)
        );
        let exact = search(&root, "needle", 3, Budget::default()).unwrap();
        assert_eq!((exact.hits.len(), exact.truncated), (3, false));
        // Bytes: the first file alone uses it up.
        let bytes = Budget {
            bytes: 10,
            ..Budget::default()
        };
        let out = search(&root, "needle", 0, bytes).unwrap();
        assert_eq!((out.scanned, out.total, out.truncated), (1, 3, true));
        // Time.
        let time = Budget {
            time: Duration::ZERO,
            ..Budget::default()
        };
        let out = search(&root, "needle", 0, time).unwrap();
        assert_eq!((out.hits.len(), out.scanned, out.truncated), (0, 0, true));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn long_lines_are_skipped_and_only_the_tail_is_read() {
        let (root, dir) = root();
        let budget = Budget {
            tail: 4096,
            line: 1024,
            ..Budget::default()
        };
        transcript(
            &dir,
            "c1",
            1,
            &[
                prompt("needle early"),
                answer(&"y".repeat(5000)),
                answer(&format!("needle in a long line {}", "z".repeat(2000))),
                prompt("needle late"),
            ],
        );
        let out = search(&root, "needle", 0, budget).unwrap();
        assert_eq!(out.hits[0].matches, 1);
        assert_eq!(out.hits[0].snippet, "needle late");
        // The title is the first prompt that was read.
        assert_eq!(out.hits[0].title, "needle late");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn snippets_are_sanitized_one_line_and_cut_around_the_word() {
        let (root, dir) = root();
        let long = format!(
            "{} the needle\n\tis \u{1b}[31mhere {}",
            "a ".repeat(100),
            "b ".repeat(100)
        );
        transcript(&dir, "c1", 1, &[prompt("q\u{202e}?"), answer(&long)]);
        let hit = &find(&root, "needle").hits[0];
        assert_eq!(hit.title, "q?");
        assert_eq!(hit.snippet.chars().count(), SNIPPET_CHARS);
        assert!(
            hit.snippet.starts_with('…') && hit.snippet.ends_with('…'),
            "{}",
            hit.snippet
        );
        assert!(
            hit.snippet.contains("the needle is [31mhere"),
            "{}",
            hit.snippet
        );
        assert!(!hit.snippet.chars().any(char::is_control));
        let _ = std::fs::remove_dir_all(&root);
    }
}
