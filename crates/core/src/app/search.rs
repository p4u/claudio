//! The conversation search over every connected host, for both front-ends
//! (A5): the web's `search` op and the TUI wizard's "Search conversations"
//! row. One `Search` per host, each within [`DEADLINE`]; a host that answers
//! `search busy` (its daemon runs one search at a time) is asked once more
//! after [`BUSY_RETRY`]. The hits come back merged newest first with their
//! host, and every host that failed is named with its error.

use std::future::Future;
use std::io;
use std::time::Duration;

use crate::client::Client;
use crate::proto::{Msg, SearchHit, Welcome};

/// A host's deadline, counting whatever its search waits for first.
pub const DEADLINE: Duration = Duration::from_secs(10);
/// The pause before asking a busy host again.
pub const BUSY_RETRY: Duration = Duration::from_millis(500);

/// What a search over several hosts found.
#[derive(Debug, Default, PartialEq)]
pub struct Found {
    /// `(host, hit)`, newest first.
    pub hits: Vec<(String, SearchHit)>,
    /// `(host, error)` for every host that did not answer.
    pub failed: Vec<(String, String)>,
    /// Whether a host left hits out (its reply was cut).
    pub truncated: bool,
}

/// One host's hits and whether its reply was cut.
pub type HostHits = Result<(Vec<SearchHit>, bool), String>;

/// Whether the daemon behind `welcome` answers `Search`.
pub fn can_search(welcome: &Welcome) -> bool {
    welcome.features.iter().any(|f| f == "search")
}

/// Ask one host: `ask` sends the `Search`; after `search busy` it is sent
/// once more, [`BUSY_RETRY`] later.
pub async fn ask_host<F, Fut>(ask: F) -> HostHits
where
    F: Fn() -> Fut,
    Fut: Future<Output = io::Result<Msg>>,
{
    let mut reply = ask().await;
    if matches!(&reply, Err(e) if e.to_string() == "search busy") {
        tokio::time::sleep(BUSY_RETRY).await;
        reply = ask().await;
    }
    match reply.map_err(|e| e.to_string())? {
        Msg::SearchReply {
            hits, truncated, ..
        } => Ok((hits, truncated)),
        _ => Err("unexpected reply from the daemon".to_owned()),
    }
}

/// Search every host of `hosts` at once with `one`, each within
/// [`DEADLINE`], and merge what they found.
pub async fn search_all<F, Fut>(hosts: Vec<String>, one: F) -> Found
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = HostHits>,
{
    let asks = hosts.into_iter().map(|host| {
        let ask = one(host.clone());
        async move {
            let res = tokio::time::timeout(DEADLINE, ask)
                .await
                .unwrap_or_else(|_| Err("timed out".to_owned()));
            (host, res)
        }
    });
    let mut found = Found::default();
    for (host, res) in futures::future::join_all(asks).await {
        match res {
            Ok((hits, cut)) => {
                found.truncated |= cut;
                found
                    .hits
                    .extend(hits.into_iter().map(|h| (host.clone(), h)));
            }
            Err(error) => found.failed.push((host, error)),
        }
    }
    found.hits.sort_by_key(|h| std::cmp::Reverse(h.1.modified));
    found
}

/// Search the conversations of `clients` (`(host, client)`, each one
/// [`can_search`] accepts) for `query`, asking for the daemon's default
/// number of hits (`limit: 0`).
pub async fn search_clients(clients: Vec<(String, Client)>, query: String) -> Found {
    let hosts = clients.iter().map(|(h, _)| h.clone()).collect();
    search_all(hosts, |host| {
        let client = clients
            .iter()
            .find(|(h, _)| *h == host)
            .map(|(_, c)| c.clone());
        let query = query.clone();
        async move {
            let client = client.ok_or_else(|| "not connected".to_owned())?;
            ask_host(|| {
                client.request(Msg::Search {
                    query: query.clone(),
                    limit: 0,
                })
            })
            .await
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn hit(conversation: &str, modified: u64) -> SearchHit {
        SearchHit {
            conversation: conversation.into(),
            cwd: "/w".into(),
            modified,
            title: "t".into(),
            snippet: "s".into(),
            matches: 1,
        }
    }

    fn reply(hits: Vec<SearchHit>) -> io::Result<Msg> {
        Ok(Msg::SearchReply {
            query: "q".into(),
            hits,
            scanned: 1,
            total: 1,
            truncated: false,
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_busy_host_is_asked_once_more_after_500_ms() {
        let calls = AtomicU32::new(0);
        let start = tokio::time::Instant::now();
        let got = ask_host(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                match n {
                    0 => Err(io::Error::other("search busy")),
                    _ => reply(vec![hit("c1", 5)]),
                }
            }
        })
        .await;
        assert_eq!(got, Ok((vec![hit("c1", 5)], false)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(start.elapsed(), BUSY_RETRY);
    }

    #[tokio::test(start_paused = true)]
    async fn still_busy_after_the_retry_is_a_failure() {
        let calls = AtomicU32::new(0);
        let got = ask_host(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(io::Error::other("search busy")) }
        })
        .await;
        assert_eq!(got, Err("search busy".to_owned()));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one retry only");
    }

    #[tokio::test(start_paused = true)]
    async fn hosts_merge_newest_first_and_failures_are_named() {
        let hosts = vec!["local".into(), "down".into(), "slow".into(), "z6".into()];
        let found = search_all(hosts, |host| async move {
            match host.as_str() {
                "local" => Ok((vec![hit("old", 10), hit("new", 30)], false)),
                "z6" => Ok((vec![hit("mid", 20)], true)),
                "down" => Err("connection closed".to_owned()),
                _ => std::future::pending().await,
            }
        })
        .await;
        let order: Vec<(&str, &str)> = found
            .hits
            .iter()
            .map(|(h, x)| (h.as_str(), x.conversation.as_str()))
            .collect();
        assert_eq!(order, [("local", "new"), ("z6", "mid"), ("local", "old")]);
        assert_eq!(
            found.failed,
            [
                ("down".to_owned(), "connection closed".to_owned()),
                ("slow".to_owned(), "timed out".to_owned())
            ]
        );
        assert!(found.truncated);
    }

    #[tokio::test]
    async fn no_host_finds_nothing() {
        let found = search_clients(Vec::new(), "q".into()).await;
        assert_eq!(found, Found::default());
    }
}
