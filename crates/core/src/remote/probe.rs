//! `claudio __probe` — prints one JSON line describing this binary.
//!
//! The bootstrapper runs this on the remote host to decide whether an upload
//! is needed. The `build` hash uses SHA-256 of the running binary so that dev
//! builds with the same version string are still distinguished.
//!
//! The same facts identify the binary a daemon runs (its `Welcome`), so a
//! client can tell an outdated daemon from a current one ([`crate::freshness`]).

use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::proto::PROTO;

/// What `__probe` prints and what the bootstrapper reads back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    pub version: String,
    pub proto: u32,
    pub os: String,
    pub arch: String,
    /// SHA-256 of the running binary, lower-case hex.
    pub build: String,
    /// The binary's modification time (Unix seconds): orders two builds of
    /// one version. Absent from older binaries.
    #[serde(default)]
    pub mtime: Option<u64>,
}

impl Probe {
    /// Build a [`Probe`] describing this running binary.
    ///
    /// On Linux this reads `/proc/self/exe`: the running code, even once the
    /// file on disk has been replaced (an upgrade, a bootstrap upload).
    pub fn current() -> io::Result<Probe> {
        let exe = if cfg!(target_os = "linux") {
            PathBuf::from("/proc/self/exe")
        } else {
            std::env::current_exe()?
        };
        let bytes = std::fs::read(&exe)?;
        let hash = Sha256::digest(&bytes);
        let build: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        let mtime = std::fs::metadata(&exe)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        Ok(Probe {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            proto: PROTO,
            os: os_name(),
            arch: arch_name(),
            build,
            mtime,
        })
    }

    /// This process's probe, computed once (the first call reads and hashes
    /// the binary). `None` when the binary cannot be read.
    pub fn own() -> Option<&'static Probe> {
        static OWN: OnceLock<Option<Probe>> = OnceLock::new();
        OWN.get_or_init(|| Probe::current().ok()).as_ref()
    }

    /// Parse a probe JSON line from the remote binary's stdout.
    pub fn parse(line: &str) -> Option<Probe> {
        serde_json::from_str(line.trim()).ok()
    }

    #[cfg(test)]
    pub fn is_up_to_date(&self, local: &Probe) -> bool {
        self.build == local.build
    }

    #[cfg(test)]
    pub fn same_platform(&self, local: &Probe) -> bool {
        self.os == local.os && self.arch == local.arch
    }
}

/// Print the probe JSON to stdout and return. Stdout must stay clean because
/// `__probe` may be called over an SSH connection where stdout is the channel.
pub fn run() -> std::process::ExitCode {
    match Probe::current() {
        Ok(probe) => match serde_json::to_string(&probe) {
            Ok(json) => {
                println!("{json}");
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("claudio: __probe: serialize: {e}");
                std::process::ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("claudio: __probe: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// The os tag used in probe JSON and release asset names (`linux` / `macos`).
pub fn os_name() -> String {
    if cfg!(target_os = "linux") {
        "linux".to_owned()
    } else if cfg!(target_os = "macos") {
        "macos".to_owned()
    } else {
        std::env::consts::OS.to_owned()
    }
}

/// The arch tag used in probe JSON and release asset names.
pub fn arch_name() -> String {
    if cfg!(target_arch = "x86_64") {
        "x86_64".to_owned()
    } else if cfg!(target_arch = "aarch64") {
        "aarch64".to_owned()
    } else {
        std::env::consts::ARCH.to_owned()
    }
}

/// Compute the SHA-256 of a file and return it as a lower-case hex string.
pub fn sha256_file(path: &std::path::Path) -> io::Result<String> {
    let bytes = std::fs::read(path)?;
    let hash = Sha256::digest(&bytes);
    Ok(hash.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip() {
        let probe = Probe {
            version: "0.1.0".into(),
            proto: PROTO,
            os: "linux".into(),
            arch: "x86_64".into(),
            build: "deadbeef".repeat(8),
            mtime: Some(1_700_000_000),
        };
        let json = serde_json::to_string(&probe).unwrap();
        assert_eq!(Probe::parse(&json), Some(probe));
    }

    #[test]
    fn parse_accepts_a_probe_without_mtime() {
        // What a binary from before `mtime` prints.
        let line = r#"{"version":"0.2.0","proto":1,"os":"linux","arch":"x86_64","build":"ab"}"#;
        assert_eq!(Probe::parse(line).unwrap().mtime, None);
    }

    #[test]
    fn own_describes_this_binary() {
        let own = Probe::own().expect("the test binary is readable");
        assert_eq!(own.build.len(), 64);
        assert!(own.mtime.is_some());
        assert_eq!(own, &Probe::current().unwrap());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(Probe::parse("not json").is_none());
        assert!(Probe::parse("").is_none());
    }

    #[test]
    fn up_to_date_compares_build_hash() {
        let a = Probe {
            version: "1".into(),
            proto: 1,
            os: "linux".into(),
            arch: "x86_64".into(),
            build: "aabb".into(),
            mtime: None,
        };
        let b = Probe {
            build: "ccdd".into(),
            ..a.clone()
        };
        let same = Probe {
            build: "aabb".into(),
            ..a.clone()
        };
        assert!(!a.is_up_to_date(&b));
        assert!(a.is_up_to_date(&same));
    }

    #[test]
    fn same_platform_compares_os_arch() {
        let a = Probe {
            version: "1".into(),
            proto: 1,
            os: "linux".into(),
            arch: "x86_64".into(),
            build: "x".into(),
            mtime: None,
        };
        let mac = Probe {
            os: "macos".into(),
            ..a.clone()
        };
        let arm = Probe {
            arch: "aarch64".into(),
            ..a.clone()
        };
        assert!(a.same_platform(&a.clone()));
        assert!(!a.same_platform(&mac));
        assert!(!a.same_platform(&arm));
    }
}
