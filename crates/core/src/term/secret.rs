//! A secret typed at the terminal: one line read with echo off. The
//! terminal's mode is restored on every exit path, SIGINT, SIGTERM and
//! SIGHUP included (a temporary handler restores it, then re-raises).

use std::io::{self, BufRead, IsTerminal, Write};
use std::mem::MaybeUninit;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicI32, Ordering};

/// The longest line read; the rest of a longer one is left unread.
pub const MAX_LINE: u64 = 4096;

/// The signals that restore the terminal before they end the process.
const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// Print `prompt` on stderr and read one line from stdin, which must be a
/// terminal, with echo off. The line comes back without its line ending
/// (nothing else is trimmed). An error when stdin is not a terminal or its
/// echo cannot be turned off: a secret is never read with echo on.
pub fn read_line_no_echo(prompt: &str) -> io::Result<String> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return Err(io::Error::other("stdin is not a terminal"));
    }
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let line = no_echo_line(stdin.as_raw_fd(), stdin.lock());
    eprintln!();
    line
}

/// File descriptor saved for the signal handler; -1 when none is installed.
static SIGNAL_FD: AtomicI32 = AtomicI32::new(-1);

/// The terminal mode the signal handler restores.
struct TermCell(std::cell::UnsafeCell<MaybeUninit<libc::termios>>);
// SAFETY: written before SIGNAL_FD is set (Release), read only by the
// handler after it saw that store (Acquire); no write while installed.
unsafe impl Sync for TermCell {}
static SIGNAL_SAVED: TermCell = TermCell(std::cell::UnsafeCell::new(MaybeUninit::uninit()));

extern "C" fn on_signal(sig: libc::c_int) {
    let fd = SIGNAL_FD.load(Ordering::Acquire);
    if fd >= 0 {
        // SAFETY: initialised before SIGNAL_FD became non-negative.
        unsafe {
            let saved = (*SIGNAL_SAVED.0.get()).assume_init_ref();
            libc::tcsetattr(fd, libc::TCSANOW, saved);
        }
    }
    // SAFETY: async-signal-safe calls only.
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

/// Restores the terminal mode and the previous signal handlers on drop.
struct Restore {
    fd: RawFd,
    saved: libc::termios,
    old: [libc::sighandler_t; SIGNALS.len()],
}

impl Drop for Restore {
    fn drop(&mut self) {
        // SAFETY: `saved` came from tcgetattr on `fd`; `old` from signal().
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
            for (sig, old) in SIGNALS.iter().zip(self.old) {
                libc::signal(*sig, old);
            }
        }
        SIGNAL_FD.store(-1, Ordering::Release);
    }
}

/// One line of `input` (at most [`MAX_LINE`] bytes) with echo off on the
/// terminal `fd`.
fn no_echo_line(fd: RawFd, input: impl BufRead) -> io::Result<String> {
    let mut saved = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr writes the struct on success.
    if unsafe { libc::tcgetattr(fd, saved.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr succeeded.
    let saved = unsafe { saved.assume_init() };
    // SAFETY: no handler reads it yet (SIGNAL_FD is -1).
    unsafe { (*SIGNAL_SAVED.0.get()).write(saved) };
    SIGNAL_FD.store(fd, Ordering::Release);
    // SAFETY: on_signal is a valid extern "C" handler.
    let old = SIGNALS
        .map(|sig| unsafe { libc::signal(sig, on_signal as *const () as libc::sighandler_t) });
    let restore = Restore { fd, saved, old };
    let mut quiet = saved;
    quiet.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
    // SAFETY: a valid termios for `fd`.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut line = String::new();
    input.take(MAX_LINE).read_line(&mut line)?;
    drop(restore);
    let end = line.trim_end_matches(['\n', '\r']).len();
    line.truncate(end);
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::fs::File;
    use std::os::unix::io::FromRawFd;
    use std::sync::Mutex;

    /// The tests share SIGNAL_FD and the process's signal handlers.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// A pseudo-terminal: (master, slave).
    fn pty() -> (File, File) {
        // SAFETY: plain libc calls; every fd is checked and owned by a File.
        unsafe {
            let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(m >= 0);
            assert_eq!(libc::grantpt(m), 0);
            assert_eq!(libc::unlockpt(m), 0);
            let name = CStr::from_ptr(libc::ptsname(m)).to_owned();
            let s = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            assert!(s >= 0);
            (File::from_raw_fd(m), File::from_raw_fd(s))
        }
    }

    #[test]
    fn a_line_is_read_without_echo_and_the_mode_comes_back() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut master, slave) = pty();
        let fd = slave.as_raw_fd();
        let echo_on = |fd| {
            let mut t = MaybeUninit::<libc::termios>::uninit();
            assert_eq!(unsafe { libc::tcgetattr(fd, t.as_mut_ptr()) }, 0);
            unsafe { t.assume_init() }.c_lflag & libc::ECHO != 0
        };
        assert!(echo_on(fd));
        master.write_all(b"  s3cret pass \n").unwrap();
        let line = no_echo_line(fd, io::BufReader::new(&slave)).unwrap();
        assert_eq!(line, "  s3cret pass ", "only the line ending goes");
        assert!(echo_on(fd), "echo is back");
        assert_eq!(SIGNAL_FD.load(Ordering::Acquire), -1);
        for sig in SIGNALS {
            let mut now = MaybeUninit::<libc::sigaction>::uninit();
            // SAFETY: a null new action only reads the current one.
            assert_eq!(
                unsafe { libc::sigaction(sig, std::ptr::null(), now.as_mut_ptr()) },
                0
            );
            let handler = unsafe { now.assume_init() }.sa_sigaction;
            assert_ne!(
                handler, on_signal as *const () as libc::sighandler_t,
                "{sig} restored"
            );
        }
    }

    #[test]
    fn a_pipe_is_refused_before_anything_is_read() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut fds = [0i32; 2];
        // SAFETY: a plain pipe; both ends are owned by Files below.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (r, mut w) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        w.write_all(b"secret\n").unwrap();
        let err = no_echo_line(r.as_raw_fd(), io::BufReader::new(&r)).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ENOTTY), "{err}");
        assert_eq!(
            SIGNAL_FD.load(Ordering::Acquire),
            -1,
            "no handler was left installed"
        );
        if !io::stdin().is_terminal() {
            assert!(read_line_no_echo("x: ").is_err());
        }
    }
}
