//! The `shortcuts` page. Replace `build` with the real page (see `pages/mod.rs`).

use super::{Page, TitlePage};

pub fn build() -> Box<dyn Page> {
    Box::new(TitlePage::new("shortcuts"))
}
