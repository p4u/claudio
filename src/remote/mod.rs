//! Remote host support for the session manager.
//!
//! - [`probe`] — `claudio __probe`: prints a JSON fingerprint of this binary.
//! - [`bridge`] — `claudio --slave`: the remote-side stdio↔socket bridge.
//! - [`hosts`] — SSH host alias discovery and MRU persistence.
//! - [`bootstrap`] — install or update the claudio binary on a remote host.
//! - [`diag`] — diagnostic / test-harness subcommands (`__bootstrap`, etc.),
//!   enabled only with `--features diag`.

pub mod bootstrap;
pub mod bridge;
#[cfg(feature = "diag")]
pub mod diag;
pub mod hosts;
pub mod probe;

use tokio::process::Command;

/// Reject hosts starting with `-` (ssh option injection).
pub fn validate_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("empty hostname".to_owned());
    }
    if host.starts_with('-') {
        return Err(format!("invalid host {host:?}: hostnames must not start with '-'"));
    }
    Ok(())
}

/// Build the standard `ssh` invocation shared by bootstrap and the client
/// bridge: `-T -o BatchMode=yes -o ConnectTimeout=15
/// -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -- HOST`.
///
/// Callers append the remote command as one more `.arg(…)` call.
pub fn ssh_cmd(host: &str) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "--",
        host,
    ]);
    cmd
}

/// Wrap `s` in single quotes, escaping any embedded single quote
/// (`'` → `'\''`). Safe for POSIX shells regardless of content.
///
/// Use this for any user-supplied value interpolated into a remote shell
/// command string, e.g. `format!("cat > {}", shell_quote(path))`.
/// Do **not** use it when the value must undergo shell variable expansion
/// (e.g. `$HOME/.local/bin/claudio`).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_host_rejects_dash_prefix() {
        assert!(validate_host("-foo").is_err());
        assert!(validate_host("--foo").is_err());
        assert!(validate_host("").is_err());
    }

    #[test]
    fn validate_host_accepts_normal_hosts() {
        assert!(validate_host("z6").is_ok());
        assert!(validate_host("user@host.example.com").is_ok());
        assert!(validate_host("192.168.1.1").is_ok());
    }

    #[test]
    fn shell_quote_handles_spaces_and_special_chars() {
        assert_eq!(shell_quote("simple"), "'simple'");
        assert_eq!(shell_quote("has spaces"), "'has spaces'");
        assert_eq!(shell_quote("with'quote"), r"'with'\''quote'");
        assert_eq!(shell_quote("/home/user name/bin"), "'/home/user name/bin'");
        assert_eq!(shell_quote("glob*chars?[here]"), "'glob*chars?[here]'");
    }

    #[test]
    fn shell_quote_home_with_spaces() {
        // Simulate building a remote command for a home dir with spaces.
        let home = "/home/my user";
        let quoted = shell_quote(&format!("{home}/.local/bin/claudio"));
        let cmd = format!("cat > {quoted}");
        assert_eq!(cmd, "cat > '/home/my user/.local/bin/claudio'");
    }
}
