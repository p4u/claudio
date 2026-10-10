//! The git viewer's rendering (Alt+l): a full-pane view of [`GitView`].
//!
//! The tab bar and status bar stay visible; the pane shows one of three pages
//! (log, commit, diff), each a header row, a body and a footer row. Everything
//! is a pure function of the view state and the clock.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::proto::{GitCommitInfo, GitFile, GitLogEntry};

use super::super::git_view::{CommitPage, DiffPage, GitView, Load, Log, Patch, MESSAGE_ROWS_MAX};
use super::super::fmt::{fmt_age, fmt_utc, spans_width, str_width, truncate};

/// Columns of the author column in the log, when the pane is wide enough.
const AUTHOR_COLS: usize = 14;
/// Panes narrower than this drop the author column.
const AUTHOR_MIN_WIDTH: usize = 70;
/// Columns of the right-aligned age.
const AGE_COLS: usize = 4;
/// Longest diffstat bar, in cells.
const BAR_CELLS: usize = 10;
/// Width of the `+N -M ████` column on the commit page.
const STATS_COLS: usize = 6 + 7 + 1 + BAR_CELLS;
const SHORT_ID: usize = 7;

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn fg(color: Color) -> Style {
    Style::default().fg(color)
}

/// Draw the view into `area` (the session pane).
pub(super) fn draw_git(frame: &mut Frame, view: &GitView, now: u64, area: Rect) {
    frame.render_widget(Clear, area);
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let width = area.width as usize;
    let rows = body.height as usize;
    let text = |frame: &mut Frame, lines: Vec<Line<'static>>, rect: Rect| {
        frame.render_widget(Paragraph::new(lines), rect)
    };
    match &view.commit {
        None => {
            text(frame, vec![log_header(view, width)], header);
            text(frame, log_body(&view.log, now, width, rows), body);
            text(frame, vec![log_footer(view, width)], footer);
            if view.log.filtering {
                let x = area.x + (str_width(&view.log.filter) + 1) as u16;
                frame.set_cursor_position((x.min(footer.right().saturating_sub(1)), footer.y));
            }
        }
        Some(commit) => match &commit.diff {
            // The commit page lays out its own header rows.
            None => {
                let page = commit_page(commit, now, width, area.height as usize);
                text(
                    frame,
                    page,
                    Rect {
                        height: area.height.saturating_sub(1),
                        ..area
                    },
                );
                text(frame, vec![commit_footer(width)], footer);
            }
            Some(diff) => {
                text(frame, vec![diff_header(commit, diff, width)], header);
                text(frame, diff_body(diff, width, rows), body);
                text(frame, vec![diff_footer(diff)], footer);
            }
        },
    }
}

// ── Log page ──────────────────────────────────────────────────────────────────

fn log_header(view: &GitView, width: usize) -> Line<'static> {
    let log = &view.log;
    let branch = log.head.as_deref().unwrap_or("(detached)");
    let scope = if view.all {
        "all branches"
    } else {
        "current branch"
    };
    let mut spans = vec![
        Span::styled(
            format!(" ⎇ {branch}"),
            fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {}", log.root), bold()),
        Span::styled(
            format!("  · {} commits · {scope}", log.commits.len()),
            dim(),
        ),
    ];
    if !log.filter.is_empty() {
        let note = format!("  · filter \"{}\": {} match", log.filter, log.visible.len());
        spans.push(Span::styled(note, fg(Color::Yellow)));
    }
    fit_line(spans, width)
}

fn log_body(log: &Log, now: u64, width: usize, rows: usize) -> Vec<Line<'static>> {
    if log.commits.is_empty() {
        let text = match &log.error {
            Some(error) => Line::styled(format!(" git: {error}"), fg(Color::Red)),
            None => Line::styled(" loading…", dim()),
        };
        return vec![text];
    }
    let top = log.cursor.top(rows);
    log.visible
        .iter()
        .enumerate()
        .skip(top)
        .take(rows)
        .map(|(row, &i)| {
            let line = log_row(&log.commits[i], now, width);
            if row == log.cursor.selected {
                reversed(line)
            } else {
                line
            }
        })
        .collect()
}

/// ` ● a1b2c3d  subject (refs)  author  3h`, padded to `width`.
fn log_row(commit: &GitLogEntry, now: u64, width: usize) -> Line<'static> {
    let author_cols = if width >= AUTHOR_MIN_WIDTH {
        AUTHOR_COLS + 1
    } else {
        0
    };
    let right = author_cols + AGE_COLS + 1;
    let left = width.saturating_sub(4 + SHORT_ID + 1 + right);

    // Refs may take up to half the room; the subject gets the rest.
    let refs = ref_spans_fitting(&commit.refs, (left / 2).saturating_sub(1));
    let refs_cols = spans_width(&refs) + usize::from(!refs.is_empty());
    let subject = truncate(&commit.subject, left.saturating_sub(refs_cols));
    let pad = left.saturating_sub(str_width(&subject) + refs_cols);

    let merge = commit.parents.len() > 1;
    let glyph = if merge { "◆" } else { "●" };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(glyph, if merge { fg(Color::Magenta) } else { dim() }),
        Span::raw(" "),
        Span::styled(short(&commit.id).to_owned(), fg(Color::Yellow)),
        Span::raw(" "),
        Span::raw(subject),
    ];
    if !refs.is_empty() {
        spans.push(Span::raw(" "));
        spans.extend(refs);
    }
    spans.push(Span::raw(" ".repeat(pad + 1)));
    if author_cols > 0 {
        let author = pad_to(&commit.author, AUTHOR_COLS);
        spans.push(Span::styled(author, fg(Color::Blue)));
        spans.push(Span::raw(" "));
    }
    let age = fmt_age(now.saturating_sub(commit.time.max(0) as u64));
    spans.push(Span::styled(format!("{age:>AGE_COLS$}"), dim()));
    spans.push(Span::raw(" "));
    pad_line(spans, width)
}

fn log_footer(view: &GitView, width: usize) -> Line<'static> {
    let log = &view.log;
    if log.filtering {
        return Line::from(vec![
            Span::styled("/", fg(Color::Yellow)),
            Span::raw(log.filter.clone()),
            Span::styled("   Enter keep · Esc clear", dim()),
        ]);
    }
    if let Some(error) = &log.error {
        let text = format!(" {error}  ·  r retry · Esc close");
        return Line::styled(truncate(&text, width), fg(Color::Red));
    }
    let hints = "↑↓ move · Enter open · / filter · a all branches · r refresh · Esc close";
    let status = if view.loading_log() {
        " loading…  "
    } else {
        " "
    };
    let text = truncate(&format!("{status}{hints}"), width);
    Line::styled(text, dim())
}

// ── Commit page ───────────────────────────────────────────────────────────────

/// The whole commit page above the footer row.
fn commit_page(commit: &CommitPage, now: u64, width: usize, height: usize) -> Vec<Line<'static>> {
    let info = match &commit.info {
        Load::Ready(info) => info,
        Load::Loading => {
            return vec![
                commit_id_line(&commit.id, None),
                Line::default(),
                Line::styled(" loading…", dim()),
            ]
        }
        Load::Failed(error) => {
            return vec![
                commit_id_line(&commit.id, None),
                Line::default(),
                Line::styled(format!(" git: {error}"), fg(Color::Red)),
            ]
        }
    };
    let mut lines = vec![
        commit_id_line(&info.id, Some(info)),
        Line::from(vec![
            Span::styled(" Author: ", dim()),
            Span::styled(info.author.clone(), fg(Color::Blue)),
            Span::styled(format!(" <{}>", info.email), dim()),
        ]),
        date_line(info, now),
        Line::from(
            [Span::styled(" Refs:   ", dim())]
                .into_iter()
                .chain(ref_spans(&info.refs))
                .collect::<Vec<_>>(),
        ),
        Line::default(),
    ];
    lines.extend(message_lines(&info.message, width));
    lines.push(Line::default());
    lines.push(stats_summary(&info.files));
    let body = height.saturating_sub(commit.files_top() + 1).max(1);
    let top = commit.files.top(body);
    lines.extend(
        info.files
            .iter()
            .enumerate()
            .skip(top)
            .take(body)
            .map(|(i, file)| {
                let line = file_row(file, width);
                if i == commit.files.selected {
                    reversed(line)
                } else {
                    line
                }
            }),
    );
    lines
}

fn commit_id_line(id: &str, info: Option<&GitCommitInfo>) -> Line<'static> {
    let mut spans = vec![
        Span::styled(" commit ", dim()),
        Span::styled(id.to_owned(), fg(Color::Yellow)),
    ];
    if info.is_some_and(|i| i.first_parent) {
        spans.push(Span::styled(
            "  merge · changes vs first parent",
            fg(Color::Magenta),
        ));
    }
    Line::from(spans)
}

fn date_line(info: &GitCommitInfo, now: u64) -> Line<'static> {
    let age = fmt_age(now.saturating_sub(info.time.max(0) as u64));
    let mut spans = vec![
        Span::styled(" Date:   ", dim()),
        Span::raw(fmt_utc(info.time)),
        Span::styled(format!("  ({age} ago)"), dim()),
    ];
    if info.committer_time != info.time {
        let note = format!("  committed {}", fmt_utc(info.committer_time));
        spans.push(Span::styled(note, dim()));
    }
    Line::from(spans)
}

/// The message with a bold subject, cut to [`MESSAGE_ROWS_MAX`] lines.
fn message_lines(message: &str, width: usize) -> Vec<Line<'static>> {
    let total = message.lines().count();
    let mut lines: Vec<Line<'static>> = message
        .lines()
        .take(MESSAGE_ROWS_MAX)
        .enumerate()
        .map(|(i, text)| {
            let style = if i == 0 { bold() } else { Style::default() };
            Line::styled(truncate(&format!("   {text}"), width), style)
        })
        .collect();
    if total > MESSAGE_ROWS_MAX {
        let more = format!("   … {} more lines", total - MESSAGE_ROWS_MAX);
        lines.push(Line::styled(more, dim()));
    }
    debug_assert!(lines.len() <= MESSAGE_ROWS_MAX + 1);
    lines
}

fn stats_summary(files: &[GitFile]) -> Line<'static> {
    let (added, removed) = files.iter().fold((0u64, 0u64), |(a, r), f| {
        (
            a + u64::from(f.added.unwrap_or(0)),
            r + u64::from(f.removed.unwrap_or(0)),
        )
    });
    Line::from(vec![
        Span::styled(format!(" {} files changed  ", files.len()), dim()),
        Span::styled(format!("+{added}"), fg(Color::Green)),
        Span::raw(" "),
        Span::styled(format!("-{removed}"), fg(Color::Red)),
    ])
}

/// ` path   +12   -3 ██████░░░░` with the stats right-aligned.
fn file_row(file: &GitFile, width: usize) -> Line<'static> {
    let name = match &file.old_path {
        Some(old) => format!("{old} → {}", file.path),
        None => file.path.clone(),
    };
    let room = width.saturating_sub(STATS_COLS + 3);
    let name = pad_to(&name, room);
    let mut spans = vec![Span::raw(" "), Span::raw(name), Span::raw("  ")];
    match (file.added, file.removed) {
        (Some(added), Some(removed)) => {
            let (green, red) = bar_cells(added, removed, BAR_CELLS);
            spans.extend([
                Span::styled(format!("+{added:>5}"), fg(Color::Green)),
                Span::styled(format!(" -{removed:>5}"), fg(Color::Red)),
                Span::raw(" "),
                Span::styled("█".repeat(green), fg(Color::Green)),
                Span::styled("█".repeat(red), fg(Color::Red)),
            ]);
        }
        _ => spans.push(Span::styled("binary", dim())),
    }
    pad_line(spans, width)
}

/// Green and red cell counts of a diffstat bar at most `width` cells wide.
fn bar_cells(added: u32, removed: u32, width: usize) -> (usize, usize) {
    let (added, removed) = (added as usize, removed as usize);
    let total = added + removed;
    if total == 0 {
        return (0, 0);
    }
    let cells = total.min(width);
    let mut green = (cells * added + total / 2) / total;
    if added > 0 && green == 0 {
        green = 1;
    }
    if removed > 0 && green == cells && cells > 1 {
        green = cells - 1;
    }
    (green, cells - green)
}

fn commit_footer(width: usize) -> Line<'static> {
    let hints = " ↑↓ select file · Enter file diff · d whole diff · Esc back";
    Line::styled(truncate(hints, width), dim())
}

// ── Diff page ─────────────────────────────────────────────────────────────────

fn diff_header(commit: &CommitPage, diff: &DiffPage, width: usize) -> Line<'static> {
    let what = diff.path.as_deref().unwrap_or("whole commit");
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(short(&commit.id).to_owned(), fg(Color::Yellow)),
        Span::styled(format!("  {what}"), bold()),
    ];
    if let Load::Ready(patch) = &diff.patch {
        spans.push(Span::styled(
            format!("  · {} lines", patch.lines.len()),
            dim(),
        ));
    }
    fit_line(spans, width)
}

fn diff_body(diff: &DiffPage, width: usize, rows: usize) -> Vec<Line<'static>> {
    match &diff.patch {
        Load::Loading => vec![Line::styled(" loading…", dim())],
        Load::Failed(error) => vec![Line::styled(format!(" git: {error}"), fg(Color::Red))],
        Load::Ready(patch) if patch.lines.is_empty() => {
            vec![Line::styled(" no textual changes", dim())]
        }
        Load::Ready(patch) => {
            let top = diff.scroll(rows);
            (top..patch.lines.len().min(top + rows))
                .map(|i| {
                    let style = line_style(&patch.lines, i);
                    Line::styled(truncate(&patch.lines[i], width), style)
                })
                .collect()
        }
    }
}

fn diff_footer(diff: &DiffPage) -> Line<'static> {
    let hints = "↑↓ PgUp PgDn scroll · n/N next/prev file · g/G top/bottom · Esc back";
    match &diff.patch {
        Load::Ready(Patch {
            truncated: true, ..
        }) => Line::styled(format!(" diff truncated · {hints}"), dim()),
        _ => Line::styled(format!(" {hints}"), dim()),
    }
}

/// How a patch line is drawn, from its first bytes and its neighbours:
/// `---`/`+++` are headers only as a pair, otherwise removed/added lines.
fn line_style(lines: &[String], i: usize) -> Style {
    let line = lines[i].as_str();
    let next_is = |prefix: &str| lines.get(i + 1).is_some_and(|l| l.starts_with(prefix));
    let prev_is = |prefix: &str| i > 0 && lines[i - 1].starts_with(prefix);
    if line.starts_with("diff --git ")
        || (line.starts_with("--- ") && next_is("+++ "))
        || (line.starts_with("+++ ") && prev_is("--- "))
    {
        bold()
    } else if line.starts_with("@@") {
        fg(Color::Cyan)
    } else if line.starts_with('+') {
        fg(Color::Green)
    } else if line.starts_with('-') {
        fg(Color::Red)
    } else if line.starts_with("index ") || line.starts_with("Binary files") {
        dim()
    } else {
        Style::default()
    }
}

// ── Refs, dates and line helpers ──────────────────────────────────────────────

/// A decoration as shown: `main`, `origin/main`, `tag: v1`, or `HEAD -> main`.
#[derive(Debug, PartialEq)]
struct GitRef {
    label: String,
    kind: RefKind,
    /// HEAD points at this branch.
    head: bool,
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum RefKind {
    Head,
    Branch,
    Remote,
    Tag,
    Other,
}

/// Classify one `--decorate=full` decoration.
fn parse_ref(raw: &str) -> GitRef {
    let (raw, head) = match raw.strip_prefix("HEAD -> ") {
        Some(rest) => (rest, true),
        None => (raw, false),
    };
    let (label, kind) = if raw == "HEAD" {
        ("HEAD", RefKind::Head)
    } else if let Some(tag) = raw.strip_prefix("tag: ") {
        (tag.strip_prefix("refs/tags/").unwrap_or(tag), RefKind::Tag)
    } else if let Some(name) = raw.strip_prefix("refs/heads/") {
        (name, RefKind::Branch)
    } else if let Some(name) = raw.strip_prefix("refs/remotes/") {
        (name, RefKind::Remote)
    } else {
        (raw.strip_prefix("refs/").unwrap_or(raw), RefKind::Other)
    };
    GitRef {
        label: label.to_owned(),
        kind,
        head,
    }
}

/// `(HEAD -> main, origin/main, tag: v1)` coloured by kind; empty for none.
fn ref_spans(refs: &[String]) -> Vec<Span<'static>> {
    if refs.is_empty() {
        return Vec::new();
    }
    let mut spans = vec![Span::styled("(", dim())];
    for (i, raw) in refs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(", ", dim()));
        }
        let r = parse_ref(raw);
        if r.head {
            spans.push(Span::styled(
                "HEAD",
                fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(" → ", dim()));
        }
        let style = match r.kind {
            RefKind::Head => fg(Color::Cyan).add_modifier(Modifier::BOLD),
            RefKind::Branch => fg(Color::Green),
            RefKind::Remote => fg(Color::Red),
            RefKind::Tag => fg(Color::Yellow).add_modifier(Modifier::BOLD),
            RefKind::Other => dim(),
        };
        let label = if r.kind == RefKind::Tag {
            format!("tag: {}", r.label)
        } else {
            r.label
        };
        spans.push(Span::styled(label, style));
    }
    spans.push(Span::styled(")", dim()));
    spans
}

/// [`ref_spans`] for as many leading refs as fit in `max_cols`.
fn ref_spans_fitting(refs: &[String], max_cols: usize) -> Vec<Span<'static>> {
    (1..=refs.len())
        .rev()
        .map(|n| ref_spans(&refs[..n]))
        .find(|spans| spans_width(spans) <= max_cols)
        .unwrap_or_default()
}

fn short(id: &str) -> &str {
    id.get(..SHORT_ID).unwrap_or(id)
}

/// `text` cut and space-padded to exactly `cols` columns.
fn pad_to(text: &str, cols: usize) -> String {
    let text = truncate(text, cols);
    let pad = cols.saturating_sub(str_width(&text));
    format!("{text}{}", " ".repeat(pad))
}

/// Pad a line with spaces to `width`, so a reversed row spans the pane.
fn pad_line(mut spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let pad = width.saturating_sub(spans_width(&spans));
    spans.push(Span::raw(" ".repeat(pad)));
    Line::from(spans)
}

/// Drop trailing spans until the line fits `width`.
fn fit_line(mut spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    while spans.len() > 1 && spans_width(&spans) > width {
        spans.pop();
    }
    Line::from(spans)
}

fn reversed(line: Line<'static>) -> Line<'static> {
    let spans = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content, s.style.add_modifier(Modifier::REVERSED)))
        .collect::<Vec<_>>();
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::proto::{GitLogPage, Msg};
    use crate::tui::git_view::GitOutcome;

    const A: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";

    #[test]
    fn refs_are_classified() {
        let r = parse_ref("HEAD -> refs/heads/main");
        assert_eq!(
            (r.label.as_str(), r.kind, r.head),
            ("main", RefKind::Branch, true)
        );
        let r = parse_ref("refs/remotes/origin/dev");
        assert_eq!(
            (r.label.as_str(), r.kind, r.head),
            ("origin/dev", RefKind::Remote, false)
        );
        assert_eq!(parse_ref("tag: refs/tags/v1.2").label, "v1.2");
        assert_eq!(parse_ref("tag: refs/tags/v1.2").kind, RefKind::Tag);
        assert_eq!(parse_ref("HEAD").kind, RefKind::Head);
        let other = parse_ref("refs/stash");
        assert_eq!(
            (other.label.as_str(), other.kind),
            ("stash", RefKind::Other)
        );
    }

    #[test]
    fn ref_spans_color_by_kind() {
        let refs = vec![
            "HEAD -> refs/heads/main".to_owned(),
            "refs/remotes/origin/main".to_owned(),
            "tag: refs/tags/v1".to_owned(),
        ];
        let spans = ref_spans(&refs);
        let find = |text: &str| spans.iter().find(|s| s.content == text).unwrap().style;
        assert_eq!(find("HEAD").fg, Some(Color::Cyan));
        assert_eq!(find("main").fg, Some(Color::Green));
        assert_eq!(find("origin/main").fg, Some(Color::Red));
        assert_eq!(find("tag: v1").fg, Some(Color::Yellow));
        assert!(ref_spans(&[]).is_empty());
    }

    #[test]
    fn diffstat_bars_scale_and_keep_both_colors() {
        assert_eq!(bar_cells(0, 0, 10), (0, 0));
        assert_eq!(bar_cells(3, 0, 10), (3, 0));
        assert_eq!(bar_cells(0, 4, 10), (0, 4));
        assert_eq!(bar_cells(50, 50, 10), (5, 5));
        // A lone addition among many removals still shows.
        assert_eq!(bar_cells(1, 99, 10), (1, 9));
        assert_eq!(bar_cells(99, 1, 10), (9, 1));
    }

    #[test]
    fn patch_lines_get_a_style_by_kind() {
        let lines: Vec<String> = [
            "diff --git a/f b/f",
            "index 1..2 100644",
            "--- a/f",
            "+++ b/f",
            "@@ -1 +1 @@",
            "-old",
            "+new",
            " ctx",
            "--- looks like a header but is a removed line",
            "+added",
        ]
        .map(String::from)
        .into();
        let style = |i| line_style(&lines, i);
        assert_eq!(style(0), bold());
        assert_eq!(style(1), dim());
        assert_eq!(style(2), bold());
        assert_eq!(style(3), bold());
        assert_eq!(style(4).fg, Some(Color::Cyan));
        assert_eq!(style(5).fg, Some(Color::Red));
        assert_eq!(style(6).fg, Some(Color::Green));
        assert_eq!(style(7), Style::default());
        assert_eq!(style(8).fg, Some(Color::Red), "a lone `---` is a removal");
    }

    fn entry(n: usize, merge: bool) -> GitLogEntry {
        GitLogEntry {
            id: format!("{n:07x}{:033}", 0),
            parents: if merge {
                vec![A.into(), A.into()]
            } else {
                vec![A.into()]
            },
            author: "Ann Author".into(),
            time: 1_700_000_000,
            refs: vec!["HEAD -> refs/heads/main".into(), "tag: refs/tags/v1".into()],
            subject: format!("subject number {n}"),
        }
    }

    fn row_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn log_rows_fill_the_width_and_show_every_column() {
        for width in [60, 100, 160] {
            let line = log_row(&entry(1, false), 1_700_003_600, width);
            assert_eq!(str_width(&row_text(&line)), width, "width {width}");
            let text = row_text(&line);
            assert!(text.contains("0000001"), "{text}");
            assert!(text.contains("1h"), "{text}");
        }
        let wide = row_text(&log_row(&entry(1, true), 1_700_003_600, 120));
        assert!(wide.contains('◆') && wide.contains("Ann Author"));
        assert!(wide.contains("HEAD → main") && wide.contains("tag: v1"));
        assert!(row_text(&log_row(&entry(1, false), 0, 120)).contains('●'));
    }

    #[test]
    fn a_narrow_row_keeps_the_subject_over_the_refs() {
        let text = row_text(&log_row(&entry(1, false), 1_700_003_600, 40));
        assert_eq!(str_width(&text), 40);
        assert!(text.contains("subject"), "{text}");
    }

    #[test]
    fn the_selected_row_is_reversed() {
        let line = reversed(log_row(&entry(1, false), 0, 80));
        assert!(line
            .spans
            .iter()
            .all(|s| s.style.add_modifier.contains(Modifier::REVERSED)));
    }

    /// Render the view into a buffer and return its rows as text.
    fn screen(view: &GitView, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| draw_git(f, view, 1_700_003_600, f.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn open_view() -> GitView {
        let (mut view, first) = GitView::open("local".into(), "/repo".into());
        let GitOutcome::Request { seq, .. } = first else {
            panic!()
        };
        let page = GitLogPage {
            root: "/repo".into(),
            head: Some("main".into()),
            commits: (1..4).map(|n| entry(n, n == 2)).collect(),
            more: false,
        };
        view.on_reply(seq, Ok(Msg::GitLogPage(page)));
        view
    }

    #[test]
    fn the_log_page_draws_header_rows_and_footer() {
        let rows = screen(&open_view(), 100, 10);
        assert!(
            rows[0].contains("⎇ main") && rows[0].contains("3 commits"),
            "{}",
            rows[0]
        );
        assert!(rows[1].contains("subject number 1"), "{}", rows[1]);
        assert!(rows[2].contains('◆'), "merge glyph: {}", rows[2]);
        assert!(rows[9].contains("Enter open"), "{}", rows[9]);
    }

    #[test]
    fn a_git_error_is_shown_inside_the_view() {
        let (mut view, first) = GitView::open("local".into(), "/tmp".into());
        let GitOutcome::Request { seq, .. } = first else {
            panic!()
        };
        view.on_reply(seq, Err("not a git repository".into()));
        let rows = screen(&view, 80, 8);
        assert!(rows[1].contains("git: not a git repository"), "{}", rows[1]);
        assert!(rows[7].contains("r retry"), "{}", rows[7]);
    }
}
