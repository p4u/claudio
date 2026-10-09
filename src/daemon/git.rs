//! Read-only git queries for the TUI's git viewer, run on the session's host.
//!
//! Three requests: [`Msg::GitLog`] (a page of history), [`Msg::GitCommit`] (one
//! commit's metadata and file list) and [`Msg::GitDiff`] (a patch). [`handle`]
//! executes one of them and always returns the reply `Msg`, an `Error`
//! included.
//!
//! The repository is untrusted input (a commit subject can carry escape
//! sequences, a repo config can name a diff driver), so:
//!
//! - git runs with a scrubbed environment and options that disable pagers,
//!   external diff drivers and textconv ([`command`]);
//! - object ids are full hashes and paths come after `--`;
//! - output is read through size caps, then every piece of text is stripped of
//!   control characters ([`sanitize`]) and capped;
//! - the reply is measured against [`proto::MAX_FRAME`] and the patch trimmed
//!   at a line boundary to fit.
//!
//! Execution (`run`) is kept apart from the pure parsers, which carry the unit
//! tests.

use std::future::Future;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::OnceCell;

use super::host::expand_tilde;
use crate::proto::{
    self, GitCommitInfo, GitFile, GitLogEntry, GitLogPage, GitPatch, Msg, GIT_LOG_MAX,
};

/// Deadline for one whole request (all its git processes).
const DEADLINE: Duration = Duration::from_secs(10);

/// Stdout caps. Output beyond them is dropped and the process killed.
const PATCH_CAP: usize = 384 * 1024;
const META_CAP: usize = 256 * 1024;
const STDERR_CAP: usize = 4096;

/// Space kept free in a frame for the envelope around the payload.
const ENVELOPE_SLACK: usize = 4096;

/// Caps on variable fields, in characters unless noted.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_SUBJECT: usize = 300;
const MAX_NAME: usize = 100;
const MAX_REF: usize = 200;
const MAX_REFS: usize = 32;
const MAX_PARENTS: usize = 16;
const MAX_PATH: usize = 1024;
const MAX_FILES: usize = 2000;
const MAX_ERROR: usize = 300;

/// Tab stops in sanitized text.
const TAB_WIDTH: usize = 4;

/// Fields per record of the log format.
const LOG_FIELDS: usize = 6;
const LOG_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%at%x00%D%x00%s";
const COMMIT_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%ct%x00%D%x00%B";
const COMMIT_FIELDS: usize = 8;

/// Environment variables that would redirect git to another repository, add a
/// pager or inject configuration.
const SCRUBBED_ENV: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
    "GIT_CONFIG_PARAMETERS",
    "GIT_PAGER",
    "PAGER",
];

/// Arguments in front of every git subcommand.
const BASE_ARGS: [&str; 16] = [
    "--no-pager",
    "--literal-pathspecs",
    "-c",
    "core.quotePath=false",
    "-c",
    "diff.noprefix=false",
    "-c",
    "diff.mnemonicPrefix=false",
    "-c",
    "diff.submodule=short",
    "-c",
    "log.showSignature=false",
    "-c",
    "color.ui=never",
    "-c",
    "core.fsmonitor=false",
];

/// Arguments for subcommands that print diffs.
const DIFF_ARGS: [&str; 3] = ["--no-ext-diff", "--no-textconv", "-M"];

// ── Entry point ───────────────────────────────────────────────────────────────

/// Execute a git request and return its reply (`Msg::Error` on failure).
/// It starts once `turn` resolves (e.g. to a concurrency permit, held until
/// the reply is ready); the deadline counts from the call, waiting included.
pub async fn handle<T>(request: Msg, turn: impl Future<Output = T>) -> Msg {
    let work = async {
        let _turn = turn.await;
        match request {
            Msg::GitLog {
                cwd,
                all,
                skip,
                limit,
            } => log(&cwd, all, skip, limit).await,
            Msg::GitCommit { cwd, id } => commit(&cwd, &id).await,
            Msg::GitDiff {
                cwd,
                id,
                path,
                old_path,
            } => diff(&cwd, &id, path, old_path).await,
            _ => Err("not a git request".to_owned()),
        }
    };
    let reply = match tokio::time::timeout(DEADLINE, work).await {
        Ok(reply) => reply.and_then(fits_frame),
        Err(_) => Err(format!("git timed out after {} s", DEADLINE.as_secs())),
    };
    reply.unwrap_or_else(|message| Msg::Error { message })
}

async fn log(cwd: &str, all: bool, skip: u32, limit: u32) -> Result<Msg, String> {
    let cwd = work_dir(cwd).await?;
    let limit = limit.clamp(1, GIT_LOG_MAX) as usize;
    // Asking for one extra commit tells us whether another page exists.
    let (skip, count) = (format!("--skip={skip}"), format!("-n{}", limit + 1));
    let mut log_args = vec!["log", "--no-color", "--decorate=full", "-z", LOG_FORMAT];
    if all {
        log_args.push("--all");
    }
    log_args.extend([skip.as_str(), count.as_str()]);

    // Not being in a repository is the likeliest failure; report it first.
    let root = run(&cwd, &["rev-parse", "--show-toplevel"], META_CAP).await?;
    let head_args = ["symbolic-ref", "--short", "-q", "HEAD"];
    let (log, head) = tokio::join!(
        run(&cwd, &log_args, META_CAP),
        run(&cwd, &head_args, STDERR_CAP)
    );
    let log = log?;
    let mut commits = parse_log(&log.stdout);
    let more = log.truncated || commits.len() > limit;
    commits.truncate(limit);
    Ok(Msg::GitLogPage(GitLogPage {
        root: clean_line(root.stdout.trim_ascii(), MAX_PATH),
        // Detached HEAD makes `symbolic-ref -q` fail: no branch.
        head: head
            .ok()
            .map(|h| clean_line(h.stdout.trim_ascii(), MAX_NAME))
            .filter(|h| !h.is_empty()),
        commits,
        more,
    }))
}

async fn commit(cwd: &str, id: &str) -> Result<Msg, String> {
    let cwd = work_dir(cwd).await?;
    check_id(id)?;
    let meta_args = [
        "show",
        "-s",
        "--decorate=full",
        COMMIT_FORMAT,
        "--end-of-options",
        id,
    ];
    let stat_args = diff_args(&["show", "--format=", "--numstat", "-z"], id).await?;
    let (meta, stat) = tokio::try_join!(
        run(&cwd, &meta_args, META_CAP),
        run(&cwd, &stat_args, META_CAP)
    )?;
    let files = parse_numstat(&stat.stdout);
    let info = parse_commit(&meta.stdout, files).ok_or("unexpected output from git show")?;
    Ok(Msg::GitCommitInfo(info))
}

async fn diff(
    cwd: &str,
    id: &str,
    path: Option<String>,
    old_path: Option<String>,
) -> Result<Msg, String> {
    let cwd = work_dir(cwd).await?;
    check_id(id)?;
    let paths: Vec<&str> = path.iter().chain(&old_path).map(String::as_str).collect();
    // `old_path` alone is meaningless: the pair exists to show a rename.
    let paths = if path.is_some() { &paths[..] } else { &[] };
    paths.iter().try_for_each(|p| check_path(p))?;

    let mut args = diff_args(&["show", "--format=", "--patch"], id).await?;
    if !paths.is_empty() {
        args.push("--");
        args.extend(paths);
    }
    let out = run(&cwd, &args, PATCH_CAP).await?;

    let mut text = sanitize(&String::from_utf8_lossy(&out.stdout), true);
    let mut truncated = out.truncated;
    if truncated {
        // The last line was cut mid-way.
        text.truncate(text.rfind('\n').map_or(0, |i| i + 1));
    }
    let mut reply = GitPatch {
        id: id.to_owned(),
        path: path.map(|p| clip(p, MAX_PATH)),
        patch: String::new(),
        truncated: true,
    };
    let budget = proto::MAX_FRAME - ENVELOPE_SLACK - json_len(&Msg::GitPatch(reply.clone()));
    let fitted = fit_lines(&text, budget);
    truncated |= fitted.len() < text.len();
    reply.patch = fitted.to_owned();
    reply.truncated = truncated;
    Ok(Msg::GitPatch(reply))
}

/// `show`-style arguments for `id`'s diff against its first parent.
async fn diff_args<'a>(head: &[&'a str], id: &'a str) -> Result<Vec<&'a str>, String> {
    let merges: &[&str] = if diff_merges_supported().await? {
        &["--diff-merges=first-parent"]
    } else {
        &["-m", "--first-parent"]
    };
    let mut args = head.to_vec();
    args.extend(DIFF_ARGS);
    args.extend(merges);
    args.extend(["--end-of-options", id]);
    Ok(args)
}

/// Whether this host's git has `--diff-merges` (2.31+), probed once.
async fn diff_merges_supported() -> Result<bool, String> {
    static SUPPORTED: OnceCell<bool> = OnceCell::const_new();
    SUPPORTED
        .get_or_try_init(|| async {
            let out = run(Path::new("."), &["--version"], STDERR_CAP).await?;
            Ok(supports_diff_merges(&String::from_utf8_lossy(&out.stdout)))
        })
        .await
        .copied()
}

/// `git version 2.53.0` → whether it is at least 2.31. Unparseable counts as new.
fn supports_diff_merges(version_line: &str) -> bool {
    let version = version_line.trim().trim_start_matches("git version ");
    let mut parts = version.split('.').map(|p| p.parse::<u32>());
    match (parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => (major, minor) >= (2, 31),
        _ => true,
    }
}

// ── Execution ─────────────────────────────────────────────────────────────────

/// What a git process printed on stdout.
struct Captured {
    stdout: Vec<u8>,
    /// More output existed than the cap allowed; the process was killed.
    truncated: bool,
}

/// The directory to run in; `~` is expanded and it must exist.
async fn work_dir(cwd: &str) -> Result<std::path::PathBuf, String> {
    let dir = expand_tilde(cwd);
    match tokio::fs::metadata(&dir).await {
        Ok(meta) if meta.is_dir() => Ok(dir),
        _ => Err(format!(
            "no such directory: {}",
            clip(cwd.to_owned(), MAX_PATH)
        )),
    }
}

fn command(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    for var in SCRUBBED_ENV {
        cmd.env_remove(var);
    }
    cmd.env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .current_dir(cwd)
        .args(BASE_ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// Run `git <args>` in `cwd`, keeping at most `cap` bytes of stdout.
async fn run(cwd: &Path, args: &[&str], cap: usize) -> Result<Captured, String> {
    let mut child = command(cwd)
        .args(args)
        .spawn()
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => "git is not installed on this host".to_owned(),
            _ => format!("cannot run git: {e}"),
        })?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let stderr = tokio::spawn(read_capped(stderr, STDERR_CAP));

    let mut out = read_capped(stdout, cap + 1)
        .await
        .map_err(|e| format!("cannot read git output: {e}"))?;
    let truncated = out.len() > cap;
    if truncated {
        out.truncate(cap);
        // Git is blocked writing to a pipe nobody reads any more.
        let _ = child.start_kill();
    }
    let status = child.wait().await.map_err(|e| format!("git failed: {e}"))?;
    if !truncated && !status.success() {
        let stderr = stderr.await.ok().and_then(Result::ok).unwrap_or_default();
        return Err(error_line(&stderr, &status.to_string()));
    }
    Ok(Captured {
        stdout: out,
        truncated,
    })
}

/// Read up to `limit` bytes; the rest of the stream is left unread.
async fn read_capped(reader: impl AsyncRead + Unpin, limit: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader.take(limit as u64).read_to_end(&mut buf).await?;
    Ok(buf)
}

/// The first line of git's stderr, without the `fatal: ` prefix.
fn error_line(stderr: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    let line = line.map(|l| l.strip_prefix("fatal: ").unwrap_or(l));
    match line {
        Some(line) => clean_line(line.as_bytes(), MAX_ERROR),
        None => format!("git exited with {fallback}"),
    }
}

// ── Validation ────────────────────────────────────────────────────────────────

/// Whether `s` is a full object id (SHA-1 or SHA-256), lowercase hex.
pub fn is_object_id(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn check_id(id: &str) -> Result<(), String> {
    if is_object_id(id) {
        Ok(())
    } else {
        Err("invalid commit id (a full hash is required)".to_owned())
    }
}

fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > MAX_PATH || path.contains('\0') {
        Err("invalid path".to_owned())
    } else {
        Ok(())
    }
}

// ── Sizes ─────────────────────────────────────────────────────────────────────

/// `Ok(msg)` when its encoding fits one frame, else a bounded error.
fn fits_frame(msg: Msg) -> Result<Msg, String> {
    if json_len(&msg) + ENVELOPE_SLACK <= proto::MAX_FRAME {
        Ok(msg)
    } else {
        Err("result too large to send".to_owned())
    }
}

/// The JSON-encoded size of `value`, without building it.
fn json_len<T: serde::Serialize + ?Sized>(value: &T) -> usize {
    struct Counter(usize);
    impl io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    // Serializing into a counter cannot fail.
    serde_json::to_writer(&mut counter, value).expect("serialize");
    counter.0
}

/// The longest run of whole lines of `text` whose JSON-escaped size fits `budget`.
fn fit_lines(text: &str, budget: usize) -> &str {
    let mut used = 0;
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        used += json_len(line) - 2; // the surrounding quotes
        if used > budget {
            break;
        }
        end += line.len();
    }
    &text[..end]
}

// ── Sanitizing ────────────────────────────────────────────────────────────────

/// Make git-sourced text safe to print on a terminal: drop control characters
/// (C0, DEL, C1, so no escape sequences), expand tabs, and keep `\n` only when
/// `multiline`.
fn sanitize(text: &str, multiline: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut col = 0;
    for c in text.chars() {
        match c {
            '\n' if multiline => {
                out.push('\n');
                col = 0;
            }
            '\t' => {
                let pad = TAB_WIDTH - col % TAB_WIDTH;
                out.extend(std::iter::repeat_n(' ', pad));
                col += pad;
            }
            c if c.is_control() => {}
            c => {
                out.push(c);
                col += 1;
            }
        }
    }
    out
}

/// `text` cut to `max` characters, ending in `…` when shortened.
fn clip(text: String, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

/// A sanitized single-line field, capped at `max` characters.
fn clean_line(bytes: &[u8], max: usize) -> String {
    clip(sanitize(&String::from_utf8_lossy(bytes), false), max)
}

/// A sanitized multi-line message: trailing blanks dropped, capped in bytes.
fn clean_message(bytes: &[u8]) -> String {
    let mut text = sanitize(&String::from_utf8_lossy(bytes), true);
    text.truncate(text.trim_end().len());
    if text.len() > MAX_MESSAGE_BYTES {
        let mut cut = MAX_MESSAGE_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push('…');
    }
    text
}

// ── Parsers ───────────────────────────────────────────────────────────────────

/// Split NUL-terminated output into its fields, dropping the last piece: it is
/// empty after a final NUL, and a fragment when the output was cut short.
fn nul_fields(data: &[u8]) -> Vec<&[u8]> {
    let mut fields: Vec<&[u8]> = data.split(|&b| b == 0).collect();
    fields.pop();
    fields
}

fn object_id(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|s| is_object_id(s))
        .map(str::to_owned)
}

/// Parse `git log -z` output in [`LOG_FORMAT`]. Records are fixed groups of
/// [`LOG_FIELDS`] fields; one that fails validation is skipped by a single
/// field, which resynchronizes after any damage.
fn parse_log(data: &[u8]) -> Vec<GitLogEntry> {
    let fields = nul_fields(data);
    let mut entries = Vec::new();
    let mut at = 0;
    while let Some(group) = fields.get(at..at + LOG_FIELDS) {
        match log_entry(group) {
            Some(entry) => {
                entries.push(entry);
                at += LOG_FIELDS;
            }
            None => at += 1,
        }
    }
    entries
}

fn log_entry(f: &[&[u8]]) -> Option<GitLogEntry> {
    Some(GitLogEntry {
        id: object_id(f[0])?,
        parents: parse_parents(f[1])?,
        author: clean_line(f[2], MAX_NAME),
        time: parse_time(f[3])?,
        refs: parse_refs(f[4]),
        subject: clean_line(f[5], MAX_SUBJECT),
    })
}

fn parse_parents(field: &[u8]) -> Option<Vec<String>> {
    field
        .split(|&b| b == b' ')
        .filter(|p| !p.is_empty())
        .take(MAX_PARENTS)
        .map(object_id)
        .collect()
}

fn parse_time(field: &[u8]) -> Option<i64> {
    std::str::from_utf8(field).ok()?.parse().ok()
}

/// `%D` with `--decorate=full`: `HEAD -> refs/heads/main, tag: refs/tags/v1`.
/// Ref names cannot contain a space, so `", "` separates unambiguously.
fn parse_refs(field: &[u8]) -> Vec<String> {
    sanitize(&String::from_utf8_lossy(field), false)
        .split(", ")
        .filter(|r| !r.is_empty())
        .take(MAX_REFS)
        .map(|r| clip(r.to_owned(), MAX_REF))
        .collect()
}

/// Parse `show -s` output in [`COMMIT_FORMAT`].
fn parse_commit(data: &[u8], files: Vec<GitFile>) -> Option<GitCommitInfo> {
    let f: Vec<&[u8]> = data.splitn(COMMIT_FIELDS, |&b| b == 0).collect();
    let [id, parents, author, email, time, committer_time, refs, message] = f[..] else {
        return None;
    };
    let parents = parse_parents(parents)?;
    Some(GitCommitInfo {
        id: object_id(id)?,
        first_parent: parents.len() > 1,
        parents,
        author: clean_line(author, MAX_NAME),
        email: clean_line(email, MAX_NAME),
        time: parse_time(time)?,
        committer_time: parse_time(committer_time)?,
        refs: parse_refs(refs),
        message: clean_message(message),
        files,
    })
}

/// Parse `--numstat -z`: `added\tremoved\tpath\0`, or for a rename
/// `added\tremoved\t\0old\0new\0`. Binary files count `-`.
fn parse_numstat(data: &[u8]) -> Vec<GitFile> {
    let mut fields = nul_fields(data).into_iter();
    let mut files = Vec::new();
    while let Some(record) = fields.next() {
        if files.len() == MAX_FILES {
            break;
        }
        if let Some(file) = numstat_file(record, &mut fields) {
            files.push(file);
        }
    }
    files
}

fn numstat_file<'a>(record: &[u8], rest: &mut impl Iterator<Item = &'a [u8]>) -> Option<GitFile> {
    let mut parts = record.splitn(3, |&b| b == b'\t');
    let (added, removed, path) = (parts.next()?, parts.next()?, parts.next()?);
    let (path, old_path) = if path.is_empty() {
        let old = rest.next()?;
        (rest.next()?, Some(old))
    } else {
        (path, None)
    };
    let count = |field: &[u8]| std::str::from_utf8(field).ok()?.parse().ok();
    Some(GitFile {
        path: clean_line(path, MAX_PATH),
        old_path: old_path.map(|p| clean_line(p, MAX_PATH)),
        added: count(added),
        removed: count(removed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccc";

    /// A log record as git prints it (`-z`: every field ends in NUL).
    fn record(id: &str, parents: &str, author: &str, refs: &str, subject: &str) -> Vec<u8> {
        [id, parents, author, "1700000000", refs, subject]
            .iter()
            .flat_map(|f| f.bytes().chain([0]))
            .collect()
    }

    #[test]
    fn log_records_parse_with_refs_and_parents() {
        let mut data = record(
            A,
            &format!("{B} {C}"),
            "Ann",
            "HEAD -> refs/heads/main, tag: refs/tags/v1",
            "merge",
        );
        data.extend(record(B, "", "Bob", "", "root"));
        let entries = parse_log(&data);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, A);
        assert_eq!(entries[0].parents, [B, C]);
        assert_eq!(entries[0].time, 1_700_000_000);
        assert_eq!(
            entries[0].refs,
            ["HEAD -> refs/heads/main", "tag: refs/tags/v1"]
        );
        assert_eq!(entries[1].parents, Vec::<String>::new());
        assert!(entries[1].refs.is_empty());
        assert_eq!(entries[1].subject, "root");
    }

    #[test]
    fn hostile_text_is_sanitized() {
        let data = record(
            A,
            "",
            "A\x1b]0;x\x07nn",
            "",
            "evil \x1b[2J\u{9b}\ttab\r\u{7f}end",
        );
        let entry = &parse_log(&data)[0];
        assert_eq!(entry.author, "A]0;xnn");
        assert_eq!(entry.subject, "evil [2J    tabend");
        assert!(!entry.subject.chars().any(char::is_control));
    }

    #[test]
    fn garbled_log_resynchronizes_and_cut_output_drops_the_fragment() {
        let mut data = b"junk\0more junk\0".to_vec();
        data.extend(record(A, "", "Ann", "", "one"));
        data.extend(b"not an id\0"); // a stray field between records
        data.extend(record(B, "", "Bob", "", "two"));
        // Output cut mid-subject: no terminating NUL on the last field.
        let mut cut = record(C, "", "Cy", "", "three");
        cut.pop();
        data.extend(cut);
        let ids: Vec<_> = parse_log(&data).into_iter().map(|e| e.id).collect();
        assert_eq!(ids, [A, B]);
    }

    #[test]
    fn caps_apply_to_every_variable_field() {
        let refs = (0..100)
            .map(|i| format!("refs/tags/t{i}"))
            .collect::<Vec<_>>();
        let data = record(A, "", &"n".repeat(500), &refs.join(", "), &"s".repeat(2000));
        let entry = &parse_log(&data)[0];
        assert_eq!(entry.author.chars().count(), MAX_NAME + 1);
        assert_eq!(entry.subject.chars().count(), MAX_SUBJECT + 1);
        assert_eq!(entry.refs.len(), MAX_REFS);
    }

    #[test]
    fn commit_metadata_parses() {
        let data = format!(
            "{A}\0{B} {C}\0Ann\0ann@x.org\01700000000\01700000100\0HEAD -> refs/heads/main\0subject\n\nbody\n\n"
        );
        let files = vec![GitFile {
            path: "f".into(),
            old_path: None,
            added: Some(1),
            removed: Some(0),
        }];
        let info = parse_commit(data.as_bytes(), files.clone()).unwrap();
        assert_eq!((info.id.as_str(), info.parents.len()), (A, 2));
        assert!(info.first_parent);
        assert_eq!(info.email, "ann@x.org");
        assert_eq!(info.committer_time, 1_700_000_100);
        assert_eq!(info.message, "subject\n\nbody");
        assert_eq!(info.files, files);
        assert!(parse_commit(b"short\0output", vec![]).is_none());
    }

    #[test]
    fn numstat_handles_renames_binaries_and_odd_names() {
        let data = b"3\t1\tsrc/a.rs\0-\t-\tlogo.png\0\
                     5\t5\t\0old name.txt\0new\tname.txt\0\
                     0\t7\tgone.txt\0";
        let files = parse_numstat(data);
        assert_eq!(files.len(), 4);
        assert_eq!((files[0].added, files[0].removed), (Some(3), Some(1)));
        assert_eq!((files[1].added, files[1].removed), (None, None));
        assert_eq!(files[2].old_path.as_deref(), Some("old name.txt"));
        assert_eq!(files[2].path, "new name.txt", "tab expanded");
        assert_eq!(files[3].path, "gone.txt");
    }

    #[test]
    fn numstat_drops_a_record_cut_by_the_size_cap() {
        let files = parse_numstat(b"1\t1\ta\0\0\t0\t");
        assert_eq!(files.len(), 1);
        let files = parse_numstat(b"1\t1\ta\02\t2\t\0old\0");
        assert_eq!(files.len(), 1, "rename missing its new name is dropped");
    }

    #[test]
    fn numstat_caps_the_file_count() {
        let data: Vec<u8> = (0..MAX_FILES + 50)
            .flat_map(|i| format!("1\t1\tf{i}\0").into_bytes())
            .collect();
        assert_eq!(parse_numstat(&data).len(), MAX_FILES);
    }

    #[test]
    fn only_full_lowercase_hashes_are_ids() {
        assert!(is_object_id(A));
        assert!(is_object_id(&"0".repeat(64)));
        for bad in [
            "",
            "HEAD",
            "abc1234",
            "--output=/tmp/x",
            &A.to_uppercase(),
            &format!("{A}0"),
            &format!("{}g", &A[..39]),
        ] {
            assert!(!is_object_id(bad), "{bad:?}");
            assert!(check_id(bad).is_err());
        }
    }

    #[test]
    fn paths_must_be_plain() {
        assert!(check_path("src/a b.rs").is_ok());
        assert!(check_path("").is_err());
        assert!(check_path("a\0b").is_err());
        assert!(check_path(&"x".repeat(MAX_PATH + 1)).is_err());
    }

    #[test]
    fn sanitize_keeps_newlines_only_when_asked() {
        assert_eq!(sanitize("a\nb\tc", true), "a\nb   c");
        assert_eq!(sanitize("a\nb", false), "ab");
        assert_eq!(sanitize("日本\tx", false), "日本  x");
    }

    #[test]
    fn long_messages_are_cut_on_a_char_boundary() {
        let message = clean_message("é".repeat(MAX_MESSAGE_BYTES).as_bytes());
        assert!(message.len() <= MAX_MESSAGE_BYTES + '…'.len_utf8());
        assert!(message.ends_with('…'));
    }

    #[test]
    fn error_text_drops_the_fatal_prefix() {
        let stderr = b"\nfatal: not a git repository (or any of the parent directories): .git\n";
        assert_eq!(
            error_line(stderr, "x"),
            "not a git repository (or any of the parent directories): .git"
        );
        assert_eq!(
            error_line(b"", "exit status: 1"),
            "git exited with exit status: 1"
        );
    }

    #[test]
    fn git_versions_are_compared_numerically() {
        assert!(supports_diff_merges("git version 2.53.0\n"));
        assert!(supports_diff_merges("git version 2.31.0"));
        assert!(supports_diff_merges("git version 3.0.1"));
        assert!(!supports_diff_merges("git version 2.30.9"));
        assert!(!supports_diff_merges("git version 2.17.1"));
        assert!(supports_diff_merges("git version 2.39.3 (Apple Git-146)"));
        assert!(supports_diff_merges("unexpected"));
    }

    /// The encoded frame, not the raw patch, must fit.
    #[test]
    fn patches_are_trimmed_to_fit_the_frame() {
        // Every line escapes (`"`, `\`, newline), doubling its encoded size.
        let patch: String = "+\"quoted\\ line with some text\"\n".repeat(30_000);
        let overhead = json_len(&Msg::GitPatch(GitPatch {
            id: A.into(),
            path: None,
            patch: String::new(),
            truncated: true,
        }));
        let budget = proto::MAX_FRAME - ENVELOPE_SLACK - overhead;
        let fitted = fit_lines(&patch, budget);
        assert!(fitted.len() < patch.len());
        assert!(fitted.ends_with('\n'), "cut at a line boundary");

        let reply = Msg::GitPatch(GitPatch {
            id: A.into(),
            path: None,
            patch: fitted.to_owned(),
            truncated: true,
        });
        let frame = proto::Frame::Control(proto::Envelope::request(u64::MAX, reply));
        let encoded = frame.encode().len() - 4;
        assert!(
            encoded <= proto::MAX_FRAME - ENVELOPE_SLACK / 2,
            "{encoded}"
        );
    }

    #[test]
    fn oversized_metadata_is_a_bounded_error() {
        let big = Msg::GitCommitInfo(GitCommitInfo {
            id: A.into(),
            parents: vec![],
            author: String::new(),
            email: String::new(),
            time: 0,
            committer_time: 0,
            refs: vec![],
            message: "m".repeat(proto::MAX_FRAME),
            files: vec![],
            first_parent: false,
        });
        assert_eq!(fits_frame(big), Err("result too large to send".into()));
    }
}
