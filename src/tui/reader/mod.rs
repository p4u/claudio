//! The reader view (Alt+u): the session's recent answers, rendered by
//! claudio from the transcript, with their formulas typeset.
//!
//! - [`doc`] — an answer as markdown blocks (on the render worker).
//! - [`layout`] — such blocks wrapped to the view's width.

pub mod doc;
pub mod layout;
