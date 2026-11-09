//! The JSON events the hub sends its tabs (docs/web-protocol.md), built
//! here so the hub only decides when.

use serde_json::{json, Value};

/// An event: `fields` tagged `"t": "ev"` and `"ev": name`.
pub(crate) fn ev(name: &str, mut fields: Value) -> Value {
    fields["t"] = "ev".into();
    fields["ev"] = name.into();
    fields
}

/// A host's CPU and memory sample.
pub(crate) fn host_stats(host: &str, (cpu, mem_used, mem_total): (f32, u64, u64)) -> Value {
    ev(
        "host_stats",
        json!({"host": host, "cpu": cpu, "mem_used": mem_used, "mem_total": mem_total}),
    )
}

/// A claude install or update on `host`: its phase, versions and output tail.
pub(crate) fn claude_event(
    host: &str,
    phase: &str,
    have: Option<&str>,
    want: Option<&str>,
    version: Option<&str>,
    tail: Option<&str>,
) -> Value {
    ev(
        "claude",
        json!({"host": host, "phase": phase, "have": have, "want": want,
            "version": version, "tail": tail}),
    )
}
