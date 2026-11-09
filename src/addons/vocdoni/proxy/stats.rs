//! Proxy stats data shared by the TUI and the web UI.

/// A stats time window, as accepted by `GET /v1/claudio/me/stats?period=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Window {
    H24,
    D7,
    D30,
}

impl Window {
    pub const ALL: [Window; 3] = [Window::H24, Window::D7, Window::D30];

    /// The `?period=` query value.
    pub fn param(self) -> &'static str {
        match self {
            Window::H24 => "24h",
            Window::D7 => "7d",
            Window::D30 => "30d",
        }
    }

    /// Length of the window in days, for per-day rates.
    pub fn days(self) -> f64 {
        match self {
            Window::H24 => 1.0,
            Window::D7 => 7.0,
            Window::D30 => 30.0,
        }
    }

    pub fn next(self) -> Window {
        match self {
            Window::H24 => Window::D7,
            Window::D7 => Window::D30,
            Window::D30 => Window::H24,
        }
    }
}
