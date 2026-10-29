//! The pinned ambient preview (C# `SettingsPinnedPreview.cs`): a copy of the "Ambient light"
//! sample that stays at the top of the page once the inline sample scrolled out of view.
//!
//! The WPF window never registered an inline sample, so the pin stayed hidden; the rule is kept
//! (and tested) here so a future inline sample pins exactly as designed.

/// Whether the pinned copy is shown. `preview_bottom` and `section_bottom` are the bottoms of
/// the inline sample and of the "Ambient light" card relative to the viewport top.
pub fn should_pin(category: &str, preview_bottom: f32, section_bottom: f32) -> bool {
    if !matches!(category, "Appearance" | "Ambience") {
        return false;
    }
    preview_bottom <= 2.0 && (category == "Ambience" || section_bottom > 16.0)
}

/// Height of the pinned sample for a form of `form_height`.
pub fn pinned_height(form_height: f32) -> f32 {
    (form_height * 0.22).clamp(42.0, 112.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_only_after_the_sample_scrolled_away_and_while_the_section_is_visible() {
        assert!(!should_pin("Ambience", 40.0, 500.0), "still visible inline");
        assert!(should_pin("Ambience", -10.0, 500.0));
        assert!(
            should_pin("Ambience", -10.0, 0.0),
            "the Ambience page always pins"
        );
        assert!(should_pin("Appearance", -10.0, 100.0));
        assert!(
            !should_pin("Appearance", -10.0, 10.0),
            "section already scrolled past"
        );
        assert!(!should_pin("Layout", -10.0, 100.0));
    }

    #[test]
    fn pinned_height_is_bounded() {
        assert_eq!(pinned_height(100.0), 42.0);
        assert_eq!(pinned_height(300.0), 66.0);
        assert_eq!(pinned_height(2000.0), 112.0);
    }
}
