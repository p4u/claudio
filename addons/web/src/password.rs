//! The `--remote` password: one argon2id PHC string in `<config>/web-password`
//! (0600). `claudio web --set-password` writes it; a test writes the
//! committed fixture directly. Parameters are fixed (OWASP's baseline), and a
//! file with any other parameters is refused, so a verification's memory is
//! always [`M_COST_KIB`] KiB.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};

use claudio_core::paths;

/// argon2id memory cost in KiB (19 MiB), time cost and lanes.
pub(crate) const M_COST_KIB: u32 = 19 * 1024;
pub(crate) const T_COST: u32 = 2;
pub(crate) const P_COST: u32 = 1;
/// A password is 8 to 1024 bytes of UTF-8 (a stored one is checked
/// against the maximum only).
pub(crate) const MIN_BYTES: usize = 8;
pub(crate) const MAX_BYTES: usize = 1024;
/// A PHC string with these parameters is about 100 bytes; anything past
/// this is not one.
const FILE_MAX: u64 = 512;

/// `<config>/web-password`.
pub(crate) fn path() -> PathBuf {
    paths::config_dir().join("web-password")
}

fn argon2() -> Argon2<'static> {
    // The parameters are constants within argon2's limits.
    let params = Params::new(M_COST_KIB, T_COST, P_COST, None).expect("valid argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Whether `pw` may be a password: its length in bytes.
pub(crate) fn acceptable(pw: &str) -> Result<(), String> {
    match pw.len() {
        n if n < MIN_BYTES => Err(format!("the password needs at least {MIN_BYTES} bytes")),
        n if n > MAX_BYTES => Err(format!("the password takes at most {MAX_BYTES} bytes")),
        _ => Ok(()),
    }
}

/// A new PHC string for `pw` (random 16-byte salt). CPU-heavy: call off
/// the executor.
pub(crate) fn hash(pw: &str) -> Result<String, String> {
    acceptable(pw)?;
    argon2()
        .hash_password(pw.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| format!("cannot hash the password: {e}"))
}

/// The stored hash, checked: argon2id, v19, exactly this module's
/// parameters, a salt and a 32-byte output.
#[derive(Clone)]
pub(crate) struct Stored(PasswordHash);

impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stored(argon2id)")
    }
}

impl Stored {
    pub(crate) fn parse(text: &str) -> Result<Stored, String> {
        let bad = |why: &str| format!("not a claudio password hash ({why})");
        let h = PasswordHash::new(text.trim_end()).map_err(|_| bad("PHC syntax"))?;
        if h.algorithm.as_str() != "argon2id" {
            return Err(bad("not argon2id"));
        }
        let params = Params::try_from(&h).map_err(|_| bad("parameters"))?;
        let version_ok = h.version.is_none_or(|v| v == 0x13);
        let fixed =
            params.m_cost() == M_COST_KIB && params.t_cost() == T_COST && params.p_cost() == P_COST;
        let salt_ok = h.salt.as_ref().is_some_and(|s| s.len() >= 16);
        let out_ok = h.hash.as_ref().is_some_and(|o| o.len() == 32);
        if !(version_ok && fixed && salt_ok && out_ok) {
            return Err(bad(
                "other parameters than m=19456,t=2,p=1, a 16-byte salt and a 32-byte hash",
            ));
        }
        Ok(Stored(h))
    }

    /// Whether `pw` is the password. CPU-heavy (19 MiB, ~15 ms optimised):
    /// call in `spawn_blocking`, under the login semaphore.
    pub(crate) fn verify(&self, pw: &str) -> bool {
        pw.len() <= MAX_BYTES && argon2().verify_password(pw.as_bytes(), &self.0).is_ok()
    }
}

/// Read the stored hash: a regular file of ours, not readable by others,
/// at most 512 bytes, one valid PHC string. Every failure says how to fix it.
pub(crate) fn load(path: &Path) -> Result<Stored, String> {
    parse_file(path, &read(path)?)
}

/// The password file's bytes: a regular file of ours, not readable by
/// others, at most 512 bytes. Every failure says how to fix it. One read
/// feeds both the password a `--remote` server checks and its sessions'
/// epoch (`sessions::epoch`). Blocking I/O.
pub(crate) fn read(path: &Path) -> Result<Vec<u8>, String> {
    let fix = "set one with `claudio web --set-password`";
    let meta = std::fs::symlink_metadata(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => format!("--remote needs a password: {fix}"),
        _ => format!("cannot read {}: {e}; {fix}", path.display()),
    })?;
    let private =
        meta.file_type().is_file() && meta.mode() & 0o077 == 0 && meta.uid() == paths::uid();
    if !private || meta.len() > FILE_MAX {
        return Err(format!(
            "{} must be a private (0600) regular file of yours holding one hash; {fix}",
            path.display()
        ));
    }
    std::fs::read(path).map_err(|e| format!("cannot read {}: {e}; {fix}", path.display()))
}

/// The stored hash in `bytes`, as [`read`] read them from `path`.
pub(crate) fn parse_file(path: &Path, bytes: &[u8]) -> Result<Stored, String> {
    let fix = "set one with `claudio web --set-password`";
    let text =
        std::str::from_utf8(bytes).map_err(|_| format!("{}: not text; {fix}", path.display()))?;
    Stored::parse(text).map_err(|e| format!("{}: {e}; {fix}", path.display()))
}

/// `claudio web --set-password`: the password twice from the terminal
/// (never a pipe), hashed, written, read back and verified.
pub(crate) fn set_password() -> Result<String, String> {
    use claudio_core::term::secret::read_line_no_echo;
    let read = |prompt: &str| {
        read_line_no_echo(prompt).map_err(|e| {
            format!("--set-password reads the password from a terminal ({e}); run it in one")
        })
    };
    let pw = read("New password: ")?;
    acceptable(&pw)?;
    if read("Again: ")? != pw {
        return Err("the two passwords differ; nothing was saved".to_owned());
    }
    let phc = hash(&pw)?;
    let path = path();
    save(&path, &phc).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    if !load(&path)?.verify(&pw) {
        return Err(format!(
            "{} does not verify; run --set-password again",
            path.display()
        ));
    }
    Ok(format!(
        "password saved to {}. Restart claudio web to use it (that also signs every browser out).",
        path.display()
    ))
}

/// Write `phc` (one line) atomically, 0600.
pub(crate) fn save(path: &Path, phc: &str) -> io::Result<()> {
    paths::write_atomic(path, format!("{phc}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const FIXTURE: &str = include_str!("../web/e2e/fixtures/web-password");

    /// A scratch dir, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Dir {
            let d = paths::socket_test_dir("web-password");
            std::fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The fixture's salt and hash fields.
    fn fixture_parts() -> (&'static str, &'static str) {
        let mut parts = FIXTURE.trim_end().rsplit('$');
        let hash = parts.next().unwrap();
        let salt = parts.next().unwrap();
        (salt, hash)
    }

    #[test]
    fn the_fixture_verifies_against_the_test_password() {
        let s = Stored::parse(FIXTURE).unwrap();
        assert!(s.verify("claudio-e2e-password"));
        assert!(!s.verify("claudio-e2e-passwore"));
        assert!(!s.verify(""));
        assert_eq!(format!("{s:?}"), "Stored(argon2id)", "no hash in Debug");
        let crlf = format!("{}\r\n", FIXTURE.trim_end());
        assert!(Stored::parse(&crlf).unwrap().verify("claudio-e2e-password"));
    }

    #[test]
    fn a_new_hash_is_argon2id_with_the_fixed_parameters() {
        let phc = hash("twelve bytes").unwrap();
        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
        assert!(Stored::parse(&phc).unwrap().verify("twelve bytes"));
        assert_ne!(hash("twelve bytes").unwrap(), phc, "a new salt each time");
    }

    #[test]
    fn password_length_is_8_to_1024_bytes() {
        let short = acceptable(&"x".repeat(7)).unwrap_err();
        assert_eq!(short, "the password needs at least 8 bytes");
        assert!(acceptable(&"x".repeat(8)).is_ok());
        assert!(acceptable(&"é".repeat(4)).is_ok(), "bytes, not characters");
        assert!(acceptable(&"x".repeat(1024)).is_ok());
        let long = acceptable(&"x".repeat(1025)).unwrap_err();
        assert_eq!(long, "the password takes at most 1024 bytes");
        assert!(hash(&"x".repeat(7)).is_err());
        let s = Stored::parse(FIXTURE).unwrap();
        assert!(
            !s.verify(&"x".repeat(1025)),
            "no hashing of an oversized body"
        );
    }

    #[test]
    fn other_parameters_are_refused() {
        let (salt, out) = fixture_parts();
        let head = "$argon2id$v=19$m=19456,t=2,p=1$";
        let bad = [
            FIXTURE.replace("m=19456", "m=65536"),
            FIXTURE.replace("t=2", "t=3"),
            FIXTURE.replace("p=1", "p=2"),
            FIXTURE.replace("$argon2id$", "$argon2i$"),
            FIXTURE.replace("v=19", "v=16"),
            FIXTURE.replace(salt, "c2FsdHNhbHQ"),
            format!("{head}{salt}"),
            format!("{head}{salt}${}", &out[..20]),
            "not a hash".to_owned(),
            String::new(),
        ];
        for text in &bad {
            assert!(Stored::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn the_file_must_be_private_and_small() {
        let dir = Dir::new();
        let p = dir.0.join("web-password");
        save(&p, &hash("a good password").unwrap()).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&p), 0o600);
        assert!(load(&p).unwrap().verify("a good password"));

        let link = dir.0.join("link");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(load(&link).is_err(), "a symlink");

        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = load(&p).unwrap_err();
        assert!(err.contains("--set-password"), "{err}");

        let big = dir.0.join("big");
        paths::write_atomic(&big, &[b'a'; 600]).unwrap();
        assert!(load(&big).is_err(), "600 bytes");

        let err = load(&dir.0.join("missing")).unwrap_err();
        assert!(err.contains("--remote needs a password"), "{err}");
    }
}
