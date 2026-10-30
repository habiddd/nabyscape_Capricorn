//! Small media policies: browser preview filtering, lyrics side-layout width and the
//! closed-dock media content/icon decisions (`MediaPreviewPolicy`, `MediaLyricsLayout`,
//! `ClosedLyrics.cs` / `ClosedMediaIcon.cs` conditions).

// ---- Browser preview policy ----

const BROWSER_NAMES: [&str; 8] = [
    "chrome", "msedge", "firefox", "brave", "opera", "vivaldi", "arc", "browser",
];

pub fn is_browser(source: Option<&str>) -> bool {
    let value = source.unwrap_or("").to_lowercase();
    BROWSER_NAMES.iter().any(|name| value.contains(name))
}

/// A GUID or a 12+ character hex id: an app identity that says nothing readable.
pub fn opaque_identity(source: Option<&str>) -> bool {
    let Some(source) = source else { return false };
    if is_guid(source) {
        return true;
    }
    source.chars().count() >= 12 && source.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_guid(text: &str) -> bool {
    let mut t = text.trim();
    for (open, close) in [('{', '}'), ('(', ')')] {
        if let Some(inner) = t.strip_prefix(open).and_then(|r| r.strip_suffix(close)) {
            t = inner;
            break;
        }
    }
    let hex = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit());
    if t.len() == 32 {
        return hex(t);
    }
    let parts: Vec<&str> = t.split('-').collect();
    t.len() == 36
        && parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && hex(p))
}

/// Automatic previews are allowed unless the browser-preview switch is off and the
/// source looks like a browser or an opaque identity.
pub fn allow_automatic(browser_media_previews: bool, source: Option<&str>) -> bool {
    browser_media_previews || !(is_browser(source) || opaque_identity(source))
}

// ---- Lyrics layout ----

pub const MINIMUM_BODY_WIDTH: f64 = 500.0;

/// Panel width with the lyrics column: it expands only when the scaled body stays
/// at least [`MINIMUM_BODY_WIDTH`] wide, otherwise the saved width is kept.
pub fn lyrics_layout_width(
    panel_width: f64,
    panel_scale: f64,
    panel_padding: f64,
    available: f64,
    lyrics: bool,
) -> f64 {
    let normal = panel_width.min(available);
    if !lyrics {
        return normal;
    }
    let extended = (normal + 280.0 * panel_scale).min(available);
    if (extended - 2.0 * panel_padding) / panel_scale >= MINIMUM_BODY_WIDTH {
        extended
    } else {
        normal
    }
}

// ---- Closed dock ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClosedMediaContent {
    #[default]
    Title,
    Waveform,
    Lyrics,
    None,
}

impl ClosedMediaContent {
    /// Unknown text normalizes to `Title`.
    pub fn parse(text: &str) -> Self {
        match text {
            "Waveform" => ClosedMediaContent::Waveform,
            "Lyrics" => ClosedMediaContent::Lyrics,
            "None" => ClosedMediaContent::None,
            _ => ClosedMediaContent::Title,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClosedMediaIcon {
    #[default]
    Waveform,
    Artwork,
    None,
}

impl ClosedMediaIcon {
    /// Unknown text normalizes to `Waveform`.
    pub fn parse(text: &str) -> Self {
        match text {
            "Artwork" => ClosedMediaIcon::Artwork,
            "None" => ClosedMediaIcon::None,
            _ => ClosedMediaIcon::Waveform,
        }
    }
}

/// What else is going on in the closed dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClosedDockContext {
    /// Window visible and not minimized.
    pub visible: bool,
    pub user_hidden: bool,
    pub tucked: bool,
    pub tuck_requested: bool,
    pub expanded: bool,
    pub peeking: bool,
    /// Volume/brightness feedback is showing.
    pub feedback_visible: bool,
    pub notification_active: bool,
    /// Voice assistant activity owns the closed dock.
    pub voice_activity: bool,
    /// A focus/timer countdown owns the closed dock.
    pub countdown_active: bool,
}

/// `ClosedMediaActive`: closed-dock media is enabled, the widget is on and something plays.
pub fn closed_media_active(
    content: ClosedMediaContent,
    media_widget_on: bool,
    media_available: bool,
    media_playing: bool,
) -> bool {
    content != ClosedMediaContent::None && media_widget_on && media_available && media_playing
}

/// `WantsClosedLyrics` (without the disposed flag): should lyric data be loaded and ticked?
pub fn wants_closed_lyrics(
    content: ClosedMediaContent,
    media_active: bool,
    ctx: &ClosedDockContext,
) -> bool {
    content == ClosedMediaContent::Lyrics
        && media_active
        && ctx.visible
        && !ctx.user_hidden
        && !ctx.tucked
        && !ctx.tuck_requested
        && !ctx.expanded
        && !ctx.peeking
        && !ctx.feedback_visible
        && !ctx.notification_active
        && !ctx.voice_activity
        && !ctx.countdown_active
}

/// `UpdateClosedMediaWaveform`: waveform replaces the title text.
pub fn closed_waveform_visible(
    content: ClosedMediaContent,
    media_active: bool,
    lyric_line_active: bool,
    ctx: &ClosedDockContext,
) -> bool {
    !ctx.notification_active
        && !ctx.expanded
        && !ctx.peeking
        && !ctx.feedback_visible
        && !ctx.countdown_active
        && !ctx.voice_activity
        && media_active
        && (content == ClosedMediaContent::Waveform
            || (content == ClosedMediaContent::Lyrics && !lyric_line_active))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClosedMediaView {
    pub cover: bool,
    pub waveform: bool,
    /// The leading activity icon is visible.
    pub activity: bool,
}

/// `UpdateClosedMediaIcon`: which of cover art, waveform and activity icon show.
/// `countdown_running` is a focus/timer with time remaining (widget on).
#[allow(clippy::too_many_arguments)]
pub fn closed_media_view(
    content: ClosedMediaContent,
    icon: ClosedMediaIcon,
    media_active: bool,
    has_artwork: bool,
    lyric_line_active: bool,
    countdown_running: bool,
    ctx: &ClosedDockContext,
) -> ClosedMediaView {
    let waveform = closed_waveform_visible(content, media_active, lyric_line_active, ctx);
    let cover = !ctx.feedback_visible
        && !countdown_running
        && !ctx.voice_activity
        && icon == ClosedMediaIcon::Artwork
        && media_active
        && has_artwork;
    let hidden = ctx.feedback_visible
        || cover
        || waveform
        || ctx.voice_activity
        || (matches!(
            content,
            ClosedMediaContent::Lyrics | ClosedMediaContent::None
        ) || icon == ClosedMediaIcon::None)
            && !countdown_running
        || !(countdown_running || media_active);
    ClosedMediaView {
        cover,
        waveform,
        activity: !hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_auto_preview_switch_leaves_spotify_intact() {
        assert!(!allow_automatic(false, Some("chrome.exe")));
        assert!(!allow_automatic(false, Some("Microsoft.MSEdge_8weky")));
        assert!(!allow_automatic(false, Some("58F7597A8562782A")));
        assert!(allow_automatic(false, Some("Spotify.exe")));
        assert!(allow_automatic(false, None));
        assert!(allow_automatic(true, Some("chrome.exe")));
    }

    #[test]
    fn opaque_identity_forms() {
        assert!(opaque_identity(Some(
            "123e4567-e89b-12d3-a456-426614174000"
        )));
        assert!(opaque_identity(Some(
            "{123e4567-e89b-12d3-a456-426614174000}"
        )));
        assert!(opaque_identity(Some("123e4567e89b12d3a456426614174000")));
        assert!(opaque_identity(Some("ABCDEF012345")));
        assert!(!opaque_identity(Some("ABCDEF01234")));
        assert!(!opaque_identity(Some("")));
        assert!(!opaque_identity(None));
        assert!(!opaque_identity(Some("Spotify.exe")));
        assert!(!opaque_identity(Some(
            "123e4567-e89b-12d3-a456-42661417400g"
        )));
        assert!(is_browser(Some("FIREFOX.exe")) && !is_browser(None));
    }

    #[test]
    fn side_lyrics_expand_only_within_working_area_and_restore_width() {
        assert!(lyrics_layout_width(526.0, 1.04, 26.0, 1900.0, true) > 800.0);
        assert_eq!(lyrics_layout_width(526.0, 1.04, 26.0, 1900.0, false), 526.0);
        assert_eq!(lyrics_layout_width(526.0, 1.04, 26.0, 600.0, true), 600.0);
        assert_eq!(lyrics_layout_width(526.0, 1.04, 26.0, 480.0, true), 480.0);
    }

    #[test]
    fn lyrics_avoid_squeezing_two_columns_into_narrow_scaled_area() {
        for scale in [1.0, 1.5, 2.0] {
            let w = lyrics_layout_width(526.0, scale, 26.0, 560.0, true);
            assert!(
                w == 526.0 || (w - 52.0) / scale >= MINIMUM_BODY_WIDTH,
                "{scale} {w}"
            );
        }
    }

    #[test]
    fn all_closed_dock_modes_parse_and_unknown_normalizes() {
        for (text, mode) in [
            ("Title", ClosedMediaContent::Title),
            ("Waveform", ClosedMediaContent::Waveform),
            ("Lyrics", ClosedMediaContent::Lyrics),
            ("None", ClosedMediaContent::None),
            ("junk", ClosedMediaContent::Title),
        ] {
            assert_eq!(ClosedMediaContent::parse(text), mode);
        }
        assert_eq!(ClosedMediaIcon::parse("None"), ClosedMediaIcon::None);
        assert_eq!(ClosedMediaIcon::parse("junk"), ClosedMediaIcon::Waveform);
    }

    fn idle() -> ClosedDockContext {
        ClosedDockContext {
            visible: true,
            ..Default::default()
        }
    }

    #[test]
    fn lyrics_demand_follows_dock_state() {
        let active = closed_media_active(ClosedMediaContent::Lyrics, true, true, true);
        assert!(active);
        assert!(wants_closed_lyrics(
            ClosedMediaContent::Lyrics,
            active,
            &idle()
        ));
        assert!(!wants_closed_lyrics(
            ClosedMediaContent::Title,
            active,
            &idle()
        ));
        let tweaks: [fn(&mut ClosedDockContext); 10] = [
            |c| c.visible = false,
            |c| c.user_hidden = true,
            |c| c.tucked = true,
            |c| c.tuck_requested = true,
            |c| c.expanded = true,
            |c| c.peeking = true,
            |c| c.feedback_visible = true,
            |c| c.notification_active = true,
            |c| c.voice_activity = true,
            |c| c.countdown_active = true,
        ];
        for tweak in tweaks {
            let mut ctx = idle();
            tweak(&mut ctx);
            assert!(!wants_closed_lyrics(
                ClosedMediaContent::Lyrics,
                active,
                &ctx
            ));
        }
        assert!(!closed_media_active(
            ClosedMediaContent::None,
            true,
            true,
            true
        ));
        assert!(!closed_media_active(
            ClosedMediaContent::Title,
            true,
            true,
            false
        ));
        assert!(!closed_media_active(
            ClosedMediaContent::Title,
            false,
            true,
            true
        ));
    }

    #[test]
    fn waveform_replaces_title_in_waveform_mode_and_lyric_breaks() {
        let ctx = idle();
        assert!(closed_waveform_visible(
            ClosedMediaContent::Waveform,
            true,
            false,
            &ctx
        ));
        assert!(closed_waveform_visible(
            ClosedMediaContent::Lyrics,
            true,
            false,
            &ctx
        ));
        assert!(!closed_waveform_visible(
            ClosedMediaContent::Lyrics,
            true,
            true,
            &ctx
        ));
        assert!(!closed_waveform_visible(
            ClosedMediaContent::Title,
            true,
            false,
            &ctx
        ));
        assert!(!closed_waveform_visible(
            ClosedMediaContent::Waveform,
            false,
            false,
            &ctx
        ));
        let busy = ClosedDockContext {
            expanded: true,
            ..idle()
        };
        assert!(!closed_waveform_visible(
            ClosedMediaContent::Waveform,
            true,
            false,
            &busy
        ));
    }

    #[test]
    fn closed_icon_view_matrix() {
        let ctx = idle();
        let v = closed_media_view(
            ClosedMediaContent::Title,
            ClosedMediaIcon::Artwork,
            true,
            true,
            false,
            false,
            &ctx,
        );
        assert_eq!(
            v,
            ClosedMediaView {
                cover: true,
                waveform: false,
                activity: false
            }
        );
        let v = closed_media_view(
            ClosedMediaContent::Title,
            ClosedMediaIcon::Artwork,
            true,
            false,
            false,
            false,
            &ctx,
        );
        assert!(!v.cover && v.activity);
        let v = closed_media_view(
            ClosedMediaContent::Waveform,
            ClosedMediaIcon::Waveform,
            true,
            true,
            false,
            false,
            &ctx,
        );
        assert!(v.waveform && !v.activity);
        let v = closed_media_view(
            ClosedMediaContent::Title,
            ClosedMediaIcon::None,
            true,
            true,
            false,
            false,
            &ctx,
        );
        assert!(!v.activity);
        let v = closed_media_view(
            ClosedMediaContent::Title,
            ClosedMediaIcon::Waveform,
            true,
            true,
            false,
            true,
            &ctx,
        );
        assert!(v.activity && !v.cover);
        let v = closed_media_view(
            ClosedMediaContent::Title,
            ClosedMediaIcon::Waveform,
            false,
            false,
            false,
            false,
            &ctx,
        );
        assert!(!v.activity);
    }
}
