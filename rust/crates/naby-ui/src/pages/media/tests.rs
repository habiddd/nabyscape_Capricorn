//! Behavior and snapshot tests of the media page and the volume page.

use super::footer::{DEVICES, LEVEL, MENU, MIXER, MUTE, SLIDER, SOURCE, SRC};
use super::*;
use crate::host::{MediaSource, MediaSourceChoice, MediaSources};
use crate::pages::common::harness::{cover_png, Harness};
use crate::pages::mixer;
use naby_core::media::{
    AudioOutput, AudioSnapshot, LyricResult, MediaSnapshot, MixerItem, MixerSnapshot, MixerTarget,
};
use naby_core::settings::Settings;
use std::time::Duration;

fn playing(h: &mut Harness) {
    h.snaps.media.set(MediaSnapshot {
        available: true,
        source_id: "Spotify.exe".into(),
        source_name: "Spotify".into(),
        title: "Midnight City".into(),
        artist: "M83".into(),
        playing: true,
        position: Duration::from_secs(42),
        duration: Duration::from_secs(243),
        can_toggle: true,
        can_next: true,
        can_previous: true,
        can_seek: true,
        artwork: Some(cover_png()),
        ..Default::default()
    });
    h.snaps.media_epoch = h.snaps.now;
}

fn audio(h: &mut Harness) {
    h.snaps.audio.set(AudioSnapshot {
        available: true,
        volume: 0.63,
        muted: false,
        device: "Speakers".into(),
        mic_available: true,
        mic_muted: false,
        mic_name: "Studio mic".into(),
        outputs: vec![
            AudioOutput {
                id: "spk".into(),
                name: "Speakers".into(),
            },
            AudioOutput {
                id: "hdp".into(),
                name: "Headphones".into(),
            },
        ],
        default_output_id: "spk".into(),
    });
}

fn sources(h: &mut Harness) {
    h.snaps.sources.set(MediaSources {
        sessions: vec![
            MediaSourceChoice {
                id: "Spotify.exe".into(),
                name: "Spotify".into(),
            },
            MediaSourceChoice {
                id: "chrome.exe".into(),
                name: "Google Chrome".into(),
            },
        ],
        selected: "@auto".into(),
    });
}

fn set(h: &mut Harness, f: impl FnOnce(&mut Settings)) {
    let mut s = h.snaps.settings.arc().as_ref().clone();
    f(&mut s);
    h.snaps.settings.replace(s);
    h.refresh_theme();
}

fn lyrics(h: &mut Harness) {
    set(h, |s| s.lyrics_enabled = true);
    h.snaps.lyrics.replace(LyricResult {
        lines: lrc::parse(
            "[00:30.00]Waiting in a car\n[00:40.00]Waiting for a ride in the dark\n[00:50.00]The night city grows\n[01:00.00]Look at the horizon glow",
        ),
        status: "Synced lyrics \u{b7} LRCLIB".into(),
    });
}

#[test]
fn transport_buttons_emit_media_commands() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    h.idle();
    h.click_id(PLAY);
    h.click_id(PREV);
    h.click_id(NEXT);
    assert_eq!(
        h.commands(),
        vec![
            Command::MediaToggle,
            Command::MediaPrevious,
            Command::MediaNext
        ]
    );
}

#[test]
fn disabled_transport_stays_silent_without_media() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    h.idle();
    assert!(h.ink() > 1500, "the no-media state still paints");
    for id in [PLAY, PREV, NEXT, SEEK] {
        h.click_id(id);
    }
    assert!(h.commands().is_empty());
    assert!(
        !h.has_control(LYRICS_TOGGLE),
        "lyrics need a track with details"
    );
    assert_eq!(
        h.page.next_frame(h.snaps.now),
        None,
        "nothing plays, nothing wakes"
    );
    h.save("page-media-nothing-playing");
}

#[test]
fn seeking_sends_one_command_after_the_drag() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    h.idle();
    h.drag_id(SEEK, 0.5);
    let seeks: Vec<_> = h
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::MediaSeek(f) => Some(f),
            _ => None,
        })
        .collect();
    assert_eq!(seeks.len(), 1, "{seeks:?}");
    assert!((seeks[0] - 0.5).abs() < 0.03);
}

#[test]
fn a_session_without_seeking_leaves_the_bar_disabled() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    let mut m = h.snaps.media.arc().as_ref().clone();
    m.can_seek = false;
    h.snaps.media.set(m);
    h.idle();
    h.drag_id(SEEK, 0.5);
    assert!(h.commands().is_empty());
}

#[test]
fn the_source_picker_lists_none_auto_and_sessions() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    sources(&mut h);
    h.idle();
    h.click_id(SOURCE);
    h.advance(0.3);
    h.idle();
    h.save("page-media-source-picker");
    h.click_id(SRC);
    h.click_id(SOURCE);
    h.click_id(SRC + 1);
    h.click_id(SOURCE);
    h.click_id(SRC + 3);
    assert_eq!(
        h.commands(),
        vec![
            Command::MediaSelectSource(MediaSource::None),
            Command::MediaSelectSource(MediaSource::Auto),
            Command::MediaSelectSource(MediaSource::App("chrome.exe".into())),
        ]
    );
}

#[test]
fn volume_popover_controls_the_endpoint() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    audio(&mut h);
    h.idle();
    h.click_id(LEVEL);
    h.advance(0.3);
    h.idle();
    h.save("page-media-volume-popover");
    h.commands();
    h.click_id(MUTE);
    assert_eq!(
        h.commands(),
        vec![Command::ToggleMute, Command::RefreshAudio]
    );
    h.drag_id(SLIDER, 0.25);
    let volumes: Vec<f64> = h
        .commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::SetVolume(v) => Some(v),
            _ => None,
        })
        .collect();
    assert!(!volumes.is_empty());
    assert!((volumes.last().unwrap() - 0.25).abs() < 0.03);
}

#[test]
fn the_devices_menu_switches_outputs_and_the_microphone() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    audio(&mut h);
    h.idle();
    h.click_id(LEVEL);
    h.idle();
    h.click_id(DEVICES);
    h.advance(0.3);
    h.idle();
    h.save("page-media-devices-menu");
    h.commands();
    // Items: Volume mixer, Speakers, Headphones, microphone.
    h.click_id(MENU + 2);
    assert_eq!(
        h.commands(),
        vec![Command::SetOutput("hdp".into()), Command::RefreshAudio]
    );
    h.click_id(LEVEL);
    h.idle();
    h.click_id(DEVICES);
    h.idle();
    h.click_id(MENU + 3);
    assert_eq!(
        h.commands()
            .into_iter()
            .filter(|c| matches!(c, Command::ToggleMicMute))
            .count(),
        1
    );
}

#[test]
fn no_audio_endpoint_paints_and_disables_the_slider() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    h.idle();
    h.click_id(LEVEL);
    h.advance(0.3);
    h.idle();
    assert!(!h.is_enabled(SLIDER));
    h.save("page-media-no-audio");
    h.drag_id(SLIDER, 0.4);
    assert!(h
        .commands()
        .iter()
        .all(|c| !matches!(c, Command::SetVolume(_))));
}

#[test]
fn the_mixer_opens_from_the_volume_popover_and_goes_back() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    audio(&mut h);
    h.snaps.mixer.replace(MixerSnapshot {
        apps: vec![MixerItem {
            target: MixerTarget::app("o", "s"),
            name: "Spotify".into(),
            output_name: "Speakers".into(),
            volume: 0.5,
            muted: false,
            process_path: None,
        }],
        outputs: vec![],
        errors: vec![],
    });
    h.idle();
    h.click_id(LEVEL);
    h.idle();
    h.click_id(MIXER);
    assert!(h.commands().contains(&Command::MixerRefresh));
    h.advance(0.3);
    h.idle();
    assert!(h.page.demand().mixer);
    assert!(h.has_control(mixer::BACK));
    h.save("page-media-mixer");
    h.click_id(mixer::BACK);
    h.idle();
    assert!(h.has_control(PLAY));
    assert!(!h.page.demand().mixer);
}

#[test]
fn the_lyrics_button_flips_the_setting() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    h.idle();
    h.click_id(LYRICS_TOGGLE);
    match &h.commands()[..] {
        [Command::ApplySettings(s)] => assert!(s.lyrics_enabled),
        other => panic!("unexpected {other:?}"),
    }
    lyrics(&mut h);
    h.idle();
    h.click_id(LYRICS_TOGGLE);
    match &h.commands()[..] {
        [Command::ApplySettings(s)] => assert!(!s.lyrics_enabled),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn lyrics_sit_beside_the_controls_only_when_the_panel_is_wide() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    lyrics(&mut h);
    h.width = 340.0;
    h.idle();
    assert!(h.page.wants_wide());
    assert!(h.page.demand().lyrics);
    let stacked = h.height();
    h.save("page-media-lyrics-below");
    h.width = 560.0;
    h.idle();
    let beside = h.height();
    assert!(
        beside < stacked,
        "side layout is shorter: {beside} vs {stacked}"
    );
    h.save("page-media-lyrics-side");
    set(&mut h, |s| s.lyrics_enabled = false);
    h.idle();
    assert!(!h.page.wants_wide());
    assert!(!h.page.demand().lyrics);
}

#[test]
fn playing_media_wakes_once_a_second() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    h.idle();
    let at = h.page.next_frame(h.snaps.now).expect("timeline ticks");
    assert!(at - h.snaps.now <= 1.1);
}

#[test]
fn track_changes_cross_fade_and_settle() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    h.idle();
    let mut m = h.snaps.media.arc().as_ref().clone();
    m.title = "Another Song".into();
    m.artwork = Some(cover_png());
    h.snaps.media.set(m);
    h.advance(0.05);
    h.idle();
    let at = h.page.next_frame(h.snaps.now).unwrap();
    assert!(at - h.snaps.now < 0.1, "the swap animates at frame rate");
    h.save("page-media-track-swap");
    h.advance(0.5);
    h.idle();
    h.advance(0.5);
    h.idle();
    // Back to the once-a-second timeline tick.
    let at = h.page.next_frame(h.snaps.now).unwrap();
    assert!(at - h.snaps.now > 0.1);
}

#[test]
fn snapshots_of_the_media_page() {
    let Some(mut h) = Harness::new("media") else {
        return;
    };
    playing(&mut h);
    audio(&mut h);
    sources(&mut h);
    h.idle();
    h.save("page-media-playing");
    set(&mut h, |s| {
        s.bold_media_title = true;
        s.live_activity = false;
    });
    h.width = 280.0;
    h.idle();
    h.save("page-media-narrow");
    h.set_scale(1.5);
    h.width = 340.0;
    h.idle();
    h.save("page-media-scale-150");
}

#[test]
fn the_volume_page_hides_behind_media_and_stands_alone_without_it() {
    let Some(mut h) = Harness::new("volume") else {
        return;
    };
    audio(&mut h);
    // Media and volume both on: the media page carries the footer; this page cancels the gap.
    assert!(h.height() < 0.0);
    h.idle();
    assert_eq!(h.ink(), 0);
    set(&mut h, |s| {
        s.widgets.retain(|w| w.id != "media");
    });
    assert!(h.height() >= 144.0 - 1.0);
    h.idle();
    assert!(h.ink() > 400);
    h.save("page-volume");
    h.click_id(LEVEL);
    h.advance(0.3);
    h.idle();
    h.commands();
    h.save("page-volume-popover");
    h.click_id(MUTE);
    assert_eq!(
        h.commands(),
        vec![Command::ToggleMute, Command::RefreshAudio]
    );
}
