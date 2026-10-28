//! Images: PNG files that keep their exact format through recoloring, float
//! views of their pixels for the transforms, and drawing helpers for
//! previews and contact sheets.
//!
//! The `image` and `png` crates are written `::image` / `::png` in this
//! crate, to tell them apart from these modules.
pub mod draw;
pub mod pixels;
pub mod png;
