//! The core's sanitising and caps for what an addon hands a front-end (C2).
//! The runtime applies them, never the addon.

use super::{
    name_ok, Block, Checklist, ChecklistItem, ExtensionRow, LaunchOption, Notice, Page, Panel,
    PanelAction, Segment, Tone,
};

/// A segment's text, in characters.
pub const SEGMENT_CHARS: usize = 64;
/// A panel, JSON-encoded.
pub const PANEL_BYTES: usize = 64 * 1024;
/// A checklist's items.
pub const CHECKLIST_ITEMS: usize = 256;
/// Any one string of a panel, checklist, row, option or notice, in bytes.
pub const TEXT_BYTES: usize = 4096;

/// `s` as one safe line: `text::sanitize`, newlines and tabs as spaces,
/// cut at `max_bytes` on a char boundary.
pub fn line(s: &str, max_bytes: usize) -> String {
    let clean: String = crate::text::sanitize(s)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.len() <= max_bytes {
        return clean;
    }
    let mut end = max_bytes;
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    clean[..end].to_string()
}

fn text(s: String) -> String {
    line(&s, TEXT_BYTES)
}

fn texts(v: Vec<String>) -> Vec<String> {
    v.into_iter().map(text).collect()
}

/// A segment with a valid id (else `segment`) and at most 64 safe chars.
pub fn segment(s: Segment) -> Segment {
    let id = if name_ok(&s.id) {
        s.id
    } else {
        "segment".into()
    };
    Segment {
        id,
        text: line(&s.text, TEXT_BYTES)
            .chars()
            .take(SEGMENT_CHARS)
            .collect(),
        ..s
    }
}

fn block(b: Block) -> Block {
    match b {
        Block::Text { text: t, tone } => Block::Text {
            text: text(t),
            tone,
        },
        Block::KeyValues { items } => Block::KeyValues {
            items: items.into_iter().map(|(k, v)| (text(k), text(v))).collect(),
        },
        Block::Table { header, rows } => Block::Table {
            header: texts(header),
            rows: rows.into_iter().map(texts).collect(),
        },
        Block::Gauge {
            label,
            used,
            limit,
            resets,
        } => Block::Gauge {
            label: text(label),
            used,
            limit,
            resets: resets.map(text),
        },
        Block::Bars { label, series } => Block::Bars {
            label: text(label),
            series: series.into_iter().map(|(k, v)| (text(k), v)).collect(),
        },
    }
}

fn action(a: PanelAction) -> PanelAction {
    PanelAction {
        id: text(a.id),
        label: text(a.label),
        key: a
            .key
            .filter(|c| !crate::text::is_unsafe(*c) && !c.is_control()),
        confirm: a.confirm.map(text),
    }
}

/// A panel with every string safe; one over 64 KiB encoded becomes an
/// error page.
pub fn panel(p: Panel) -> Panel {
    let title = text(p.title);
    let clean = Panel {
        title,
        pages: p
            .pages
            .into_iter()
            .map(|page| Page {
                title: text(page.title),
                blocks: page.blocks.into_iter().map(block).collect(),
            })
            .collect(),
        actions: p.actions.into_iter().map(action).collect(),
        updated: p.updated,
    };
    let size = serde_json::to_vec(&clean).map_or(usize::MAX, |v| v.len());
    if size > PANEL_BYTES {
        return error_panel(&clean.title, "this panel is over 64 KiB");
    }
    clean
}

/// A checklist of at most 256 items, every string safe.
pub fn checklist(c: Checklist) -> Checklist {
    Checklist {
        id: text(c.id),
        revision: text(c.revision),
        title: text(c.title),
        intro: text(c.intro),
        items: c
            .items
            .into_iter()
            .take(CHECKLIST_ITEMS)
            .map(|i| ChecklistItem {
                id: text(i.id),
                label: text(i.label),
                detail: i.detail.map(text),
                section: i.section.map(text),
                on: i.on,
            })
            .collect(),
    }
}

/// An extension row with every string safe.
pub fn row(r: ExtensionRow) -> ExtensionRow {
    ExtensionRow {
        name: text(r.name),
        description: text(r.description),
        default_on: r.default_on,
        refused: r.refused.map(text),
        details: texts(r.details),
        setup: r.setup.map(text),
    }
}

/// A launch option with every string safe.
pub fn option(o: LaunchOption) -> LaunchOption {
    LaunchOption {
        id: text(o.id),
        label: text(o.label),
        group: text(o.group),
        default: o.default,
        detail: o.detail.map(text),
    }
}

/// A notice with a safe text.
pub fn notice(n: Notice) -> Notice {
    Notice {
        text: text(n.text),
        level: n.level,
    }
}

/// A one-page panel that says why the real one is not shown.
pub fn error_panel(title: &str, why: &str) -> Panel {
    Panel {
        title: line(title, TEXT_BYTES),
        pages: vec![Page {
            title: "Error".into(),
            blocks: vec![Block::Text {
                text: line(why, TEXT_BYTES),
                tone: Some(Tone::Hot),
            }],
        }],
        actions: Vec::new(),
        updated: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon::Level;

    #[test]
    fn clean_caps_segments_panels_checklists_and_strips_controls() {
        // Controls: ESC, BEL, a C1 control, newlines and tabs.
        let dirty = "a\x1b[31mb\x07c\u{9b}d\ne\tf\r";
        assert_eq!(line(dirty, TEXT_BYTES), "a[31mbcd e f");
        // Cut on a char boundary.
        assert_eq!(line("ééé", 3), "é");
        assert_eq!(line(&"x".repeat(5000), TEXT_BYTES).len(), TEXT_BYTES);

        let s = segment(Segment {
            id: "Bad Id".into(),
            text: format!("\x1b]0;t\x07{}", "é".repeat(100)),
            tone: Tone::Ok,
            priority: 7,
        });
        assert_eq!(s.id, "segment");
        assert_eq!(s.text.chars().count(), SEGMENT_CHARS);
        assert!(!s.text.contains('\x1b') && !s.text.contains('\x07'));
        assert_eq!(s.priority, 7);
        assert_eq!(
            segment(Segment {
                id: "route".into(),
                text: "ok".into(),
                tone: Tone::Muted,
                priority: 0
            })
            .id,
            "route"
        );

        // A panel's strings are cleaned wherever they are.
        let p = panel(Panel {
            title: "T\x1b".into(),
            pages: vec![Page {
                title: "p\n1".into(),
                blocks: vec![
                    Block::KeyValues {
                        items: vec![("k\x07".into(), "v\u{85}".into())],
                    },
                    Block::Table {
                        header: vec!["h\x1b".into()],
                        rows: vec![vec!["c\n".into()]],
                    },
                ],
            }],
            actions: vec![PanelAction {
                id: "a".into(),
                label: "L\x1b".into(),
                key: Some('\x1b'),
                confirm: Some("sure?\x07".into()),
            }],
            updated: Some(1),
        });
        let json = serde_json::to_string(&p).unwrap();
        for bad in ['\x1b', '\x07', '\u{85}', '\n'] {
            assert!(!json.contains(bad), "{bad:?} in {json}");
        }
        assert_eq!(p.title, "T");
        assert_eq!(p.actions[0].key, None);

        // A 70 KiB panel becomes one error page.
        let big = Panel {
            title: "Big".into(),
            pages: vec![Page {
                title: "p".into(),
                blocks: (0..18)
                    .map(|_| Block::Text {
                        text: "x".repeat(4000),
                        tone: None,
                    })
                    .collect(),
            }],
            actions: Vec::new(),
            updated: None,
        };
        assert!(serde_json::to_vec(&big).unwrap().len() > 70 * 1024);
        let err = panel(big);
        assert_eq!(err.title, "Big");
        assert_eq!(err.pages.len(), 1);
        assert!(matches!(
            &err.pages[0].blocks[..],
            [Block::Text {
                tone: Some(Tone::Hot),
                ..
            }]
        ));

        // 300 items → 256.
        let c = checklist(Checklist {
            id: "c".into(),
            revision: "r".into(),
            title: "t".into(),
            intro: "i\x1b".into(),
            items: (0..300)
                .map(|i| ChecklistItem {
                    id: format!("i{i}"),
                    label: "l\x07".into(),
                    detail: Some("d\n".into()),
                    section: None,
                    on: true,
                })
                .collect(),
        });
        assert_eq!(c.items.len(), CHECKLIST_ITEMS);
        assert_eq!(c.intro, "i");
        assert_eq!(c.items[0].label, "l");
        assert_eq!(c.items[0].detail.as_deref(), Some("d "));

        let n = notice(Notice {
            text: "x\x1by".into(),
            level: Level::Warn,
        });
        assert_eq!(n.text, "xy");
        let r = row(ExtensionRow {
            name: "n".into(),
            description: "d\x1b".into(),
            default_on: true,
            refused: Some("r\x07".into()),
            details: vec!["a\nb".into()],
            setup: None,
        });
        assert_eq!(
            (
                r.description.as_str(),
                r.refused.as_deref(),
                r.details[0].as_str()
            ),
            ("d", Some("r"), "a b")
        );
        let o = option(LaunchOption {
            id: "id".into(),
            label: "L\x1b".into(),
            group: "g".into(),
            default: false,
            detail: Some("x\u{9b}".into()),
        });
        assert_eq!((o.label.as_str(), o.detail.as_deref()), ("L", Some("x")));
    }
}
