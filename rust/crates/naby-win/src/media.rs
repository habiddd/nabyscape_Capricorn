//! System media sessions (`MediaService` in `Application/Services.cs`) over
//! `GlobalSystemMediaTransportControlsSessionManager`.
//!
//! A worker thread in the multithreaded apartment owns every WinRT object. Updates are
//! event driven (sessions, properties, playback, timeline); between events the position is
//! extrapolated from the snapshot time by [`MediaService::timeline`]. Disabling the service
//! drops the manager and the artwork so a media-less dock holds no session memory.
#![allow(dead_code)] // consumed by the app shell, which is wired separately

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use naby_core::media::policy::allow_automatic;
use naby_core::media::source::AUTO_ID;
use naby_core::media::{MediaSnapshot, MediaSourceSelection};
use windows::core::{Interface, HSTRING};
use windows::ApplicationModel::AppInfo;
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Storage::Streams::DataReader;

use crate::audio_logic::session_name_fallback;
use crate::devices::ComApartment;
use crate::media_logic::{
    artwork_acceptable, artwork_key, display_artist, display_title, extrapolate, idle_artist,
    now_filetime_ticks, read_timeline, seek_ticks, RawTimeline, CONTROLS_UNAVAILABLE,
    CONTROL_FAILED, CONTROL_UNSUPPORTED, SEEK_UNSUPPORTED, SESSION_UNAVAILABLE,
};
use crate::wake::Waker;

/// One entry of the session list shown in the source picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSession {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
}

enum Msg {
    SessionsChanged,
    CurrentChanged,
    Properties(u64),
    Playback(u64),
    Timeline(u64),
    SetEnabled(bool),
    Select(Option<String>),
    Command(Command),
    Seek(f64),
    Shutdown,
}

struct Shared {
    snapshot: MediaSnapshot,
    sessions: Vec<MediaSession>,
    selection: String,
    /// `(elapsed, duration)` seconds at `stamp`.
    sample: (f64, f64),
    stamp: Instant,
}

pub struct MediaService {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl MediaService {
    /// Starts enabled; the first session snapshot arrives through `wake`.
    pub fn start(wake: Waker) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            snapshot: MediaSnapshot::default(),
            sessions: Vec::new(),
            selection: AUTO_ID.to_owned(),
            sample: (0.0, 0.0),
            stamp: Instant::now(),
        }));
        let (tx, rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let worker_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("naby-media".into())
            .spawn(move || worker(rx, worker_tx, worker_shared, wake))
            .ok();
        MediaService { shared, tx, thread }
    }

    pub fn snapshot(&self) -> MediaSnapshot {
        lock(&self.shared).snapshot.clone()
    }

    /// `(elapsed, duration)` seconds now: the last reported position advanced by wall time
    /// while playing (`MediaService.Timeline`).
    pub fn timeline(&self) -> (f64, f64) {
        let s = lock(&self.shared);
        extrapolate(
            s.sample,
            s.snapshot.playing,
            s.stamp.elapsed().as_secs_f64(),
        )
    }

    /// Sessions the picker offers.
    pub fn sessions(&self) -> Vec<MediaSession> {
        lock(&self.shared).sessions.clone()
    }

    /// `@auto`, `@none` or a fixed app id.
    pub fn source_selection_id(&self) -> String {
        lock(&self.shared).selection.clone()
    }

    /// Automatic previews (hover peek) are suppressed for browsers and opaque ids when
    /// browser previews are off (`MediaPreviewPolicy.AllowAutomatic`).
    pub fn automatic_preview_allowed(&self, browser_media_previews: bool) -> bool {
        let s = lock(&self.shared);
        let id = (!s.snapshot.source_id.is_empty()).then_some(s.snapshot.source_id.as_str());
        allow_automatic(browser_media_previews, id)
    }

    /// `false` drops the manager, sessions and artwork; `true` reconnects.
    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.tx.send(Msg::SetEnabled(enabled));
    }

    /// Fixes the source to one app id, or `None` for "no source".
    pub fn select_source(&self, id: Option<&str>) {
        let _ = self
            .tx
            .send(Msg::Select(Some(id.unwrap_or("@none").to_owned())));
    }

    pub fn select_automatic(&self) {
        let _ = self.tx.send(Msg::Select(None));
    }

    pub fn play(&self) {
        let _ = self.tx.send(Msg::Command(Command::Play));
    }
    pub fn pause(&self) {
        let _ = self.tx.send(Msg::Command(Command::Pause));
    }
    pub fn toggle(&self) {
        let _ = self.tx.send(Msg::Command(Command::Toggle));
    }
    pub fn next(&self) {
        let _ = self.tx.send(Msg::Command(Command::Next));
    }
    pub fn previous(&self) {
        let _ = self.tx.send(Msg::Command(Command::Previous));
    }
    /// Seeks to `fraction` (0..=1) of the track.
    pub fn seek(&self, fraction: f64) {
        let _ = self.tx.send(Msg::Seek(fraction));
    }
}

impl Drop for MediaService {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Selected {
    session: Session,
    id: String,
    reg: u64,
    tokens: [i64; 3],
}

struct Worker {
    tx: Sender<Msg>,
    shared: Arc<Mutex<Shared>>,
    wake: Waker,
    enabled: bool,
    manager: Option<(Manager, [i64; 2])>,
    sessions: Vec<Session>,
    selected: Option<Selected>,
    selection: MediaSourceSelection,
    snap: MediaSnapshot,
    sample: (f64, f64),
    art_key: String,
    registrations: u64,
    names: HashMap<String, String>,
}

fn worker(rx: Receiver<Msg>, tx: Sender<Msg>, shared: Arc<Mutex<Shared>>, wake: Waker) {
    let _com = ComApartment::mta();
    let mut w = Worker {
        tx,
        shared,
        wake,
        enabled: true,
        manager: None,
        sessions: Vec::new(),
        selected: None,
        selection: MediaSourceSelection::new(),
        snap: MediaSnapshot::default(),
        sample: (0.0, 0.0),
        art_key: String::new(),
        registrations: 0,
        names: HashMap::new(),
    };
    w.connect();
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Shutdown => break,
            Msg::SessionsChanged => w.reload(),
            Msg::CurrentChanged => w.apply_selection(w.selected.as_ref().map(|s| s.id.clone())),
            Msg::Properties(reg) => {
                if w.is_current(reg) {
                    w.refresh();
                }
            }
            Msg::Playback(reg) => {
                if w.is_current(reg) {
                    w.read_playback();
                    w.read_timeline();
                    w.publish();
                }
            }
            Msg::Timeline(reg) => {
                if w.is_current(reg) {
                    w.read_timeline();
                    w.publish();
                }
            }
            Msg::SetEnabled(on) => w.set_enabled(on),
            Msg::Select(choice) => {
                w.selection
                    .select(Some(choice.as_deref().unwrap_or(AUTO_ID)));
                if w.enabled {
                    w.apply_selection(w.selected.as_ref().map(|s| s.id.clone()));
                }
            }
            Msg::Command(cmd) => w.command(cmd),
            Msg::Seek(f) => w.seek(f),
        }
    }
    w.disconnect();
}

impl Worker {
    fn is_current(&self, reg: u64) -> bool {
        self.enabled && self.selected.as_ref().is_some_and(|s| s.reg == reg)
    }

    fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        self.enabled = on;
        if on {
            self.connect();
        } else {
            self.disconnect();
            self.art_key.clear();
            self.sample = (0.0, 0.0);
            self.snap = MediaSnapshot::default();
            self.publish();
        }
    }

    fn connect(&mut self) {
        if !self.enabled || self.manager.is_some() {
            return;
        }
        let result = (|| -> windows::core::Result<(Manager, [i64; 2])> {
            let manager = Manager::RequestAsync()?.get()?;
            let tx = self.tx.clone();
            let sessions = manager.SessionsChanged(&TypedEventHandler::new(move |_, _| {
                let _ = tx.send(Msg::SessionsChanged);
                Ok(())
            }))?;
            let tx = self.tx.clone();
            let current = manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                let _ = tx.send(Msg::CurrentChanged);
                Ok(())
            }))?;
            Ok((manager, [sessions, current]))
        })();
        match result {
            Ok(m) => {
                self.manager = Some(m);
                self.reload();
            }
            Err(_) => {
                self.snap = MediaSnapshot {
                    artist: CONTROLS_UNAVAILABLE.to_owned(),
                    ..MediaSnapshot::default()
                };
                self.publish();
            }
        }
    }

    /// Unhooks every event and releases the manager and sessions.
    fn disconnect(&mut self) {
        self.unselect();
        self.sessions.clear();
        if let Some((manager, [a, b])) = self.manager.take() {
            let _ = manager.RemoveSessionsChanged(a);
            let _ = manager.RemoveCurrentSessionChanged(b);
        }
    }

    fn unselect(&mut self) {
        if let Some(sel) = self.selected.take() {
            let _ = sel.session.RemoveMediaPropertiesChanged(sel.tokens[0]);
            let _ = sel.session.RemovePlaybackInfoChanged(sel.tokens[1]);
            let _ = sel.session.RemoveTimelinePropertiesChanged(sel.tokens[2]);
        }
    }

    fn reload(&mut self) {
        let Some((manager, _)) = &self.manager else {
            return;
        };
        self.sessions = manager
            .GetSessions()
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        self.apply_selection(self.selected.as_ref().map(|s| s.id.clone()));
    }

    fn session_id(session: &Session) -> String {
        session
            .SourceAppUserModelId()
            .map(|h| h.to_string())
            .unwrap_or_default()
    }

    fn display_name(&mut self, id: &str) -> String {
        if let Some(n) = self.names.get(id) {
            return n.clone();
        }
        let name = AppInfo::GetFromAppUserModelId(&HSTRING::from(id))
            .and_then(|a| a.DisplayInfo())
            .and_then(|d| d.DisplayName())
            .map(|n| n.to_string())
            .ok()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| session_name_fallback(id));
        if self.names.len() >= 64 {
            self.names.clear();
        }
        self.names.insert(id.to_owned(), name.clone());
        name
    }

    fn apply_selection(&mut self, previous: Option<String>) {
        if !self.enabled {
            return;
        }
        let ids: Vec<String> = self.sessions.iter().map(Self::session_id).collect();
        let current = self
            .manager
            .as_ref()
            .and_then(|(m, _)| m.GetCurrentSession().ok())
            .map(|s| Self::session_id(&s));
        let chosen = self
            .selection
            .resolve(&ids, current.as_deref(), previous.as_deref());
        let session = chosen.and_then(|id| {
            self.sessions
                .iter()
                .find(|s| Self::session_id(s) == id)
                .cloned()
        });
        self.select(session);
    }

    fn select(&mut self, session: Option<Session>) {
        let same = match (&self.selected, &session) {
            (Some(sel), Some(new)) => sel.session.as_raw() == new.as_raw(),
            (None, None) => true,
            _ => false,
        };
        if same {
            self.refresh();
            return;
        }
        self.unselect();
        self.sample = (0.0, 0.0);
        if let Some(session) = session {
            self.registrations += 1;
            let reg = self.registrations;
            let tx = self.tx.clone();
            let props = session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                let _ = tx.send(Msg::Properties(reg));
                Ok(())
            }));
            let tx = self.tx.clone();
            let playback = session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                let _ = tx.send(Msg::Playback(reg));
                Ok(())
            }));
            let tx = self.tx.clone();
            let timeline =
                session.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                    let _ = tx.send(Msg::Timeline(reg));
                    Ok(())
                }));
            let id = Self::session_id(&session);
            self.selected = Some(Selected {
                session,
                id,
                reg,
                tokens: [
                    props.unwrap_or(0),
                    playback.unwrap_or(0),
                    timeline.unwrap_or(0),
                ],
            });
        }
        self.refresh();
    }

    fn idle(&mut self) {
        self.art_key.clear();
        self.sample = (0.0, 0.0);
        self.snap = MediaSnapshot {
            artist: idle_artist(self.selection.is_none()).to_owned(),
            ..MediaSnapshot::default()
        };
        self.publish();
    }

    /// Re-reads title, artist, artwork, controls and timeline of the selected session.
    fn refresh(&mut self) {
        let Some(sel) = &self.selected else {
            self.idle();
            return;
        };
        let session = sel.session.clone();
        let id = sel.id.clone();
        let result = (|| -> windows::core::Result<()> {
            let props = session.TryGetMediaPropertiesAsync()?.get()?;
            let title = props.Title()?.to_string();
            let artist = props.Artist()?.to_string();
            let album = props.AlbumTitle()?.to_string();
            let key = artwork_key(&id, &title, &artist, &album);
            let artwork = if key == self.art_key {
                self.snap.artwork.clone()
            } else {
                props.Thumbnail().ok().and_then(read_thumbnail)
            };
            self.art_key = key;
            let name = self.display_name(&id);
            self.snap.available = true;
            self.snap.source_id = id.clone();
            self.snap.source_name = name;
            self.snap.title = display_title(&title);
            self.snap.artist = display_artist(&artist, &id);
            self.snap.album = album;
            self.snap.artwork = artwork;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.read_playback();
                self.read_timeline();
            }
            Err(_) => {
                self.snap.available = true;
                self.snap.artist = SESSION_UNAVAILABLE.to_owned();
                self.snap.playing = false;
                self.snap.can_toggle = false;
                self.snap.can_next = false;
                self.snap.can_previous = false;
                self.snap.can_seek = false;
            }
        }
        self.publish();
    }

    fn read_playback(&mut self) {
        let Some(sel) = &self.selected else { return };
        let info = sel.session.GetPlaybackInfo().and_then(|p| {
            let controls = p.Controls()?;
            Ok((
                p.PlaybackStatus()? == PlaybackStatus::Playing,
                controls.IsPlayPauseToggleEnabled()?,
                controls.IsNextEnabled()?,
                controls.IsPreviousEnabled()?,
                controls.IsPlaybackPositionEnabled()?,
            ))
        });
        match info {
            Ok((playing, toggle, next, previous, seek)) => {
                self.snap.playing = playing;
                self.snap.can_toggle = toggle;
                self.snap.can_next = next;
                self.snap.can_previous = previous;
                self.snap.can_seek = seek;
            }
            Err(_) => {
                self.snap.playing = false;
                self.snap.can_toggle = false;
                self.snap.can_next = false;
                self.snap.can_previous = false;
                self.snap.can_seek = false;
            }
        }
    }

    fn read_timeline(&mut self) {
        let sample = if self.selection.is_none() {
            (0.0, 0.0)
        } else if let Some(sel) = &self.selected {
            sel.session
                .GetTimelineProperties()
                .and_then(|t| {
                    Ok(RawTimeline {
                        start: t.StartTime()?.Duration,
                        end: t.EndTime()?.Duration,
                        position: t.Position()?.Duration,
                        last_updated: t.LastUpdatedTime()?.UniversalTime,
                    })
                })
                .map(|raw| read_timeline(raw, self.snap.playing, now_filetime_ticks()))
                .unwrap_or((0.0, 0.0))
        } else {
            (0.0, 0.0)
        };
        self.sample = sample;
        self.snap.position = Duration::from_secs_f64(sample.0.max(0.0));
        self.snap.duration = Duration::from_secs_f64(sample.1.max(0.0));
    }

    fn command(&mut self, cmd: Command) {
        let Some(sel) = &self.selected else { return };
        let session = sel.session.clone();
        let supported = match cmd {
            Command::Toggle => self.snap.can_toggle,
            Command::Next => self.snap.can_next,
            Command::Previous => self.snap.can_previous,
            Command::Play | Command::Pause => session
                .GetPlaybackInfo()
                .and_then(|p| p.Controls())
                .and_then(|c| {
                    if cmd == Command::Play {
                        c.IsPlayEnabled()
                    } else {
                        c.IsPauseEnabled()
                    }
                })
                .unwrap_or(false),
        };
        let outcome = (|| -> windows::core::Result<bool> {
            if !supported {
                return Ok(false);
            }
            match cmd {
                Command::Play => session.TryPlayAsync()?.get(),
                Command::Pause => session.TryPauseAsync()?.get(),
                Command::Toggle => session.TryTogglePlayPauseAsync()?.get(),
                Command::Next => session.TrySkipNextAsync()?.get(),
                Command::Previous => session.TrySkipPreviousAsync()?.get(),
            }
        })();
        match outcome {
            Ok(true) => {}
            Ok(false) => self.snap.artist = CONTROL_UNSUPPORTED.to_owned(),
            Err(_) => self.snap.artist = CONTROL_FAILED.to_owned(),
        }
        self.publish();
    }

    fn seek(&mut self, fraction: f64) {
        let Some(sel) = &self.selected else { return };
        if !self.snap.can_seek {
            return;
        }
        let session = sel.session.clone();
        let done = (|| -> windows::core::Result<Option<bool>> {
            let t = session.GetTimelineProperties()?;
            let Some(ticks) = seek_ticks(t.StartTime()?.Duration, t.EndTime()?.Duration, fraction)
            else {
                return Ok(None);
            };
            Ok(Some(session.TryChangePlaybackPositionAsync(ticks)?.get()?))
        })();
        if let Ok(Some(false)) = done {
            self.snap.artist = SEEK_UNSUPPORTED.to_owned();
        }
        self.read_timeline();
        self.publish();
    }

    fn publish(&mut self) {
        let sessions: Vec<MediaSession> = {
            let list = self.sessions.clone();
            list.iter()
                .map(|s| {
                    let id = Self::session_id(s);
                    let name = self.display_name(&id);
                    MediaSession { id, name }
                })
                .collect()
        };
        if self.selection.is_none() {
            // "None" never reports a live session.
            self.snap.available = false;
        }
        let changed = {
            let mut s = lock(&self.shared);
            let changed = s.snapshot != self.snap
                || s.sessions != sessions
                || s.selection != self.selection.id();
            s.sample = self.sample;
            s.stamp = Instant::now();
            if changed {
                s.snapshot = self.snap.clone();
                s.sessions = sessions;
                s.selection = self.selection.id().to_owned();
            }
            changed
        };
        if changed {
            (self.wake)();
        }
    }
}

/// Reads an encoded thumbnail into memory, dropping empty or oversized ones.
fn read_thumbnail(
    reference: windows::Storage::Streams::IRandomAccessStreamReference,
) -> Option<Arc<[u8]>> {
    let stream = reference.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?;
    if !artwork_acceptable(size) {
        let _ = stream.Close();
        return None;
    }
    let bytes = (|| -> windows::core::Result<Vec<u8>> {
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        reader.LoadAsync(size as u32)?.get()?;
        let mut buf = vec![0u8; size as usize];
        reader.ReadBytes(&mut buf)?;
        let _ = reader.Close();
        Ok(buf)
    })()
    .ok();
    let _ = stream.Close();
    bytes.map(Arc::from)
}
