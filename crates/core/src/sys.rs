//! Small facts about this machine.

/// This machine's host name (`gethostname`), or `""` when it cannot be read.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the pointer and length describe a valid, writable buffer.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_is_what_the_system_says() {
        let name = hostname();
        assert!(!name.contains('\0'));
        let Ok(out) = std::process::Command::new("hostname").output() else {
            return; // no `hostname` command to compare with
        };
        if !out.status.success() {
            return;
        }
        assert_eq!(name, String::from_utf8_lossy(&out.stdout).trim());
    }
}
