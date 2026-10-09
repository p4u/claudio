//! Daemon tests for the git viewer ops, against real repositories created
//! with `git init` in a temp dir. The fixture holds a rename, a binary file,
//! a file whose name is not UTF-8, a commit subject with an escape sequence,
//! a merge and an unmerged branch.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;
use crate::proto::{GitCommitInfo, GitLogPage, GitPatch};

/// Run git in `dir` with a fixed identity; panics on failure.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Tess", "-c", "user.email=tess@example.org"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

/// The fixture repository and the ids of its commits.
struct Repo {
    dir: PathBuf,
    first: String,
    renamed: String,
    merge: String,
}

impl Repo {
    fn create(parent: &Path) -> Repo {
        let dir = parent.join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();

        write("a.txt", "one\ntwo\nthree\nfour\nfive\nsix\n");
        let first = commit_all(&dir, "first commit");
        git(&dir, &["tag", "v1"]);

        git(&dir, &["mv", "a.txt", "b.txt"]);
        write("b.txt", "one\ntwo\nthree\nfour\nfive\nsix\nseven\n");
        std::fs::write(dir.join("bin.dat"), [0u8, 1, 2, 0, 255]).unwrap();
        let odd = OsString::from_vec(b"caf\xe9.txt".to_vec());
        std::fs::write(dir.join(odd), "latin-1 name\n").unwrap();
        let message = "evil \x1b[31msubject\n\nbody line one\nbody line two";
        let renamed = commit_all(&dir, message);

        git(&dir, &["checkout", "-q", "-b", "feat"]);
        write("c.txt", "feature\n");
        commit_all(&dir, "feat work");
        git(&dir, &["checkout", "-q", "main"]);
        write("d.txt", "main work\n");
        commit_all(&dir, "main work");
        git(
            &dir,
            &["merge", "-q", "--no-ff", "feat", "-m", "merge feat"],
        );
        let merge = git(&dir, &["rev-parse", "HEAD"]);

        git(&dir, &["checkout", "-q", "-b", "wip"]);
        write("e.txt", "unmerged\n");
        commit_all(&dir, "wip work");
        git(&dir, &["checkout", "-q", "main"]);

        Repo {
            dir,
            first,
            renamed,
            merge,
        }
    }

    fn cwd(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }

    fn log(&self, all: bool, skip: u32, limit: u32) -> Msg {
        Msg::GitLog {
            cwd: self.cwd(),
            all,
            skip,
            limit,
        }
    }

    fn commit(&self, id: &str) -> Msg {
        Msg::GitCommit {
            cwd: self.cwd(),
            id: id.into(),
        }
    }

    fn diff(&self, id: &str, path: Option<&str>, old_path: Option<&str>) -> Msg {
        Msg::GitDiff {
            cwd: self.cwd(),
            id: id.into(),
            path: path.map(Into::into),
            old_path: old_path.map(Into::into),
        }
    }
}

async fn log_page(c: &mut Client, request: Msg) -> GitLogPage {
    match c.call(request).await {
        Msg::GitLogPage(page) => page,
        other => panic!("expected a log page, got {other:?}"),
    }
}

async fn commit_info(c: &mut Client, request: Msg) -> GitCommitInfo {
    match c.call(request).await {
        Msg::GitCommitInfo(info) => info,
        other => panic!("expected commit info, got {other:?}"),
    }
}

async fn patch(c: &mut Client, request: Msg) -> GitPatch {
    match c.call(request).await {
        Msg::GitPatch(patch) => patch,
        other => panic!("expected a patch, got {other:?}"),
    }
}

async fn error_text(c: &mut Client, request: Msg) -> String {
    match c.call(request).await {
        Msg::Error { message } => message,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn log_pages_through_history_and_scopes_to_all_branches() {
    let d = TestDaemon::start().await;
    let repo = Repo::create(&d.dir);
    let mut c = d.client().await;

    let first = log_page(&mut c, repo.log(false, 0, 2)).await;
    assert_eq!(first.commits.len(), 2);
    assert!(first.more);
    assert_eq!(first.head.as_deref(), Some("main"));
    assert_eq!(
        std::fs::canonicalize(&first.root).unwrap(),
        std::fs::canonicalize(&repo.dir).unwrap()
    );
    let tip = &first.commits[0];
    assert_eq!(tip.id, repo.merge);
    assert_eq!(tip.parents.len(), 2);
    assert_eq!(tip.subject, "merge feat");
    assert_eq!(tip.author, "Tess");
    assert!(
        tip.refs.contains(&"HEAD -> refs/heads/main".to_owned()),
        "{:?}",
        tip.refs
    );

    // The rest of main's history: 5 commits in all, the escape stripped.
    let rest = log_page(&mut c, repo.log(false, 2, 10)).await;
    assert_eq!(rest.commits.len(), 3);
    assert!(!rest.more);
    let renamed = rest.commits.iter().find(|e| e.id == repo.renamed).unwrap();
    assert_eq!(renamed.subject, "evil [31msubject");
    let root = rest.commits.last().unwrap();
    assert_eq!(
        (root.id.as_str(), root.parents.len()),
        (repo.first.as_str(), 0)
    );
    assert!(
        root.refs.contains(&"tag: refs/tags/v1".to_owned()),
        "{:?}",
        root.refs
    );

    // `all` adds the unmerged branch.
    let all = log_page(&mut c, repo.log(true, 0, 50)).await;
    assert_eq!(all.commits.len(), 6);
    assert!(all
        .commits
        .iter()
        .any(|e| e.refs.contains(&"refs/heads/wip".to_owned())));

    // A limit above the cap is clamped, not rejected.
    let big = log_page(&mut c, repo.log(false, 0, 100_000)).await;
    assert_eq!(big.commits.len(), 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commit_lists_renames_binaries_and_odd_names() {
    let d = TestDaemon::start().await;
    let repo = Repo::create(&d.dir);
    let mut c = d.client().await;

    let info = commit_info(&mut c, repo.commit(&repo.renamed)).await;
    assert_eq!(info.id, repo.renamed);
    assert!(!info.first_parent);
    assert_eq!(info.author, "Tess");
    assert_eq!(info.email, "tess@example.org");
    assert_eq!(
        info.message,
        "evil [31msubject\n\nbody line one\nbody line two"
    );

    let file = |path: &str| info.files.iter().find(|f| f.path == path).unwrap();
    let renamed = file("b.txt");
    assert_eq!(renamed.old_path.as_deref(), Some("a.txt"));
    assert_eq!((renamed.added, renamed.removed), (Some(1), Some(0)));
    let binary = file("bin.dat");
    assert_eq!((binary.added, binary.removed), (None, None));
    assert!(
        info.files.iter().any(|f| f.path.starts_with("caf")),
        "{:?}",
        info.files
    );
    assert_eq!(info.files.len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_merge_is_shown_against_its_first_parent() {
    let d = TestDaemon::start().await;
    let repo = Repo::create(&d.dir);
    let mut c = d.client().await;

    let info = commit_info(&mut c, repo.commit(&repo.merge)).await;
    assert!(info.first_parent);
    assert_eq!(info.parents.len(), 2);
    let paths: Vec<_> = info.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["c.txt"], "what the merge brought into main");

    let whole = patch(&mut c, repo.diff(&repo.merge, None, None)).await;
    assert!(
        whole.patch.contains("diff --git a/c.txt b/c.txt"),
        "{}",
        whole.patch
    );
    assert!(!whole.patch.contains("d.txt"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diff_returns_one_file_or_the_whole_commit() {
    let d = TestDaemon::start().await;
    let repo = Repo::create(&d.dir);
    let mut c = d.client().await;

    let one = patch(
        &mut c,
        repo.diff(&repo.renamed, Some("b.txt"), Some("a.txt")),
    )
    .await;
    assert_eq!(one.id, repo.renamed);
    assert_eq!(one.path.as_deref(), Some("b.txt"));
    assert!(!one.truncated);
    assert!(one.patch.contains("rename from a.txt"), "{}", one.patch);
    assert!(one.patch.contains("+seven"));
    assert!(!one.patch.contains("bin.dat"));

    let whole = patch(&mut c, repo.diff(&repo.renamed, None, None)).await;
    assert!(whole.path.is_none());
    assert!(whole.patch.contains("bin.dat") && whole.patch.contains("Binary files"));
    assert!(!whole.patch.chars().any(|c| c.is_control() && c != '\n'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_huge_patch_is_truncated_to_fit_a_frame() {
    let d = TestDaemon::start().await;
    let repo = Repo::create(&d.dir);
    // 2 MiB of tab-indented, quote-heavy lines: far over every cap.
    let line = "\t\"quoted\\\" text\"\n";
    std::fs::write(
        repo.dir.join("huge.txt"),
        line.repeat(2 * 1024 * 1024 / line.len()),
    )
    .unwrap();
    let id = commit_all(&repo.dir, "huge");
    let mut c = d.client().await;

    let request = repo.diff(&id, Some("huge.txt"), None);
    let req = c.request(request).await;
    let frame = c
        .until(|f| match f {
            Frame::Control(env) if env.req == Some(req) => Some(f.clone()),
            _ => None,
        })
        .await;
    assert!(
        frame.encode().len() - 4 <= proto::MAX_FRAME,
        "fits one frame"
    );
    let Frame::Control(Envelope {
        msg: Msg::GitPatch(patch),
        ..
    }) = frame
    else {
        panic!("expected a patch")
    };
    assert!(patch.truncated);
    assert!(patch.patch.ends_with('\n'), "cut at a line boundary");
    assert!(patch.patch.len() > 100_000, "still a useful amount");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_that_could_be_injection_are_rejected() {
    let d = TestDaemon::start().await;
    let repo = Repo::create(&d.dir);
    let mut c = d.client().await;

    for bad in [
        "HEAD",
        "main",
        "--output=/tmp/x",
        &repo.merge[..12],
        &repo.merge.to_uppercase(),
    ] {
        let message = error_text(&mut c, repo.commit(bad)).await;
        assert!(message.contains("invalid commit id"), "{bad}: {message}");
        let message = error_text(&mut c, repo.diff(bad, None, None)).await;
        assert!(message.contains("invalid commit id"), "{bad}: {message}");
    }
    let message = error_text(&mut c, repo.diff(&repo.merge, Some("a\0b"), None)).await;
    assert!(message.contains("invalid path"), "{message}");
    // The pathspec is literal: a glob matches no file instead of all of them.
    let none = patch(&mut c, repo.diff(&repo.renamed, Some("*.txt"), None)).await;
    assert!(none.patch.is_empty(), "{}", none.patch);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_errors_come_back_as_bounded_messages() {
    let d = TestDaemon::start().await;
    let mut c = d.client().await;
    let not_a_repo = d.work().to_string_lossy().into_owned();
    let log = Msg::GitLog {
        cwd: not_a_repo,
        all: false,
        skip: 0,
        limit: 10,
    };
    let message = error_text(&mut c, log).await;
    assert!(message.contains("not a git repository"), "{message}");

    let missing = Msg::GitLog {
        cwd: "/nonexistent/claudio/dir".into(),
        all: false,
        skip: 0,
        limit: 10,
    };
    assert!(error_text(&mut c, missing)
        .await
        .contains("no such directory"));
}

/// A repo whose config names a textconv filter and an external diff driver
/// that both leave a marker file when run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_diff_drivers_never_execute() {
    let d = TestDaemon::start().await;
    let dir = d.dir.join("drivers");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    let (textconv, external) = (d.dir.join("textconv-ran"), d.dir.join("external-ran"));
    git(
        &dir,
        &[
            "config",
            "diff.x.textconv",
            &format!("touch {}; cat", textconv.display()),
        ],
    );
    git(
        &dir,
        &[
            "config",
            "diff.external",
            &format!("sh -c 'touch {}' --", external.display()),
        ],
    );
    std::fs::write(dir.join(".gitattributes"), "*.x diff=x\n").unwrap();
    std::fs::write(dir.join("t.x"), "one\n").unwrap();
    commit_all(&dir, "base");
    std::fs::write(dir.join("t.x"), "two\n").unwrap();
    let id = commit_all(&dir, "change");

    // Sanity: without our flags both drivers do run (`show` uses an external
    // diff only when asked), so the markers prove something.
    git(&dir, &["show", "-p", &id]);
    assert!(textconv.exists(), "fixture must trigger textconv");
    git(&dir, &["show", "--ext-diff", "-p", &id]);
    assert!(
        external.exists(),
        "fixture must trigger the external driver"
    );
    std::fs::remove_file(&textconv).unwrap();
    std::fs::remove_file(&external).unwrap();

    let mut c = d.client().await;
    let cwd = dir.to_string_lossy().into_owned();
    let request = Msg::GitDiff {
        cwd: cwd.clone(),
        id: id.clone(),
        path: None,
        old_path: None,
    };
    let diff = patch(&mut c, request).await;
    assert!(diff.patch.contains("+two"), "{}", diff.patch);
    commit_info(&mut c, Msg::GitCommit { cwd, id }).await;
    assert!(!textconv.exists(), "textconv ran");
    assert!(!external.exists(), "the external diff driver ran");
}

/// While a git process is stuck, the client loop keeps serving terminal I/O.
/// Git is held up by a config include that points at a FIFO.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_git_request_does_not_stall_terminal_data() {
    let d = TestDaemon::start().await;
    let repo = Repo::create(&d.dir);
    let fifo = d.dir.join("blocker");
    let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: `path` is a valid NUL-terminated string.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let config = repo.dir.join(".git/config");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(&format!("[include]\n\tpath = {}\n", fifo.display()));
    std::fs::write(&config, text).unwrap();

    let mut c = d.client().await;
    let id = Uuid::new_v4();
    c.spawn(d.spec(id)).await;
    c.attach_until(id, BANNER).await;

    let git_req = c.request(repo.log(false, 0, 10)).await;
    c.send(Frame::Data {
        session: id,
        bytes: b"still alive\r".to_vec(),
    })
    .await;
    let mut seen = Vec::new();
    c.until(|f| match f {
        Frame::Control(env) if env.req == Some(git_req) => {
            panic!("git answered while it should be blocked: {env:?}")
        }
        Frame::Data { session, bytes } if *session == id => {
            seen.extend_from_slice(bytes);
            String::from_utf8_lossy(&seen)
                .contains("still alive")
                .then_some(())
        }
        _ => None,
    })
    .await;

    // Release git: a writer lets the blocked open finish; then drop the FIFO
    // so the commands that follow find nothing to wait for.
    let release = fifo.clone();
    within(tokio::task::spawn_blocking(move || {
        drop(
            std::fs::OpenOptions::new()
                .write(true)
                .open(&release)
                .unwrap(),
        );
        std::fs::remove_file(&release).unwrap();
    }))
    .await
    .unwrap();
    let reply = c
        .until(|f| match f {
            Frame::Control(Envelope { req: Some(r), msg }) if *r == git_req => Some(msg.clone()),
            _ => None,
        })
        .await;
    assert!(matches!(reply, Msg::GitLogPage(_)), "{reply:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_requests_are_answered_out_of_order_without_losing_any() {
    let d = TestDaemon::start().await;
    let repo = Repo::create(&d.dir);
    let mut c = d.client().await;
    // More requests than the per-client limit of 2 running at once.
    let reqs = [
        c.request(repo.log(false, 0, 5)).await,
        c.request(repo.commit(&repo.merge)).await,
        c.request(repo.diff(&repo.renamed, None, None)).await,
        c.request(repo.log(true, 0, 5)).await,
        c.request(repo.commit(&repo.first)).await,
    ];
    let mut answered = std::collections::HashSet::new();
    while answered.len() < reqs.len() {
        let req = c
            .until(|f| match f {
                Frame::Control(Envelope { req: Some(r), msg }) if reqs.contains(r) => {
                    assert!(!matches!(msg, Msg::Error { .. }), "{msg:?}");
                    Some(*r)
                }
                _ => None,
            })
            .await;
        assert!(answered.insert(req), "answered twice");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_leaves_does_not_wedge_the_daemon() {
    let d = TestDaemon::start().await;
    let repo = Repo::create(&d.dir);
    let mut gone = d.client().await;
    for _ in 0..6 {
        gone.request(repo.log(false, 0, 5)).await;
    }
    drop(gone);
    let mut c = d.client().await;
    let page = log_page(&mut c, repo.log(false, 0, 5)).await;
    assert_eq!(page.commits.len(), 5);
}
