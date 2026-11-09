//! The panel renderer's tests on a TestBackend: each block kind, a long
//! text clipped to the width, and the chrome.

use super::*;
use crate::addon::{Page, Panel, PanelAction, PanelDecl};
use ratatui::{backend::TestBackend, Terminal};

const DECL: PanelDecl = PanelDecl {
    id: "proxy",
    title: "Proxy stats",
    key: None,
    windows: &["24h", "7d", "30d"],
};

fn view(blocks: Vec<Block>) -> PanelView {
    let mut v = PanelView::new("vocdoni", DECL, None, 1);
    let panel = Panel {
        title: "Proxy stats: work".into(),
        pages: vec![
            Page {
                title: "Overview".into(),
                blocks,
            },
            Page {
                title: "Models".into(),
                blocks: Vec::new(),
            },
        ],
        actions: vec![PanelAction {
            id: "switch".into(),
            label: "next sub".into(),
            key: Some('n'),
            confirm: None,
        }],
        updated: Some(100),
    };
    v.answered(1, panel);
    v
}

fn text(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

#[test]
fn key_values_align_and_tables_size_their_columns() {
    let v = view(vec![
        Block::KeyValues {
            items: vec![
                ("Account".into(), "pau".into()),
                ("Cache hit".into(), "86%".into()),
            ],
        },
        Block::Table {
            header: vec!["Model".into(), "Req".into()],
            rows: vec![
                vec!["opus-5-5[1m]".into(), "812".into()],
                vec!["haiku".into(), "122".into()],
            ],
        },
    ]);
    assert_eq!(
        text(&body_lines(&v, 60)),
        [
            " Account    pau",
            " Cache hit  86%",
            "",
            " Model         Req",
            " opus-5-5[1m]  812",
            " haiku         122",
        ]
    );
}

#[test]
fn gauges_and_bars_draw_their_numbers() {
    let v = view(vec![
        Block::Gauge {
            label: "Limit: output tokens per 5h".into(),
            used: 900_000.0,
            limit: 5_000_000.0,
            resets: Some("BLOCKED until soon".into()),
        },
        Block::Bars {
            label: "Requests / day".into(),
            series: vec![("24h".into(), 1234.0), ("7d".into(), 617.0)],
        },
    ]);
    let lines = text(&body_lines(&v, 80));
    assert_eq!(lines[0], " Limit: output tokens per 5h");
    assert!(lines[1].starts_with(" [███▋"), "{:?}", lines[1]);
    assert!(
        lines[1].ends_with("] 900k/5.0M  BLOCKED until soon"),
        "{:?}",
        lines[1]
    );
    assert_eq!(lines[3], " Requests / day");
    assert_eq!(lines[4], format!("   24h {}       1k", "█".repeat(20)));
    assert!(lines[5].starts_with("   7d  ██████████░"), "{:?}", lines[5]);
}

#[test]
fn long_text_wraps_and_cells_are_clipped_to_the_width() {
    let long = "word ".repeat(30);
    let v = view(vec![
        Block::Text {
            text: long.trim().into(),
            tone: Some(Tone::Muted),
        },
        Block::Table {
            header: Vec::new(),
            rows: vec![vec!["x".repeat(100)]],
        },
    ]);
    let lines = body_lines(&v, 20);
    for l in &lines {
        assert!(l.width() <= 20, "{:?}", text(std::slice::from_ref(l)));
    }
    assert!(lines.len() > 5, "wrapped");
    assert_eq!(lines[0].style, dim());
}

#[test]
fn the_chrome_shows_pages_window_actions_and_age() {
    let v = view(vec![Block::Text {
        text: "No proxy configured.".into(),
        tone: None,
    }]);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| draw_panel(f, &v, 130)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let screen: String = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect();
    for want in [
        "Proxy stats: work",
        " 1 Overview ",
        " 2 Models ",
        "window: [24h] 7d  30d",
        "No proxy configured.",
        "n next sub",
        "updated 30s ago",
    ] {
        assert!(screen.contains(want), "{want:?} not in:\n{screen}");
    }
    // A tiny terminal draws without panicking.
    let mut tiny = Terminal::new(TestBackend::new(10, 4)).unwrap();
    tiny.draw(|f| draw_panel(f, &v, 130)).unwrap();
}
