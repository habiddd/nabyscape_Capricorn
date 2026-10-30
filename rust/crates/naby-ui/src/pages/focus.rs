//! The `focus` page: the countdown view in its focus / break mode (see `timer.rs`).

use super::timer::CountdownPage;
use super::Page;

pub fn build() -> Box<dyn Page> {
    Box::new(CountdownPage::new(true))
}
