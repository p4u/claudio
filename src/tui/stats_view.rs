//! The proxy stats popup (Alt+s): a pure, multi-page view state machine.
//!
//! This module owns the popup's page, time window, scroll offset and loading
//! flag, and turns key presses into [`StatsOutcome`]s. Rendering lives in
//! `ui/stats.rs`; fetching in `tui/mod.rs` (via `Effect::FetchProxyStats`).
//! Nothing here does I/O, so every transition is unit-testable.

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ── Window ────────────────────────────────────────────────────────────────────

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

// ── Page ──────────────────────────────────────────────────────────────────────

/// One page of the popup, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Models,
    Trends,
    Pool,
    Sessions,
}

impl Page {
    pub const ALL: [Page; 5] = [
        Page::Overview,
        Page::Models,
        Page::Trends,
        Page::Pool,
        Page::Sessions,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Models => "Models",
            Page::Trends => "Trends",
            Page::Pool => "Pool & limits",
            Page::Sessions => "Sessions",
        }
    }

    /// 1-based position, also the number key that jumps to the page.
    pub fn index(self) -> usize {
        Page::ALL.iter().position(|p| *p == self).unwrap_or(0) + 1
    }

    fn next(self) -> Page {
        Page::ALL[self.index() % Page::ALL.len()]
    }

    fn prev(self) -> Page {
        Page::ALL[(self.index() + Page::ALL.len() - 2) % Page::ALL.len()]
    }

    /// The windows a page needs to render fully. The Trends page compares all
    /// three; every other page shows the selected one.
    fn windows(self, selected: Window) -> Vec<Window> {
        match self {
            Page::Trends => Window::ALL.to_vec(),
            _ => vec![selected],
        }
    }
}

// ── View ──────────────────────────────────────────────────────────────────────

/// State of the stats popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsView {
    /// Proxy profile being shown. `None` when the manager has no proxy at all;
    /// the popup then explains how to configure one.
    pub profile: Option<String>,
    /// Set when the active session runs direct and the stats shown belong to
    /// the default profile instead.
    pub borrowed_profile: bool,
    pub page: Page,
    pub window: Window,
    /// Scroll offset of the page body, in lines (see [`StatsView::scroll`]).
    scroll: u16,
    /// The largest useful scroll offset of the page, as the renderer last
    /// measured it (it alone knows how many lines the page has).
    max_scroll: Cell<u16>,
    /// A fetch is in flight for this profile.
    pub loading: bool,
}

/// What the app should do after a key press.
#[derive(Debug, PartialEq, Eq)]
pub enum StatsOutcome {
    Nothing,
    Close,
    /// Fetch stats for these windows (the app adds pool health, and the model
    /// catalogue when it is not cached yet).
    Fetch(Vec<Window>),
}

impl StatsView {
    /// Open the popup on the Overview page. Returns the initial fetch, if any.
    pub fn open(profile: Option<String>, borrowed_profile: bool) -> (StatsView, StatsOutcome) {
        let view = StatsView {
            profile,
            borrowed_profile,
            page: Page::Overview,
            window: Window::H24,
            scroll: 0,
            // Not drawn yet: nothing to limit scrolling to.
            max_scroll: Cell::new(u16::MAX),
            loading: false,
        };
        let outcome = view.fetch_missing(&[]);
        (view, outcome)
    }

    /// The scroll offset to draw the page at.
    pub fn scroll(&self) -> u16 {
        self.scroll.min(self.max_scroll.get())
    }

    /// Record the largest useful scroll offset of the page being drawn.
    pub fn set_max_scroll(&self, max: u16) {
        self.max_scroll.set(max);
    }

    /// Handle a key. `cached` lists the windows already fetched for this
    /// profile.
    pub fn on_key(&mut self, key: &KeyEvent, cached: &[Window]) -> StatsOutcome {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT)
        {
            return StatsOutcome::Nothing;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => StatsOutcome::Close,
            KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => {
                self.goto(self.page.next(), cached)
            }
            KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => {
                self.goto(self.page.prev(), cached)
            }
            KeyCode::Char(c @ '1'..='9') => match Page::ALL.get(c as usize - '1' as usize) {
                Some(p) => self.goto(*p, cached),
                None => StatsOutcome::Nothing,
            },
            KeyCode::Char('w') => {
                self.window = self.window.next();
                self.scroll = 0;
                self.fetch_missing(cached)
            }
            KeyCode::Char('r') => self.fetch(self.page.windows(self.window)),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_to(self.scroll().saturating_add(1)),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_to(self.scroll().saturating_sub(1)),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.scroll_to(self.scroll().saturating_add(10))
            }
            KeyCode::PageUp => self.scroll_to(self.scroll().saturating_sub(10)),
            KeyCode::Home | KeyCode::Char('g') => self.scroll_to(0),
            KeyCode::End | KeyCode::Char('G') => self.scroll_to(u16::MAX),
            _ => StatsOutcome::Nothing,
        }
    }

    /// A fetch completed (successfully or not) for this profile.
    pub fn on_fetched(&mut self) {
        self.loading = false;
    }

    fn goto(&mut self, page: Page, cached: &[Window]) -> StatsOutcome {
        if page != self.page {
            self.page = page;
            self.scroll = 0;
        }
        self.fetch_missing(cached)
    }

    /// Scroll to `to`, which may be past the end: [`StatsView::scroll`]
    /// clamps, and the next move starts from where the page is drawn.
    fn scroll_to(&mut self, to: u16) -> StatsOutcome {
        self.scroll = to;
        StatsOutcome::Nothing
    }

    /// Fetch whatever the current page needs that is not cached yet.
    fn fetch_missing(&self, cached: &[Window]) -> StatsOutcome {
        let missing: Vec<Window> = self
            .page
            .windows(self.window)
            .into_iter()
            .filter(|w| !cached.contains(w))
            .collect();
        if missing.is_empty() {
            StatsOutcome::Nothing
        } else {
            self.fetch(missing)
        }
    }

    fn fetch(&self, windows: Vec<Window>) -> StatsOutcome {
        if self.profile.is_none() {
            return StatsOutcome::Nothing;
        }
        StatsOutcome::Fetch(windows)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn view() -> StatsView {
        StatsView::open(Some("p".into()), false).0
    }

    #[test]
    fn open_fetches_24h_for_a_profile_only() {
        let (v, out) = StatsView::open(Some("p".into()), false);
        assert_eq!(v.page, Page::Overview);
        assert_eq!(v.window, Window::H24);
        assert_eq!(out, StatsOutcome::Fetch(vec![Window::H24]));

        let (_, out) = StatsView::open(None, false);
        assert_eq!(out, StatsOutcome::Nothing);
    }

    #[test]
    fn page_switching_wraps_and_resets_scroll() {
        let mut v = view();
        v.scroll = 5;
        assert_eq!(
            v.on_key(&press(KeyCode::Right), &[Window::H24]),
            StatsOutcome::Nothing
        );
        assert_eq!(v.page, Page::Models);
        assert_eq!(v.scroll(), 0);
        v.on_key(&press(KeyCode::Left), &[Window::H24]);
        v.on_key(&press(KeyCode::Left), &[Window::H24]);
        assert_eq!(
            v.page,
            Page::Sessions,
            "Left from Overview wraps to the last page"
        );
        v.on_key(&press(KeyCode::Tab), &[Window::H24]);
        assert_eq!(v.page, Page::Overview);
        v.on_key(&press(KeyCode::Char('4')), &[Window::H24]);
        assert_eq!(v.page, Page::Pool);
        v.on_key(&press(KeyCode::Char('9')), &[Window::H24]);
        assert_eq!(v.page, Page::Pool, "out-of-range number is ignored");
    }

    #[test]
    fn trends_page_fetches_missing_windows() {
        let mut v = view();
        let out = v.on_key(&press(KeyCode::Char('3')), &[Window::H24]);
        assert_eq!(out, StatsOutcome::Fetch(vec![Window::D7, Window::D30]));
        let out = v.on_key(&press(KeyCode::Char('3')), &Window::ALL);
        assert_eq!(out, StatsOutcome::Nothing, "all cached: nothing to fetch");
    }

    #[test]
    fn window_change_fetches_only_when_uncached() {
        let mut v = view();
        let out = v.on_key(&press(KeyCode::Char('w')), &[Window::H24]);
        assert_eq!(v.window, Window::D7);
        assert_eq!(out, StatsOutcome::Fetch(vec![Window::D7]));
        let out = v.on_key(&press(KeyCode::Char('w')), &[Window::H24, Window::D30]);
        assert_eq!(v.window, Window::D30);
        assert_eq!(out, StatsOutcome::Nothing);
        v.on_key(&press(KeyCode::Char('w')), &Window::ALL);
        assert_eq!(v.window, Window::H24, "cycles back");
    }

    #[test]
    fn refresh_forces_a_fetch_of_the_page_windows() {
        let mut v = view();
        assert_eq!(
            v.on_key(&press(KeyCode::Char('r')), &Window::ALL),
            StatsOutcome::Fetch(vec![Window::H24])
        );
        v.page = Page::Trends;
        assert_eq!(
            v.on_key(&press(KeyCode::Char('r')), &Window::ALL),
            StatsOutcome::Fetch(Window::ALL.to_vec())
        );
        let mut none = StatsView::open(None, false).0;
        assert_eq!(
            none.on_key(&press(KeyCode::Char('r')), &[]),
            StatsOutcome::Nothing
        );
    }

    #[test]
    fn scroll_is_bounded_by_the_drawn_page() {
        let mut v = view();
        v.set_max_scroll(3);
        for _ in 0..5 {
            v.on_key(&press(KeyCode::Down), &[]);
        }
        assert_eq!(v.scroll(), 3);
        v.on_key(&press(KeyCode::PageDown), &[]);
        assert_eq!(v.scroll(), 3);
        v.on_key(&press(KeyCode::Up), &[]);
        assert_eq!(v.scroll(), 2, "moves start where the page is drawn");
        v.on_key(&press(KeyCode::Home), &[]);
        assert_eq!(v.scroll(), 0);
        v.on_key(&press(KeyCode::Up), &[]);
        assert_eq!(v.scroll(), 0, "never underflows");
        v.on_key(&press(KeyCode::End), &[]);
        assert_eq!(v.scroll(), 3);
        // The page shrinks (new data): drawn at its new end.
        v.set_max_scroll(1);
        assert_eq!(v.scroll(), 1);
        v.on_key(&press(KeyCode::Up), &[]);
        assert_eq!(v.scroll(), 0);
    }

    #[test]
    fn close_and_modifier_keys() {
        let mut v = view();
        assert_eq!(v.on_key(&press(KeyCode::Esc), &[]), StatsOutcome::Close);
        assert_eq!(
            v.on_key(&press(KeyCode::Char('q')), &[]),
            StatsOutcome::Close
        );
        let alt_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT);
        assert_eq!(v.on_key(&alt_s, &[]), StatsOutcome::Nothing);
        assert_eq!(v.page, Page::Overview);
    }

    #[test]
    fn page_index_roundtrip() {
        for (i, p) in Page::ALL.iter().enumerate() {
            assert_eq!(p.index(), i + 1);
            assert_eq!(p.next().prev(), *p);
        }
    }
}
