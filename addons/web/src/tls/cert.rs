//! The `--remote` certificate: self-signed, ECDSA P-256, made once and kept
//! in `<config>/web-tls/{key,cert}.pem` (0600, in a 0700 directory) so its
//! fingerprint stays what the member checked. `--rotate-cert` replaces it.

use std::net::IpAddr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};

use claudio_core::paths;

/// A PEM file larger than this is not one we wrote.
const PEM_MAX: u64 = 16 * 1024;
const VALIDITY: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);
const BACKDATE: Duration = Duration::from_secs(24 * 60 * 60);

/// `<config>/web-tls`.
pub(crate) fn dir() -> PathBuf {
    paths::config_dir().join("web-tls")
}

/// The certificate in use and its TLS server config.
pub(crate) struct Cert {
    pub der: CertificateDer<'static>,
    pub config: Arc<ServerConfig>,
}

impl Cert {
    /// SHA-256 of the DER certificate, uppercase hex, colon-separated (what
    /// browsers show).
    pub(crate) fn fingerprint(&self) -> String {
        fingerprint(&self.der)
    }

    /// Whether the certificate covers `name` (a host name or an IP literal).
    pub(crate) fn covers(&self, name: &str) -> bool {
        let Ok(cert) = webpki::EndEntityCert::try_from(&self.der) else {
            return false;
        };
        let server_name = match name.parse::<IpAddr>() {
            Ok(ip) => ServerName::IpAddress(ip.into()),
            Err(_) => match ServerName::try_from(name) {
                Ok(n) => n,
                Err(_) => return false,
            },
        };
        cert.verify_is_valid_for_subject_name(&server_name).is_ok()
    }
}

pub(crate) fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The kept certificate, or a new one for `names` when there is none or
/// `rotate`. An unreadable, foreign, non-private or mismatched pair is an
/// error naming `--rotate-cert`: never replaced silently. Blocking I/O.
pub(crate) fn load_or_create(dir: &Path, names: &[String], rotate: bool) -> Result<Cert, String> {
    let (key_path, cert_path) = (dir.join("key.pem"), dir.join("cert.pem"));
    paths::ensure_private_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let exists = |p: &Path| std::fs::symlink_metadata(p).is_ok();
    if rotate || (!exists(&key_path) && !exists(&cert_path)) {
        let (key_pem, cert_pem) = generate(names)?;
        // The key first: a crash in between leaves a mismatched pair, which
        // the next start refuses (a key mismatch) rather than reuses.
        paths::write_atomic(&key_path, key_pem.as_bytes())
            .and_then(|()| paths::write_atomic(&cert_path, cert_pem.as_bytes()))
            .map_err(|e| format!("cannot write {}: {e}", dir.display()))?;
    }
    let fix = |what: String| {
        format!("{what}; `claudio web --remote --rotate-cert` makes a new certificate")
    };
    let key_pem = read_private(&key_path).map_err(fix)?;
    let cert_pem = read_private(&cert_path).map_err(fix)?;
    let der = CertificateDer::from_pem_slice(cert_pem.as_bytes())
        .map_err(|e| fix(format!("{}: {e}", cert_path.display())))?;
    // The parse error is not shown: it could quote a line of the key (C1).
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .map_err(|_| fix(format!("{}: not a PEM private key", key_path.display())))?;
    let config =
        server_config(der.clone(), key).map_err(|e| fix(format!("{}: {e}", dir.display())))?;
    Ok(Cert { der, config })
}

/// `--print-cert`: the kept `cert.pem`, checked like every read of it.
pub(crate) fn read_cert_pem(dir: &Path) -> Result<String, String> {
    let path = dir.join("cert.pem");
    if std::fs::symlink_metadata(&path).is_err() {
        return Err("no certificate yet: start `claudio web --remote` once to make it".to_owned());
    }
    let pem = read_private(&path)?;
    CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(pem)
}

/// A private regular file of ours, at most [`PEM_MAX`] bytes, as text.
fn read_private(path: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let private =
        meta.file_type().is_file() && meta.mode() & 0o077 == 0 && meta.uid() == paths::uid();
    if !private || meta.len() > PEM_MAX {
        return Err(format!(
            "{} is not a private (0600) regular file of yours",
            path.display()
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A new key and self-signed certificate (PEM): CN `claudio web`, SANs
/// `names`, valid from a day ago for ten years.
fn generate(names: &[String]) -> Result<(String, String), String> {
    let err = |e: rcgen::Error| format!("cannot make a certificate: {e}");
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(err)?;
    let mut params = rcgen::CertificateParams::new(names.to_vec()).map_err(err)?;
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, "claudio web");
    params.distinguished_name = dn;
    let now = SystemTime::now();
    params.not_before = (now - BACKDATE).into();
    params.not_after = (now + VALIDITY).into();
    let cert = params.self_signed(&key).map_err(err)?;
    Ok((key.serialize_pem(), cert.pem()))
}

/// TLS 1.2 and 1.3 with ring (named, never the process default), no
/// client auth, ALPN `http/1.1`. Fails when the key does not match.
fn server_config(
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<ServerConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const NAMES: [&str; 5] = ["localhost", "127.0.0.1", "::1", "box", "box.local"];

    fn names() -> Vec<String> {
        NAMES.iter().map(|s| s.to_string()).collect()
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn private(p: &Path, bytes: &[u8]) {
        std::fs::write(p, bytes).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn a_certificate_is_made_once_and_kept() {
        let dir = paths::socket_test_dir("web-cert");
        let first = load_or_create(&dir, &names(), false).unwrap();
        let f = first.fingerprint();
        assert_eq!(f.len(), 95, "{f}");
        assert!(f.split(':').all(|p| p.len() == 2
            && p.bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())));
        let again = load_or_create(&dir, &names(), false).unwrap();
        assert_eq!(again.fingerprint(), f);
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("key.pem")), 0o600);
        assert_eq!(mode(&dir.join("cert.pem")), 0o600);
        for name in NAMES {
            assert!(again.covers(name), "{name}");
        }
        assert!(!again.covers("other.example") && !again.covers("10.0.0.1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_replaces_it() {
        let dir = paths::socket_test_dir("web-cert");
        let f = load_or_create(&dir, &names(), false).unwrap().fingerprint();
        let rotated = load_or_create(&dir, &names(), true).unwrap().fingerprint();
        assert_ne!(rotated, f);
        let kept = load_or_create(&dir, &names(), false).unwrap().fingerprint();
        assert_eq!(kept, rotated);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_pair_is_refused_never_replaced() {
        let dir = paths::socket_test_dir("web-cert");
        let other = paths::socket_test_dir("web-cert-other");
        load_or_create(&dir, &names(), false).unwrap();
        load_or_create(&other, &names(), false).unwrap();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        let cert_before = std::fs::read(&cert_path).unwrap();
        let refused = |why: &str| {
            let err = load_or_create(&dir, &names(), false).err().expect(why);
            assert!(err.contains("--rotate-cert"), "{why}: {err}");
            assert_eq!(std::fs::read(&cert_path).unwrap(), cert_before, "{why}");
        };
        // (a) Another pair's key.
        let key_before = std::fs::read(&key_path).unwrap();
        private(&key_path, &std::fs::read(other.join("key.pem")).unwrap());
        refused("a mismatched key");
        private(&key_path, &key_before);
        assert!(load_or_create(&dir, &names(), false).is_ok());
        // (b) A certificate others can read.
        std::fs::set_permissions(&cert_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        refused("a 0644 certificate");
        std::fs::set_permissions(&cert_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        // (c) Half a pair: never regenerated.
        std::fs::remove_file(&key_path).unwrap();
        refused("no key");
        assert!(!key_path.exists(), "no key was made");
        private(&key_path, &key_before);
        // (d) A symlink to a good private file.
        let target = other.join("cert.pem");
        std::fs::remove_file(&cert_path).unwrap();
        std::os::unix::fs::symlink(&target, &cert_path).unwrap();
        let err = load_or_create(&dir, &names(), false)
            .err()
            .expect("a symlink");
        assert!(err.contains("--rotate-cert"), "{err}");
        std::fs::remove_file(&cert_path).unwrap();
        // (e) Too large.
        let mut big = cert_before.clone();
        big.resize(17 * 1024, b'\n');
        private(&cert_path, &big);
        let err = load_or_create(&dir, &names(), false).err().expect("17 KiB");
        assert!(err.contains("--rotate-cert"), "{err}");
        assert_eq!(std::fs::read(&cert_path).unwrap(), big, "not replaced");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn print_cert_reads_only_the_certificate() {
        let dir = paths::socket_test_dir("web-cert");
        std::fs::create_dir_all(&dir).unwrap();
        let err = read_cert_pem(&dir).unwrap_err();
        assert!(err.contains("claudio web --remote"), "{err}");
        load_or_create(&dir, &names(), false).unwrap();
        let pem = read_cert_pem(&dir).unwrap();
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"), "{pem}");
        assert!(!pem.contains("PRIVATE KEY"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
