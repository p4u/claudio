use std::time::{Duration, Instant};

use super::*;

/// `src` drawn, and every row within the caps and free of controls.
fn draw(src: &str) -> Vec<String> {
    let rows = render(src).unwrap_or_else(|e| panic!("{src}: {}", e.reason));
    assert!(rows.len() <= MAX_ROWS);
    for row in &rows {
        assert!(text::width(row) <= MAX_COLS, "{row}");
        assert!(!row.chars().any(char::is_control), "{row:?}");
        assert_eq!(row.trim_end(), row);
    }
    rows
}

/// The reason `src` is not drawn, found within a second.
fn refused(src: &str) -> String {
    let start = Instant::now();
    let reason = render(src).expect_err("unsupported").reason;
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{reason}: {:?}",
        start.elapsed()
    );
    reason
}

/// AC8: a TD flowchart with a decision, labels, and a loop back up.
#[test]
fn td_flowchart_with_a_decision_and_labels() {
    let src = "flowchart TD
    A[Start] --> B{Is it ready?}
    B -->|yes| C[Ship it]
    B -->|no| D(Fix the build)
    D --> B";
    let expected = [
        "            ┌───────┐",
        "            │ Start │",
        "            └───┬───┘",
        "                │",
        "                ▼",
        "        ╱──────────────╲",
        "        │ Is it ready? │",
        "        ╲───────┬──────╱",
        "                │ ▲",
        "                │ └────┐",
        "     ┌──────────┴────┐ │",
        "     │               │ │",
        "    yes             no │",
        "     ▼               ▼ │",
        "┌─────────┐  ╭─────────┴─────╮",
        "│ Ship it │  │ Fix the build │",
        "└─────────┘  ╰───────────────╯",
    ];
    assert_eq!(draw(src), expected);
}

/// AC8: an LR chain (`a --> b --> c`), with a rounded end.
#[test]
fn lr_chain() {
    let src = "graph LR
    a[Parse] --> b[Plan] --> c[Build] --> d([Done])";
    let expected = [
        "┌───────┐  ┌──────┐  ┌───────┐  ╭──────╮",
        "│ Parse ├─▶│ Plan ├─▶│ Build ├─▶│ Done │",
        "└───────┘  └──────┘  └───────┘  ╰──────╯",
    ];
    assert_eq!(draw(src), expected);
}

/// AC8: a cycle: the back edge leaves its own port with `▲`.
#[test]
fn cycle() {
    let src = "graph TD
    A --> B
    B --> C
    C --> A";
    let expected = [
        "┌─────┐",
        "│  A  │",
        "└──┬──┘",
        "   │ ▲",
        "   │ └─┐",
        "   ▼   │",
        " ┌───┐ │",
        " │ B │ │",
        " └─┬─┘ │",
        "   │ ┌─┘",
        "   ▼ │",
        "┌────┴┐",
        "│  C  │",
        "└─────┘",
    ];
    assert_eq!(draw(src), expected);
}

/// AC8: a sequence diagram with a note and a loop.
#[test]
fn sequence_with_a_note_and_a_loop() {
    let src = "sequenceDiagram
    participant A as Alice
    participant B as Bob
    A->>B: Hello Bob
    Note right of B: thinks
    loop every minute
        B-->>A: still here
    end
    A-)B: bye";
    let expected = [
        "  ┌───────┐      ┌─────┐",
        "  │ Alice │      │ Bob │",
        "  └───┬───┘      └──┬──┘",
        "      │  Hello Bob  │",
        "      ├────────────▶│",
        "      │             │ ┌┄┄┄┄┄┄┄┄┐",
        "      │             │ ┆ thinks ┆",
        "      │             │ └┄┄┄┄┄┄┄┄┘",
        "┌─ loop: every minute ─────────────┐",
        "│     │ still here  │              │",
        "│     │◀┄┄┄┄┄┄┄┄┄┄┄┄┤              │",
        "└─────┼─────────────┼──────────────┘",
        "      │     bye     │",
        "      ├────────────▶│",
        "      │             │",
    ];
    assert_eq!(draw(src), expected);
}

/// Fan-out and merges share junctions; dotted edges stay dotted.
#[test]
fn merged_junctions_and_dotted_edges() {
    let src = "graph TD
  A[Client] --> B[API gateway]
  B --> C[Auth service]
  B --> D[Orders]
  D --> E[(Database)]
  C --> E
  D -.-> F[Queue]
  F -.-> G[Worker]
  G --> E";
    let expected = [
        "          ┌────────┐",
        "          │ Client │",
        "          └────┬───┘",
        "               │",
        "               ▼",
        "        ┌─────────────┐",
        "        │ API gateway │",
        "        └──────┬──────┘",
        "        ┌──────┴───────┐",
        "        ▼              ▼",
        "┌──────────────┐  ┌────────┐",
        "│ Auth service │  │ Orders │",
        "└───────┬──────┘  └────┬───┘",
        "        └───┐      ┌┄┄┄┴─┐",
        "            │      ▼     │",
        "            │  ┌───────┐ │",
        "            │  │ Queue │ │",
        "            │  └───┬───┘ │",
        "            │      ┆     │",
        "            │      ▼     │",
        "            │ ┌────────┐ │",
        "            │ │ Worker │ │",
        "            │ └────┬───┘ │",
        "            └──────┤     │",
        "                   ├─────┘",
        "                   ▼",
        "             ┌──────────┐",
        "             │ Database │",
        "             └──────────┘",
    ];
    assert_eq!(draw(src), expected);
}

/// `%%`, `;`, `&`, `-- text -->`, `---`, `subgraph`/`style` lines, BT;
/// a self-loop cannot be drawn and is listed under the drawing.
#[test]
fn syntax_variants_and_an_unplaceable_edge() {
    let src = "%% a comment
graph BT;
  subgraph s [Group]
  a & b -- uses --> c;
  end
  c --- d
  style a fill:#f9f
  a --> a";
    let expected = [
        "     ┌───┐",
        "     │ d │",
        "     └─┬─┘",
        "       │",
        "       │",
        "     ┌─┴─┐",
        "     │ c │",
        "     └───┘",
        "       ▲",
        "       ├───┐",
        "   ┌───┘   │",
        " uses    uses",
        "   │       │",
        " ┌─┴─┐   ┌─┴─┐",
        " │ a │   │ b │",
        " └───┘   └───┘",
        "",
        "a ──▶ a",
    ];
    assert_eq!(draw(src), expected);
}

/// RL, `<-.->` as two dotted edges, a stadium as rounded.
#[test]
fn right_to_left_and_both_ways() {
    let src = "flowchart RL
  x([Out]) <-.-> y[In]";
    let expected = [
        "┌────┐   ╭─────╮",
        "│    │   │     │",
        "│ In │◀┄┄┤ Out │",
        "│    ├┄┄▶│     │",
        "└────┘   ╰─────╯",
    ];
    assert_eq!(draw(src), expected);
}

/// Self-messages, solid and dotted lines without heads, `alt`/`else`,
/// a note over two participants.
#[test]
fn sequence_blocks_and_self_messages() {
    let src = "sequenceDiagram
  Alice->>Bob: Hi
  Bob->>Bob: think
  alt is sick
    Bob-->Alice: Not so good
  else is well
    Bob->Alice: Feeling fresh
  end
  Note over Alice,Bob: done";
    let expected = [
        "  ┌───────┐         ┌─────┐",
        "  │ Alice │         │ Bob │",
        "  └───┬───┘         └──┬──┘",
        "      │      Hi        │",
        "      ├───────────────▶│",
        "      │                ├──┐ think",
        "      │                │  │",
        "      │                │◀─┘",
        "┌─ alt: is sick ───────┼────────────┐",
        "│     │  Not so good   │            │",
        "│     ├┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┤            │",
        "├┄ else: is well ┄┄┄┄┄┄┼┄┄┄┄┄┄┄┄┄┄┄┄┤",
        "│     │ Feeling fresh  │            │",
        "│     ├────────────────┤            │",
        "└─────┼────────────────┼────────────┘",
        "    ┌┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┐",
        "    ┆ done               ┆",
        "    └┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┘",
        "      │                │",
    ];
    assert_eq!(draw(src), expected);
}

/// Ruling R6: `title` is a first row, centered and cut to the drawing's
/// width; `box … end` only groups (its participants are kept, no frame).
#[test]
fn sequence_title_and_box() {
    let src = "sequenceDiagram
  title Greetings
  box Purple Team
    participant A as Alice
    participant B as Bob
  end
  A->>B: Hello";
    let expected = [
        "    Greetings",
        "┌───────┐  ┌─────┐",
        "│ Alice │  │ Bob │",
        "└───┬───┘  └──┬──┘",
        "    │  Hello  │",
        "    ├────────▶│",
        "    │         │",
    ];
    assert_eq!(draw(src), expected);
    let long = format!("sequenceDiagram\n  title {}\n  a->>b: hi", "t".repeat(40));
    let rows = draw(&long);
    let widest = rows[1..].iter().map(|r| text::width(r)).max().unwrap();
    assert_eq!(text::width(&rows[0]), widest, "{rows:?}");
    assert!(rows[0].ends_with('…'), "{rows:?}");
    // A box is no frame: `else` cannot split it, and it takes one `end`.
    assert_eq!(
        refused("sequenceDiagram\n  box\n  else x\n  end"),
        "else outside a block"
    );
    assert_eq!(
        refused("sequenceDiagram\n  box\n  a->>b: hi\n  end\n  end"),
        "end without a block"
    );
}

/// AC8: other diagram types and unknown syntax are not drawn.
#[test]
fn unsupported_types_and_syntax() {
    assert_eq!(
        refused("classDiagram\n  Animal <|-- Duck"),
        "unsupported diagram: classDiagram"
    );
    assert_eq!(
        refused("stateDiagram-v2\n  [*] --> A"),
        "unsupported diagram: stateDiagram-v2"
    );
    assert_eq!(refused("pie\n  \"a\": 1"), "unsupported diagram: pie");
    assert_eq!(refused(""), "empty diagram");
    assert_eq!(refused("%% only a comment"), "empty diagram");
    assert_eq!(refused("graph XY\n  a --> b"), "unsupported direction: XY");
    assert_eq!(
        refused("graph TD\n  a --> b[unclosed"),
        "unclosed node shape"
    );
    assert_eq!(refused("graph TD\n  a b"), "cannot parse: a b");
    assert_eq!(refused("sequenceDiagram\n  end"), "end without a block");
    assert_eq!(
        refused("sequenceDiagram\n  Alice says hi"),
        "cannot parse: Alice says hi"
    );
}

/// AC8: hostile input hits each cap and is refused fast.
#[test]
fn hostile_input_hits_each_cap() {
    // Source size, checked before parsing.
    let long = format!("graph TD\n  a[{}] --> b", "x".repeat(MAX_SOURCE_BYTES));
    assert!(too_complex(&long));
    assert_eq!(refused(&long), "diagram source too large");
    let many = format!("graph TD\n{}", "  a --> b\n".repeat(MAX_SOURCE_LINES));
    assert!(too_complex(&many));
    assert_eq!(refused(&many), "diagram source too large");
    assert!(!too_complex("graph TD\n  a --> b"));
    // Nodes and edges, counted as they are parsed.
    let chain: Vec<String> = (0..MAX_NODES)
        .map(|i| format!("  n{i} --> n{}", i + 1))
        .collect();
    let nodes = format!("graph TD\n{}", chain.join("\n"));
    assert_eq!(refused(&nodes), "more than 64 nodes");
    let fan = "graph LR\n  a & b & c & d & e & f & g & h & i & j & k & l --> \
               m & n & o & p & q & r & s & t & u & v & w";
    assert_eq!(refused(fan), "more than 128 edges");
    // Drawing size, computed before the canvas is allocated.
    let wide: Vec<String> = (0..12)
        .map(|i| format!("  r --> c{i}[abcdefghijklmnopqrst]"))
        .collect();
    assert_eq!(
        refused(&format!("graph TD\n{}", wide.join("\n"))),
        "diagram too large"
    );
    let tall: Vec<String> = (0..40).map(|i| format!("  n{i} --> n{}", i + 1)).collect();
    assert_eq!(
        refused(&format!("graph TD\n{}", tall.join("\n"))),
        "diagram too large"
    );
    // Sequence diagrams: participants, messages, rows, nesting.
    let people: Vec<String> = (0..=MAX_NODES)
        .map(|i| format!("  participant p{i}"))
        .collect();
    let people = format!("sequenceDiagram\n{}", people.join("\n"));
    assert_eq!(refused(&people), "more than 64 participants");
    let chatty = format!("sequenceDiagram\n{}", "  a->>b: hi\n".repeat(MAX_EDGES + 1));
    assert_eq!(refused(&chatty), "more than 128 messages");
    let rows = format!("sequenceDiagram\n{}", "  a->>b: hi\n".repeat(80));
    assert_eq!(refused(&rows), "diagram too large");
    let deep = format!("sequenceDiagram\n{}  a->>b: hi\n", "  loop x\n".repeat(9));
    assert_eq!(refused(&deep), "blocks nested too deep");
}

/// The largest graphs the caps allow lay out quickly, with or without a
/// drawing at the end.
#[test]
fn dense_graphs_within_the_caps_are_quick() {
    let mut src = String::from("graph TD\n");
    for i in 0..MAX_NODES {
        src.push_str(&format!("  n{i} --> n{}\n", (i * 7 + 3) % MAX_NODES));
        src.push_str(&format!("  n{i} -.-> n{}\n", (i * 13 + 5) % MAX_NODES));
    }
    assert!(!too_complex(&src));
    let start = Instant::now();
    let _ = render(&src);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

/// Labels wrap at 24 columns, at most 3 lines, and lose what cannot be
/// drawn.
#[test]
fn labels_wrap_and_are_cleaned() {
    assert_eq!(text::wrap("short"), ["short"]);
    assert_eq!(
        text::wrap("one two three four five six seven eight"),
        ["one two three four five", "six seven eight"]
    );
    let long = "word ".repeat(30);
    let lines = text::wrap(&long);
    assert_eq!(lines.len(), text::MAX_LINES);
    assert!(lines[2].ends_with('…'), "{lines:?}");
    assert!(lines.iter().all(|l| text::width(l) <= text::WRAP));
    assert_eq!(text::wrap(&"x".repeat(30)), ["x".repeat(24), "x".repeat(6)]);
    assert_eq!(
        text::clean("a<br>b\u{1b}[31m\u{200b}\u{301}\tc"),
        "a\nb[31m c"
    );
}
