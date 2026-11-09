//! `--remote`: its address grammar, the names its certificate and `Host`
//! check use, and what it prints at start.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::DEFAULT_PORT;

/// `--remote`'s value: `ADDR`, `ADDR:PORT`, `:PORT`, `[V6]` or `[V6]:PORT`
/// (an IPv6 address needs brackets). The port defaults to 7337, the
/// address to `0.0.0.0`.
pub(crate) fn remote_addr(v: &str) -> Result<SocketAddr, String> {
    let bad = || {
        format!(
            "--remote {v:?}: not ADDR, ADDR:PORT, :PORT or [V6]:PORT \
             (an IPv6 address needs brackets: --remote [::]:7337)"
        )
    };
    let port = |p: &str| {
        p.parse::<u16>()
            .ok()
            .filter(|n| *n > 0 && p.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(bad)
    };
    if let Some(p) = v.strip_prefix(':') {
        return Ok(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port(p)?));
    }
    if let Some(rest) = v.strip_prefix('[') {
        let (ip, tail) = rest.split_once(']').ok_or_else(bad)?;
        let ip: Ipv6Addr = ip.parse().map_err(|_| bad())?;
        let port = match tail {
            "" => DEFAULT_PORT,
            t => port(t.strip_prefix(':').ok_or_else(bad)?)?,
        };
        return Ok(SocketAddr::new(ip.into(), port));
    }
    let (ip, p) = match v.split_once(':') {
        Some((ip, p)) => (ip, port(p)?),
        None => (v, DEFAULT_PORT),
    };
    let ip: Ipv4Addr = ip.parse().map_err(|_| bad())?;
    Ok(SocketAddr::new(ip.into(), p))
}

/// The host name as a DNS name (lowercase `[a-z0-9.-]`), if it is one. An
/// IP literal is not a name (some containers report one as the host name).
pub(crate) fn dns_name(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let ok = host.parse::<IpAddr>().is_err()
        && (1..=253).contains(&host.len())
        && !host.starts_with(['-', '.'])
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    ok.then_some(host)
}

/// This machine's names: the host name and its mDNS `<first label>.local`.
pub(crate) fn own_names(hostname: &str) -> Vec<String> {
    let Some(host) = dns_name(hostname) else {
        return Vec::new();
    };
    let short = host.split('.').next().unwrap_or_default().to_owned();
    let mut names = vec![host];
    let local = format!("{short}.local");
    if !names.contains(&local) {
        names.push(local);
    }
    names
}

/// The certificate's SANs: loopback names, this machine's names, the
/// listen address when specific, and each `--allow-host` name (no port).
pub(crate) fn cert_names(
    hostname: &str,
    listen: SocketAddr,
    allow_hosts: &[String],
) -> Vec<String> {
    let mut names: Vec<String> = ["localhost", "127.0.0.1", "::1"].map(String::from).to_vec();
    names.extend(own_names(hostname));
    if !listen.ip().is_unspecified() {
        names.push(listen.ip().to_string());
    }
    for name in allow_hosts {
        names.push(name.split(':').next().unwrap_or_default().to_owned());
    }
    let mut seen = Vec::new();
    names.retain(|n| {
        let new = !seen.contains(n);
        seen.push(n.clone());
        new
    });
    names
}

/// A `Host` form of `ip`: IPv6 in brackets.
fn url_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

/// The URL a local browser uses with the token, when a loopback request
/// can reach the listener: the address itself when loopback, `127.0.0.1`
/// for `0.0.0.0`, `[::1]` for `[::]`.
pub(crate) fn token_url(listen: SocketAddr, token: &str) -> Option<String> {
    let host = match listen.ip() {
        ip if ip.is_loopback() => url_host(ip),
        IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_owned(),
        IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_owned(),
        _ => return None,
    };
    Some(format!("https://{host}:{}/#t={token}", listen.port()))
}

/// The one stdout URL: the specific address, else the host name.
pub(crate) fn public_url(listen: SocketAddr, hostname: &str) -> String {
    let host = if listen.ip().is_unspecified() {
        dns_name(hostname).unwrap_or_else(|| "localhost".to_owned())
    } else {
        url_host(listen.ip())
    };
    format!("https://{host}:{}/", listen.port())
}

/// What `--remote` prints at start, every line prefixed `claudio web: `.
pub(crate) struct Banner {
    pub stdout: String,
    pub stderr: Vec<String>,
}

/// The facts the banner shows.
pub(crate) struct BannerFacts<'a> {
    pub listen: SocketAddr,
    pub hostname: &'a str,
    pub fingerprint: &'a str,
    /// The SAN candidates the kept certificate covers, and those it lacks.
    pub covered: &'a [String],
    pub missing: &'a [String],
    pub token_url: Option<&'a str>,
}

pub(crate) fn banner(f: &BannerFacts<'_>) -> Banner {
    let mut err = vec![
        format!("TLS certificate SHA-256 {}", f.fingerprint),
        format!("the certificate names {}", f.covered.join(", ")),
    ];
    if !f.missing.is_empty() {
        err.push(format!(
            "the certificate does not name {}: a browser also warns about that name; \
             --rotate-cert adds it (the fingerprint changes)",
            f.missing.join(", ")
        ));
    }
    if f.listen.ip().is_unspecified() {
        let local = own_names(f.hostname)
            .pop()
            .unwrap_or_else(|| "this machine's address".to_owned());
        err.push(format!(
            "listening on every interface, port {}; a host firewall (ufw, firewalld, macOS's) \
             may still block it. Other machines use {local} or this machine's address",
            f.listen.port()
        ));
    }
    if let Some(url) = f.token_url {
        err.push(format!("on this machine: {url}"));
    }
    err.push("other machines log in with the password (claudio web --set-password)".to_owned());
    Banner {
        stdout: format!("claudio web: {}", public_url(f.listen, f.hostname)),
        stderr: err
            .into_iter()
            .map(|l| format!("claudio web: {l}"))
            .collect(),
    }
}

/// What `--remote` serves with, made before the runtime starts.
pub(crate) struct Remote {
    pub addr: SocketAddr,
    pub cert: crate::tls::cert::Cert,
    /// The stored password hash, loaded before anything else.
    pub password: crate::password::Stored,
    pub allowed: crate::auth::Allowed,
    pub banner: Banner,
}

/// Check the password file first (nothing is made or bound without one),
/// then load or make the certificate. Blocking I/O.
pub(crate) fn prepare(
    args: &crate::WebArgs,
    addr: SocketAddr,
    token: &str,
) -> Result<Remote, String> {
    let password = crate::password::load(&crate::password::path())?;
    let hostname = claudio_core::sys::hostname();
    let names = cert_names(&hostname, addr, &args.allow_hosts);
    let cert =
        crate::tls::cert::load_or_create(&crate::tls::cert::dir(), &names, args.rotate_cert)?;
    let (covered, missing): (Vec<String>, Vec<String>) =
        names.into_iter().partition(|n| cert.covers(n));
    let token_url = token_url(addr, token);
    let banner = banner(&BannerFacts {
        listen: addr,
        hostname: &hostname,
        fingerprint: &cert.fingerprint(),
        covered: &covered,
        missing: &missing,
        token_url: token_url.as_deref(),
    });
    let mut allowed_names = own_names(&hostname);
    allowed_names.extend(args.allow_hosts.iter().cloned());
    Ok(Remote {
        addr,
        cert,
        password,
        allowed: crate::auth::Allowed::remote(addr, &allowed_names),
        banner,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_addresses_follow_the_grammar() {
        let ok = |v: &str| remote_addr(v).unwrap().to_string();
        assert_eq!(ok("0.0.0.0"), "0.0.0.0:7337");
        assert_eq!(ok(":8443"), "0.0.0.0:8443");
        assert_eq!(ok("192.168.1.20:9000"), "192.168.1.20:9000");
        assert_eq!(ok("[::]"), "[::]:7337");
        assert_eq!(ok("[::]:8443"), "[::]:8443");
        assert_eq!(ok("[fe80::1]:7337"), "[fe80::1]:7337");
        for bad in [
            "::1",
            "fe80::1:7337",
            "[::1",
            "[::1]8443",
            ":0",
            ":x",
            "host:7337",
            "1.2.3.4:",
            "1.2.3.4:+1",
            "",
            "localhost",
        ] {
            assert!(remote_addr(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn names_cover_loopback_the_host_and_allowed_names() {
        let any: SocketAddr = "0.0.0.0:7337".parse().unwrap();
        let n = cert_names("Box.lan", any, &["claudio.test:7337".to_owned()]);
        assert_eq!(
            n,
            [
                "localhost",
                "127.0.0.1",
                "::1",
                "box.lan",
                "box.local",
                "claudio.test"
            ]
        );
        let n = cert_names("bad_name", "192.168.1.20:7337".parse().unwrap(), &[]);
        assert_eq!(n, ["localhost", "127.0.0.1", "::1", "192.168.1.20"]);
        assert_eq!(
            cert_names("10.0.0.7", any, &[]),
            ["localhost", "127.0.0.1", "::1"]
        );
        assert_eq!(own_names("10.0.0.7"), Vec::<String>::new());
        assert_eq!(public_url(any, "10.0.0.7"), "https://localhost:7337/");
    }

    #[test]
    fn urls_and_the_banner() {
        let any: SocketAddr = "0.0.0.0:7337".parse().unwrap();
        assert_eq!(
            token_url(any, "t").as_deref(),
            Some("https://127.0.0.1:7337/#t=t")
        );
        assert_eq!(
            token_url("[::]:1".parse().unwrap(), "t").as_deref(),
            Some("https://[::1]:1/#t=t")
        );
        assert_eq!(
            token_url("127.0.0.1:5".parse().unwrap(), "t").as_deref(),
            Some("https://127.0.0.1:5/#t=t")
        );
        assert_eq!(token_url("192.168.1.20:5".parse().unwrap(), "t"), None);
        assert_eq!(public_url(any, "box"), "https://box:7337/");
        assert_eq!(
            public_url("127.0.0.1:5".parse().unwrap(), "box"),
            "https://127.0.0.1:5/"
        );
        let b = banner(&BannerFacts {
            listen: any,
            hostname: "box",
            fingerprint: "AB:CD",
            covered: &["localhost".into()],
            missing: &["box.local".into()],
            token_url: Some("https://127.0.0.1:7337/#t=t"),
        });
        assert_eq!(b.stdout, "claudio web: https://box:7337/");
        assert!(b.stderr.iter().all(|l| l.starts_with("claudio web: ")));
        assert!(b.stderr[0].ends_with("SHA-256 AB:CD"));
        assert!(b.stderr.iter().any(|l| l.contains("--rotate-cert")));
        assert!(b
            .stderr
            .iter()
            .any(|l| l.contains("firewall") && l.contains("box.local")));
    }
}
