//! Protocol version, kept apart so `paths` can name sockets without
//! depending on `proto`.

/// Protocol version. Bump on incompatible changes; it is part of the daemon
/// socket name, so daemons of different versions coexist instead of colliding.
pub const PROTO: u32 = 1;
