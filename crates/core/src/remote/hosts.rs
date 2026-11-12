//! SSH host aliases from `~/.ssh/config` and the MRU store.
//!
//! [`candidates`] returns the list shown in the wizard's "Where" step:
//! MRU hosts first (most recent first), then every ssh config alias in file
//! order. Wildcard patterns (`*`, `?`, `!`) are filtered out because they
//! don't describe a concrete host.
//!
//! [`touch`] updates the MRU list in `~/.config/claudio/hosts.json` whenever
//! a remote host is successfully connected.
//!
//! [`forward_agent`] and [`set_forward_agent`] read and write `config.toml`'s
//! core section `[ssh] forward_agent`: the hosts whose bridge ssh asks for
//! agent forwarding.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::SaveError;
use crate::paths;

/// The hosts MRU file.
fn mru_path() -> PathBuf {
    paths::config_dir().join("hosts.json")
}

// ── MRU ──────────────────────────────────────────────────────────────────────

/// The persisted MRU list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HostsMru {
    /// Most recently used first.
    #[serde(default)]
    hosts: Vec<String>,
}

/// Load the MRU list. A missing or unreadable file returns an empty list.
fn load_mru() -> HostsMru {
    fs::read(mru_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Save the MRU list atomically.
fn save_mru(mru: &HostsMru) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(mru).map_err(std::io::Error::other)?;
    paths::write_atomic(&mru_path(), &json)
}

/// Record a successful connection to `host`, moving it to the front of the
/// MRU list.
pub fn touch(host: &str) {
    let mut mru = load_mru();
    mru.hosts.retain(|h| h != host);
    mru.hosts.insert(0, host.to_owned());
    // Keep the list from growing without bound.
    mru.hosts.truncate(50);
    let _ = save_mru(&mru);
}

// ── [ssh] forward_agent ───────────────────────────────────────────────────────

/// The core section of per-host ssh options, and its forwarding key.
const SSH_SECTION: &str = "ssh";
const FORWARD_AGENT: &str = "forward_agent";

/// Whether `host`, exactly as connected, is in `config.toml`'s
/// `[ssh] forward_agent`, read now.
pub fn forward_agent(host: &str) -> bool {
    forward_agent_hosts().iter().any(|h| h == host)
}

/// `config.toml`'s `[ssh] forward_agent`, read now ([`forward_agent_in`]).
pub fn forward_agent_hosts() -> Vec<String> {
    forward_agent_in(&paths::config_file())
}

/// `[ssh] forward_agent` of the config file at `path`: its entries that
/// [`super::validate_host`] takes, in order. A missing or unreadable file,
/// section or key, or a value that is not an array, reads as empty; the rest
/// of the file is not looked at.
pub fn forward_agent_in(path: &Path) -> Vec<String> {
    let table = fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok());
    let items = table
        .as_ref()
        .and_then(|t| t.get(SSH_SECTION)?.get(FORWARD_AGENT)?.as_array());
    items
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .filter(|h| super::validate_host(h).is_ok())
        .map(str::to_owned)
        .collect()
}

/// [`set_forward_agent_in`] on `config.toml`: the one writer of
/// `[ssh] forward_agent`; front-ends call it.
pub fn set_forward_agent(host: &str, on: bool) -> Result<bool, SaveError> {
    set_forward_agent_in(&paths::config_file(), host, on)
}

/// Put `host` in (`on`) or take it out of `[ssh] forward_agent` of the
/// config file at `path` with [`crate::app::config_edit::edit_section`]
/// (atomic, minimal, serialised). On: the first copy stays where it is, a
/// later one goes, and a new host goes at the end. Off: every copy goes.
/// Other entries keep their order; a value that is not an array is
/// replaced. Whether the file changed.
pub fn set_forward_agent_in(path: &Path, host: &str, on: bool) -> Result<bool, SaveError> {
    super::validate_host(host).map_err(SaveError::BadHost)?;
    crate::app::config_edit::edit_section(path, SSH_SECTION, |table| {
        let mut items = match table.remove(FORWARD_AGENT) {
            Some(toml::Value::Array(items)) => items,
            _ => Vec::new(),
        };
        let is_host = |v: &toml::Value| v.as_str() == Some(host);
        let had = items.iter().any(is_host);
        let mut first = true;
        items.retain(|v| !is_host(v) || (on && std::mem::replace(&mut first, false)));
        if on && !had {
            items.push(toml::Value::String(host.to_owned()));
        }
        table.insert(FORWARD_AGENT.to_owned(), toml::Value::Array(items));
    })
}

// ── SSH config parsing ────────────────────────────────────────────────────────

/// Recursion depth limit for `Include` directives.
const MAX_INCLUDE_DEPTH: usize = 5;

/// Parse `~/.ssh/config` (and its `Include` files) and return every concrete
/// host alias in file order. Patterns containing `*`, `?`, or `!` are skipped.
pub fn ssh_hosts() -> Vec<String> {
    let config = home_ssh().join("config");
    let mut seen_files = HashSet::new();
    let mut out = Vec::new();
    parse_file(&config, 0, &home_ssh(), &mut seen_files, &mut out);
    out
}

/// Combined candidate list for the wizard: MRU first, then any ssh alias not
/// already in the MRU list, in config file order.
pub fn candidates() -> Vec<String> {
    let mru = load_mru();
    let mru_set: HashSet<&str> = mru.hosts.iter().map(String::as_str).collect();
    let mut out: Vec<String> = mru.hosts.clone();
    for h in ssh_hosts() {
        if !mru_set.contains(h.as_str()) {
            out.push(h);
        }
    }
    out
}

fn home_ssh() -> PathBuf {
    crate::paths::home().join(".ssh")
}

/// Parse one ssh config file, recursing into `Include`s.
fn parse_file(
    path: &Path,
    depth: usize,
    ssh_dir: &Path,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<String>,
) {
    if depth > MAX_INCLUDE_DEPTH {
        return;
    }
    // Canonicalize to avoid double-visiting the same file.
    let canon = match path.canonicalize() {
        Ok(c) => c,
        Err(_) => return,
    };
    if !seen.insert(canon) {
        return;
    }
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return,
    };
    parse_text(&text, depth, ssh_dir, seen, out);
}

/// Parse the text of one ssh config file.
fn parse_text(
    text: &str,
    depth: usize,
    ssh_dir: &Path,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<String>,
) {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Split into keyword and value.
        let (kw, rest) = match line.split_once(|c: char| c.is_ascii_whitespace()) {
            Some(pair) => pair,
            None => continue,
        };
        let rest = rest.trim();
        match kw.to_ascii_lowercase().as_str() {
            "host" => {
                // A line can have multiple aliases: `Host foo bar baz`.
                for alias in rest.split_ascii_whitespace() {
                    if is_concrete(alias) {
                        out.push(alias.to_owned());
                    }
                }
            }
            "include" => {
                // Expand globs relative to the ssh directory.
                for pattern in rest.split_ascii_whitespace() {
                    let pattern_path = if pattern.starts_with('/') {
                        PathBuf::from(pattern)
                    } else {
                        ssh_dir.join(pattern)
                    };
                    let expanded = expand_glob(&pattern_path);
                    for p in expanded {
                        parse_file(&p, depth + 1, ssh_dir, seen, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// A host alias is "concrete" (usable as a target) when it contains none of
/// the ssh pattern metacharacters.
fn is_concrete(alias: &str) -> bool {
    !alias.chars().any(|c| matches!(c, '*' | '?' | '!'))
}

/// Expand a glob pattern and return matching paths (alphabetical order).
fn expand_glob(pattern: &Path) -> Vec<PathBuf> {
    let pattern_str = pattern.to_string_lossy();
    // Simple glob: only `*` in the filename component.
    let dir = match pattern.parent() {
        Some(d) => d.to_path_buf(),
        None => return Vec::new(),
    };
    let file_pattern = match pattern.file_name().and_then(|n| n.to_str()) {
        Some(f) => f,
        None => {
            return if pattern.exists() {
                vec![pattern.to_path_buf()]
            } else {
                Default::default()
            }
        }
    };
    if !pattern_str.contains('*') && !pattern_str.contains('?') {
        return if pattern.exists() {
            vec![pattern.to_path_buf()]
        } else {
            Vec::new()
        };
    }
    let entries = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    let mut matched: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            glob_match(file_pattern, &name)
        })
        .map(|e| e.path())
        .take(MAX_GLOB_FILES) // bound file count
        .collect();
    matched.sort();
    matched
}

/// Maximum files returned per glob expansion (DoS protection).
const MAX_GLOB_FILES: usize = 256;

/// Minimal glob match: `*` matches any sequence of characters, `?` one char.
///
/// Uses an iterative O(n·m) algorithm with last-star backtracking.  A pattern
/// with many `*`s followed by a non-matching suffix cannot cause exponential
/// blow-up.
fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    // `star_pi` / `star_ni`: position of the last `*` match.
    let (mut star_pi, mut star_ni) = (usize::MAX, 0usize);

    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            // Record where we saw the star and move pattern forward.
            star_pi = pi;
            star_ni = ni; // star matches 0 chars initially
            pi += 1;
        } else if star_pi != usize::MAX {
            // Backtrack: the star absorbs one more name char.
            star_ni += 1;
            ni = star_ni;
            pi = star_pi + 1;
        } else {
            return false;
        }
    }
    // Skip trailing stars.
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch() -> PathBuf {
        let tmp = std::env::temp_dir().join(format!("claudio-hosts-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    #[test]
    fn is_concrete_filters_wildcards() {
        assert!(is_concrete("devbox"));
        assert!(is_concrete("user@host.example.com"));
        assert!(!is_concrete("*.example.com"));
        assert!(!is_concrete("!bastion"));
        assert!(!is_concrete("host?"));
    }

    #[test]
    fn parse_multi_alias_and_comments() {
        let text = "# header\nHost foo bar baz\n  IdentityFile ~/.ssh/id_ed25519\nHost *.wild\nHost prod\n";
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        parse_text(text, 0, Path::new("/tmp"), &mut seen, &mut out);
        assert_eq!(out, vec!["foo", "bar", "baz", "prod"]);
    }

    #[test]
    fn parse_case_insensitive_keyword() {
        let text = "HOST myserver\n  User root\n";
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        parse_text(text, 0, Path::new("/tmp"), &mut seen, &mut out);
        assert_eq!(out, vec!["myserver"]);
    }

    #[test]
    fn include_with_glob_is_resolved() {
        let dir = scratch();
        let subdir = dir.join("conf.d");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join("10-work.conf"), "Host work1\nHost work2\n").unwrap();
        fs::write(subdir.join("20-home.conf"), "Host home1\n").unwrap();
        let main_config = dir.join("config");
        fs::write(&main_config, "Include conf.d/*.conf\nHost direct\n").unwrap();

        let mut seen = HashSet::new();
        let mut out = Vec::new();
        parse_file(&main_config, 0, &dir, &mut seen, &mut out);
        // Include files are resolved; order: files alphabetically within glob,
        // then direct.
        assert!(out.contains(&"work1".to_owned()));
        assert!(out.contains(&"work2".to_owned()));
        assert!(out.contains(&"home1".to_owned()));
        assert!(out.contains(&"direct".to_owned()));
        // Direct comes last because Include appears first in the file.
        assert_eq!(out.last().unwrap(), "direct");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn include_depth_limit_stops_infinite_recursion() {
        let dir = scratch();
        let config = dir.join("config");
        // A config that includes itself.
        fs::write(&config, "Include config\nHost loop\n").unwrap();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        parse_file(&config, 0, &dir, &mut seen, &mut out);
        // Should not panic or loop forever; the self-include is skipped.
        assert_eq!(out, vec!["loop"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mru_ordering() {
        // Test the MRU logic in isolation, without touching the real config dir.
        let mut mru = HostsMru {
            hosts: vec!["a".into(), "b".into(), "c".into()],
        };
        let host = "b";
        mru.hosts.retain(|h| h != host);
        mru.hosts.insert(0, host.to_owned());
        assert_eq!(mru.hosts, vec!["b", "a", "c"]);
    }

    #[test]
    fn candidates_deduplicates_mru_and_config() {
        // Build a temporary ssh config with known hosts.
        let ssh_dir = scratch();
        let config = ssh_dir.join("config");
        fs::write(&config, "Host alpha\nHost beta\nHost gamma\n").unwrap();
        // Simulate: alpha is in MRU (most recent first), beta is also in MRU.
        let mru = HostsMru {
            hosts: vec!["alpha".into(), "delta".into()],
        };
        // Manually merge like candidates() does.
        let mru_set: HashSet<&str> = mru.hosts.iter().map(String::as_str).collect();
        let mut seen_files = HashSet::new();
        let mut config_hosts = Vec::new();
        parse_file(&config, 0, &ssh_dir, &mut seen_files, &mut config_hosts);
        let mut result = mru.hosts.clone();
        for h in config_hosts {
            if !mru_set.contains(h.as_str()) {
                result.push(h);
            }
        }
        assert_eq!(result, vec!["alpha", "delta", "beta", "gamma"]);
        fs::remove_dir_all(&ssh_dir).unwrap();
    }

    #[test]
    fn glob_match_patterns() {
        assert!(glob_match("*.conf", "foo.conf"));
        assert!(!glob_match("*.conf", "foo.txt"));
        assert!(glob_match("10-*.conf", "10-work.conf"));
        assert!(glob_match("host?", "host1"));
        assert!(!glob_match("host?", "host10"));
    }

    #[test]
    fn glob_worst_case_finishes_fast() {
        // A pattern with many stars followed by a non-matching suffix.
        // The recursive approach would be exponential; the iterative one is O(n*m).
        let pattern = "*".repeat(20) + "x";
        let name = "a".repeat(100);
        let start = std::time::Instant::now();
        assert!(!glob_match(&pattern, &name));
        assert!(start.elapsed().as_millis() < 10, "glob_match took too long");
    }

    /// A config file in a new private temp dir, holding `text`.
    fn config_with(text: &str) -> PathBuf {
        let path = scratch().join("config.toml");
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn forward_agent_is_added_removed_and_deduplicated_in_order() {
        let text =
            "# mine\n[claude]\ninstall = \"off\"\n\n[ssh]\nforward_agent = [\"a\", \"b\", \"a\"]\n";
        let path = config_with(text);
        assert_eq!(forward_agent_in(&path), ["a", "b", "a"]);
        // On: the first copy stays, a later one goes.
        assert!(set_forward_agent_in(&path, "a", true).unwrap());
        assert_eq!(forward_agent_in(&path), ["a", "b"]);
        assert!(
            !set_forward_agent_in(&path, "a", true).unwrap(),
            "already on"
        );
        assert!(set_forward_agent_in(&path, "me@10.0.0.2", true).unwrap());
        assert_eq!(forward_agent_in(&path), ["a", "b", "me@10.0.0.2"]);
        // Off: every copy goes; the others keep their order.
        assert!(set_forward_agent_in(&path, "a", false).unwrap());
        assert_eq!(forward_agent_in(&path), ["b", "me@10.0.0.2"]);
        assert!(
            !set_forward_agent_in(&path, "a", false).unwrap(),
            "already off"
        );
        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.starts_with("# mine\n[claude]\ninstall = \"off\"\n"),
            "{after}"
        );
        let table: toml::Table = after.parse().unwrap();
        assert_eq!(table.len(), 2, "{after}");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn forward_agent_section_is_made_when_missing() {
        let path = config_with("[ui]\nnotify = true\n");
        assert!(forward_agent_in(&path).is_empty());
        assert!(set_forward_agent_in(&path, "devbox", true).unwrap());
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.starts_with("[ui]\nnotify = true\n"), "{after}");
        assert!(
            after.contains("[ssh]\nforward_agent = [\"devbox\"]"),
            "{after}"
        );
        assert_eq!(forward_agent_in(&path), ["devbox"]);
        // No file at all reads as empty, and is created on a write.
        let missing = path.with_file_name("none.toml");
        assert!(forward_agent_in(&missing).is_empty());
        assert!(set_forward_agent_in(&missing, "devbox", true).unwrap());
        assert_eq!(forward_agent_in(&missing), ["devbox"]);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn forward_agent_refuses_and_ignores_bad_hosts() {
        let path = config_with("[ssh]\nforward_agent = [\"-oProxyCommand=x\", \"\", \"ok\", 3]\n");
        assert_eq!(forward_agent_in(&path), ["ok"], "bad entries are ignored");
        for bad in ["-oProxyCommand=x", ""] {
            let err = set_forward_agent_in(&path, bad, true).unwrap_err();
            assert!(matches!(err, SaveError::BadHost(_)), "{bad}: {err}");
        }
        assert!(crate::addon::CORE_SECTIONS.contains(&SSH_SECTION));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn forward_agent_that_is_not_an_array_reads_as_empty() {
        let path = config_with("[claude]\ninstall = \"off\"\n[ssh]\nforward_agent = \"devbox\"\n");
        assert!(forward_agent_in(&path).is_empty());
        let config = crate::config::load_from(&path);
        assert_eq!(
            config.claude.install,
            crate::config::UpdatePolicy::Off,
            "the rest still loads"
        );
        assert!(set_forward_agent_in(&path, "devbox", true).unwrap());
        assert_eq!(forward_agent_in(&path), ["devbox"]);
        let path2 = config_with("ssh = 1\n");
        assert!(forward_agent_in(&path2).is_empty());
        assert!(matches!(
            set_forward_agent_in(&path2, "devbox", true),
            Err(SaveError::UnsupportedSection(_))
        ));
        for p in [&path, &path2] {
            fs::remove_dir_all(p.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn glob_match_multiple_stars() {
        assert!(glob_match("a*b*c", "aXbYc"));
        assert!(!glob_match("a*b*c", "aXbY"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("**", "anything"));
        assert!(!glob_match("a*x", "abc"));
    }
}
