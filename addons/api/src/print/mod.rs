//! The `-p`/`--print` emulation path: drive interactive `claude` under a PTY for
//! one turn and read the answer from the session JSONL. Also backs `--api`'s
//! persistent sessions (`driver::PtySession`).

pub mod driver;
pub mod emit;
pub mod hooks;
pub mod session;
