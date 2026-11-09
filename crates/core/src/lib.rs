//! claudio-core: the generic core of claudio. The daemon protocol and the
//! session daemon, paths and config, what claudio knows about `claude`, the
//! terminal model, the client, SSH remotes and freshness, release and
//! upgrade, the addon API (`addon`) and the layer both front-ends share
//! (`app`). It names no addon and draws nothing (ACCEPTANCE A1, D7).

pub mod addon;
pub mod app;
pub mod claude;
pub mod client;
pub mod config;
pub mod daemon;
pub mod freshness;
pub mod paths;
pub mod proto;
pub mod release;
pub mod remote;
pub mod term;
pub mod text;
pub mod upgrade;
pub mod version;
