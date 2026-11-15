//! Claude Code's environment variables as claudio's custom env sees them
//! (docs/specs/2026-11-15-custom-env-design.md): the names claudio refuses,
//! the one validator of a name and a value, the caps, the TUI's escaped form
//! of a value, and the embedded catalogue of the documented variables.
//!
//! A value is a secret: it travels in [`Value`], whose `Debug` prints its
//! length only, and no error here ever quotes one (C1).

use std::fmt;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::SESSION_MARKERS;

/// The longest name, in bytes (ASCII).
pub const MAX_NAME: usize = 128;
/// The longest value, in UTF-8 bytes.
pub const MAX_VALUE: usize = 8192;
/// The most variables in one scope.
pub const MAX_VARS: usize = 128;
/// The most bytes of names and values together in one scope.
pub const MAX_SCOPE_BYTES: usize = 128 * 1024;

/// Why a name is refused, besides [`SESSION_MARKERS`] (refused because claude
/// sets them for its own children).
const REFUSED: &[(&str, &str)] = &[
    (
        "PATH",
        "the daemon sets it (the login shell's), and claude must stay on it",
    ),
    ("TERM", "the session's terminal sets it"),
    ("COLORTERM", "the session's terminal sets it"),
    ("CLAUDE_CODE_REMOTE", "Claude Code sets it for its host"),
    (
        "CLAUDE_CODE_ACCOUNT_UUID",
        "Claude Code sets it for its host",
    ),
    (
        "CLAUDE_CODE_SIMPLE",
        "it turns off the hooks claudio relies on",
    ),
    (
        "CLAUDE_CODE_SAFE_MODE",
        "it turns off the hooks claudio relies on",
    ),
];

/// Why a [`SESSION_MARKERS`] name is refused.
const MARKER: &str = "claude sets it for its own children; a session with it believes it is one";

/// An env value: a secret. Its `Debug` prints `<n bytes>`, never the text.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Value(String);

impl Value {
    /// `text` as a value (not checked: see [`check_value`]).
    pub fn new(text: impl Into<String>) -> Value {
        Value(text.into())
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The text, owned.
    pub fn into_string(self) -> String {
        self.0
    }

    /// Its length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.0.len())
    }
}

/// Why a name or a value is not taken. Its text never quotes a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not `[A-Za-z_][A-Za-z0-9_]{0,127}`.
    BadName,
    /// A name claudio refuses, and why.
    Refused(&'static str),
    /// Over [`MAX_VALUE`] bytes.
    TooLong,
    /// A control character other than newline and tab.
    Control,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::BadName => write!(
                f,
                "not a variable name (letters, digits and _, not starting with a digit, at most {MAX_NAME})"
            ),
            Refusal::Refused(why) => write!(f, "refused: {why}"),
            Refusal::TooLong => write!(f, "value over {MAX_VALUE} bytes"),
            Refusal::Control => write!(
                f,
                "value holds a control character (only newline and tab are allowed)"
            ),
        }
    }
}

/// Whether `name` is `[A-Za-z_][A-Za-z0-9_]{0,127}`.
pub fn name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_NAME
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Why claudio refuses `name` (exact case), if it does.
pub fn refused(name: &str) -> Option<&'static str> {
    if SESSION_MARKERS.contains(&name) {
        return Some(MARKER);
    }
    REFUSED
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, why)| *why)
}

/// Every refused name and why: the session markers, then the others.
pub fn refused_names() -> Vec<(&'static str, &'static str)> {
    let markers = SESSION_MARKERS.iter().map(|n| (*n, MARKER));
    markers.chain(REFUSED.iter().copied()).collect()
}

/// A name a user may set: well formed and not refused.
pub fn check_name(name: &str) -> Result<(), Refusal> {
    if !name_ok(name) {
        return Err(Refusal::BadName);
    }
    match refused(name) {
        Some(why) => Err(Refusal::Refused(why)),
        None => Ok(()),
    }
}

/// A value a user may set: at most [`MAX_VALUE`] bytes, and no character
/// for which `char::is_control` holds but `\n` and `\t` (so NUL, `\r`, ESC,
/// DEL and U+0080–U+009F are refused). Empty is allowed.
pub fn check_value(value: &str) -> Result<(), Refusal> {
    if value.len() > MAX_VALUE {
        return Err(Refusal::TooLong);
    }
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(Refusal::Control);
    }
    Ok(())
}

/// `name` and `value` both taken; the error names the variable when its
/// name is well formed (never the value).
pub fn check(name: &str, value: &str) -> Result<(), String> {
    check_name(name).map_err(|e| shown(name, &e))?;
    check_value(value).map_err(|e| shown(name, &e))
}

/// `why` about variable `name`: `NAME: why` when the name is well formed,
/// else `why` alone (a bad key may be a value typed on the wrong side).
pub fn shown(name: &str, why: &impl fmt::Display) -> String {
    if name_ok(name) {
        format!("{name}: {why}")
    } else {
        why.to_string()
    }
}

/// `value` as the TUI shows and edits it: a newline as `\n`, a tab as `\t`,
/// a backslash as `\\`.
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out
}

/// The TUI's escaped text back to the value: `\n`, `\t` and `\\`; a
/// backslash before any other character, or at the end, is an error.
/// `unescape(&escape(v)) == Ok(v)` for every `v`.
pub fn unescape(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                let other = if other.is_control() { '?' } else { other };
                return Err(format!(
                    "unknown escape \\{other}: write \\\\ for a backslash"
                ));
            }
            None => return Err("a lone \\ at the end: write \\\\ for a backslash".to_owned()),
        }
    }
    Ok(out)
}

/// One documented Claude Code variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownVar {
    pub name: &'static str,
    /// One word: `model`, `auth`, `network`…
    pub category: &'static str,
    /// At most 160 characters; says when a flag is presence-only.
    pub description: &'static str,
}

/// The catalogue as embedded: `NAME\tcategory\tdescription` per line, `#`
/// comments.
const CATALOGUE: &str = include_str!("env_catalogue.tsv");

/// Claude Code's documented variables (the embedded catalogue), in name
/// order, parsed once. A malformed line is left out.
pub fn catalogue() -> &'static [KnownVar] {
    static PARSED: OnceLock<Vec<KnownVar>> = OnceLock::new();
    PARSED.get_or_init(|| parse_catalogue(CATALOGUE))
}

/// The catalogue entry of `name`, if it is documented.
pub fn known(name: &str) -> Option<&'static KnownVar> {
    let list = catalogue();
    list.binary_search_by(|v| v.name.cmp(name))
        .ok()
        .and_then(|i| list.get(i))
}

/// `text`'s well-formed lines, sorted by name.
fn parse_catalogue(text: &'static str) -> Vec<KnownVar> {
    let mut list: Vec<KnownVar> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut cells = l.split('\t');
            let (name, category, description) = (cells.next()?, cells.next()?, cells.next()?);
            let ok = cells.next().is_none() && name_ok(name) && !category.is_empty();
            ok.then_some(KnownVar {
                name,
                category,
                description,
            })
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(b.name));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_letters_digits_and_underscores_not_starting_with_a_digit() {
        for good in [
            "A",
            "_",
            "https_proxy",
            "no_proxy",
            "MAX_THINKING_TOKENS",
            "a1",
        ] {
            assert_eq!(check_name(good), Ok(()), "{good}");
        }
        let long = "A".repeat(MAX_NAME);
        assert_eq!(check_name(&long), Ok(()));
        for bad in [
            "",
            "1A",
            "A-B",
            "A B",
            "Ä",
            "A=B",
            "A.B",
            &"A".repeat(MAX_NAME + 1),
        ] {
            assert_eq!(check_name(bad), Err(Refusal::BadName), "{bad}");
        }
    }

    #[test]
    fn the_refused_set_is_the_session_markers_and_the_seven_names() {
        for name in SESSION_MARKERS {
            assert!(
                matches!(check_name(name), Err(Refusal::Refused(_))),
                "{name}"
            );
        }
        for name in [
            "PATH",
            "TERM",
            "COLORTERM",
            "CLAUDE_CODE_REMOTE",
            "CLAUDE_CODE_ACCOUNT_UUID",
            "CLAUDE_CODE_SIMPLE",
            "CLAUDE_CODE_SAFE_MODE",
        ] {
            assert!(
                matches!(check_name(name), Err(Refusal::Refused(_))),
                "{name}"
            );
        }
        assert_eq!(refused_names().len(), SESSION_MARKERS.len() + 7);
        // Exact case: `path` is another variable.
        assert_eq!(check_name("path"), Ok(()));
        let why = check("PATH", "x").unwrap_err();
        assert!(why.starts_with("PATH: refused: "), "{why}");
    }

    #[test]
    fn values_allow_newlines_and_tabs_and_refuse_other_controls_and_length() {
        for good in ["", "x", "a\nb", "a\tb", "Name: v\nOther: w", "ü € 😀"] {
            assert_eq!(check_value(good), Ok(()), "{good:?}");
        }
        assert_eq!(check_value(&"x".repeat(MAX_VALUE)), Ok(()));
        assert_eq!(
            check_value(&"x".repeat(MAX_VALUE + 1)),
            Err(Refusal::TooLong)
        );
        // 8192 bytes, not characters: 2731 × 'é' (2 bytes) is 5462 bytes, fine;
        // 4097 × 'é' is 8194.
        assert_eq!(check_value(&"é".repeat(4097)), Err(Refusal::TooLong));
        for bad in ["a\0b", "a\rb", "\x1b[31m", "a\x7fb", "a\u{85}b", "a\u{9f}b"] {
            assert_eq!(check_value(bad), Err(Refusal::Control), "{bad:?}");
        }
    }

    #[test]
    fn errors_name_a_well_formed_key_and_never_the_value() {
        let secret = "sk-SECRET\0";
        let why = check("TOKEN", secret).unwrap_err();
        assert!(why.starts_with("TOKEN: value holds a control"), "{why}");
        assert!(!why.contains("sk-SECRET"));
        // A bad key may be a value typed on the wrong side: not quoted.
        let why = check("sk-SECRET", "v").unwrap_err();
        assert!(!why.contains("sk-SECRET"), "{why}");
    }

    #[test]
    fn the_escaped_form_round_trips_and_refuses_unknown_escapes() {
        for v in [
            "",
            "plain",
            "a\nb",
            "\t",
            "C:\\new",
            "\\\\",
            "x\\",
            "\\n literal",
            "ü\n\t\\",
        ] {
            assert_eq!(unescape(&escape(v)).as_deref(), Ok(v), "{v:?}");
        }
        assert_eq!(escape("a\nb\tc\\d"), "a\\nb\\tc\\\\d");
        // `C:\\new` is the path; `C:\new` is "C:", a newline and "ew" (`\n`
        // is an escape), and `C:\Users` is refused: `\U` is none.
        assert_eq!(unescape("C:\\\\new").as_deref(), Ok("C:\\new"));
        assert_eq!(unescape("C:\\new").as_deref(), Ok("C:\new"));
        assert_eq!(
            unescape("C:\\Users").unwrap_err(),
            "unknown escape \\U: write \\\\ for a backslash"
        );
        assert!(unescape("end\\").unwrap_err().contains("at the end"));
    }

    #[test]
    fn value_debug_prints_the_length_only() {
        let v = Value::new("sk-SECRET-value");
        assert_eq!(format!("{v:?}"), "<15 bytes>");
        let pair = vec![("TOKEN".to_owned(), v)];
        let shown = format!("{pair:?} {:?}", Some(&pair));
        assert!(!shown.contains("SECRET"), "{shown}");
        assert_eq!(
            serde_json::to_string(&pair[0].1).unwrap(),
            "\"sk-SECRET-value\""
        );
    }

    #[test]
    fn the_catalogue_parses_with_unique_names_and_no_refused_one() {
        let list = catalogue();
        assert_eq!(
            list.len(),
            CATALOGUE.lines().filter(|l| !l.starts_with('#')).count()
        );
        assert!(list.len() > 400, "{}", list.len());
        for pair in list.windows(2) {
            assert!(
                pair[0].name < pair[1].name,
                "{} twice or unsorted",
                pair[1].name
            );
        }
        for v in list {
            assert_eq!(check_name(v.name), Ok(()), "{} is refused or bad", v.name);
            assert!(v.description.chars().count() <= 160, "{}", v.name);
            assert!(!v.category.contains(' '), "{}", v.name);
        }
        let opus = known("ANTHROPIC_DEFAULT_OPUS_MODEL").expect("documented");
        assert_eq!(opus.category, "model");
        assert!(
            known("DISABLE_TELEMETRY").is_some_and(|v| v.description.starts_with("Presence-only"))
        );
        assert!(known("PATH").is_none() && known("CLAUDECODE").is_none());
    }

    #[test]
    fn a_malformed_catalogue_line_is_left_out() {
        let list =
            parse_catalogue("# c\nA\tcat\tok\nbad line\n1X\tcat\tbad name\nB\tcat\tx\textra\n\n");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "A");
    }
}
