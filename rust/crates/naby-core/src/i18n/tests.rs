use super::*;

fn holes(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s
        .split('{')
        .skip(1)
        .filter_map(|p| p.split('}').next().map(str::to_owned))
        .collect();
    v.sort();
    v
}

#[test]
fn every_language_has_every_key() {
    assert_eq!(ENTRIES.len(), 325);
    let mut seen = std::collections::HashSet::new();
    for (en, es, id) in ENTRIES.iter() {
        assert!(!en.is_empty());
        assert!(seen.insert(*en), "duplicate key {en}");
        assert!(!es.is_empty(), "missing Spanish for {en}");
        assert!(!id.is_empty(), "missing Indonesian for {en}");
        assert_eq!(holes(en), holes(es), "placeholders differ (es): {en}");
        assert_eq!(holes(en), holes(id), "placeholders differ (id): {en}");
    }
    for l in languages() {
        for (en, _, _) in ENTRIES.iter() {
            assert!(!t(l.code, en).is_empty());
        }
    }
}

#[test]
fn translate_and_fall_back() {
    assert_eq!(t("es", "Settings"), "Ajustes");
    assert_eq!(t("id", "Settings"), "Pengaturan");
    assert_eq!(t("en", "Settings"), "Settings");
    assert_eq!(t("fr", "Settings"), "Settings");
    assert_eq!(t("es", "Not a catalog caption"), "Not a catalog caption");
    assert_eq!(t("es", ""), "");
    assert_eq!(t("es", "Weather"), "Tiempo");
}

#[test]
fn format_substitutes_placeholders() {
    assert_eq!(
        format("es", "Available: {0}", &[&"2.25.0"]),
        "Disponible: 2.25.0"
    );
    assert_eq!(
        format("en", "Version {0} is available.", &[&"1.2.3"]),
        "Version 1.2.3 is available."
    );
    assert_eq!(format("en", "no args {0} {1}", &[&5]), "no args 5 {1}");
    assert_eq!(format("en", "{{literal}} {0}", &[&1]), "{literal} 1");
    assert_eq!(format("en", "dangling {", &[]), "dangling {");
}

#[test]
fn languages_and_normalize() {
    let codes: Vec<_> = languages().iter().map(|l| l.code).collect();
    assert_eq!(codes, ["en", "es", "id"]);
    assert_eq!(normalize_language("id"), "id");
    assert_eq!(normalize_language("invalid"), "en");
    assert_eq!(normalize_language(""), "en");
}

#[test]
fn accessibility_scale_math() {
    let mut s = crate::settings::Settings {
        accessibility_text_scale: 9.0,
        accessibility_high_contrast: true,
        ..Default::default()
    };
    let a = Accessibility::from_settings(&s);
    assert_eq!(a.text_scale, 1.5);
    assert!(a.high_contrast(false));
    assert_eq!(a.font_size(12.0), 18.0);
    assert_eq!(a.scale_percent_text(), "150%");
    s.accessibility_text_scale = f64::NAN;
    s.accessibility_high_contrast = false;
    let a = Accessibility::from_settings(&s);
    assert_eq!(a.text_scale, 1.0);
    assert!(!a.high_contrast(false) && a.high_contrast(true));
    assert_eq!(scaled_font_size(10.0, f64::NAN), 10.0);
    assert_eq!(scaled_font_size(f64::NAN, 2.0), 0.0);
}
