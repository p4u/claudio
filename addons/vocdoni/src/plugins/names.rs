//! The charsets of a plugin's names and env keys, re-exported by
//! [`super::manifest`] (the parser of the names they check).

/// `[a-z0-9-]{1,40}`: a plugin name, and a path component on every host.
pub fn name_ok(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `[A-Z_][A-Z0-9_]{0,63}`, and not one a plugin may not set.
pub fn key_ok(key: &str) -> Result<(), String> {
    let mut bytes = key.bytes();
    let shape = key.len() <= 64
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    if !shape {
        return Err("bad env key".to_owned());
    }
    let upper = key.to_ascii_uppercase();
    if DENIED_PREFIXES.iter().any(|p| upper.starts_with(p)) || DENIED_KEYS.contains(&&*upper) {
        return Err(format!("env key {key} not allowed"));
    }
    Ok(())
}

/// Env key prefixes a plugin may not set: the proxy's own and Claude Code's
/// switches, the loaders and the toolchains that read code from the env.
const DENIED_PREFIXES: &[&str] = &[
    "ANTHROPIC_",
    "CLAUDIO_",
    "CLAUDE_",
    "NODE_",
    "LD_",
    "DYLD_",
    "GIT_",
    "SSL_",
    "XDG_",
    "SSH_",
];

/// Env keys a plugin may not set (nor their lowercase forms).
const DENIED_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "SHELL",
    "TERM",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "BASH_ENV",
    "ENV",
    "PYTHONPATH",
    "PERL5OPT",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
];
