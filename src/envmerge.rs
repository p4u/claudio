//! Merging a session's env layers. Kept apart from `proto` (which applies it
//! in `Msg::merge_env`) so the plugin state can use it without a cycle.

/// A session's env from the plugins' env (`plugin`, in registry order) and
/// the proxy's (`proxy`, `proxy::env::session_env`): the plugin pairs whose key the
/// proxy does not set, the first of a key repeated among plugins, then the
/// proxy's. The proxy wins, and no key appears twice.
pub fn merge_env_pairs(
    plugin: &[(String, String)],
    proxy: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(plugin.len() + proxy.len());
    for (k, v) in plugin {
        let taken = proxy.iter().chain(out.iter()).any(|(p, _)| p == k);
        if !taken {
            out.push((k.clone(), v.clone()));
        }
    }
    out.extend(proxy);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_puts_plugins_first_and_lets_the_proxy_win() {
        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        let plugin = [
            pair("KB_TOKEN", "kb"),
            pair("API_TIMEOUT_MS", "1"),
            pair("KB_TOKEN", "x"),
        ];
        let proxy = vec![
            pair("ANTHROPIC_AUTH_TOKEN", "tok"),
            pair("API_TIMEOUT_MS", "30000"),
        ];
        assert_eq!(
            merge_env_pairs(&plugin, proxy.clone()),
            [
                pair("KB_TOKEN", "kb"),
                pair("ANTHROPIC_AUTH_TOKEN", "tok"),
                pair("API_TIMEOUT_MS", "30000"),
            ]
        );
        assert_eq!(merge_env_pairs(&[], proxy.clone()), proxy);
        assert_eq!(
            merge_env_pairs(&plugin[..1], Vec::new()),
            [pair("KB_TOKEN", "kb")]
        );
    }
}
