//! The application shell: process setup, single instance, the Win32 message loop, the hidden
//! host window that owns the tray icon / hotkeys / `WM_APP_WAKE`, and the seam to whatever is
//! painted in the dock window (`Surface`).
//!
//! Window procedures only translate messages into `HostEvent`s; this loop applies them, so
//! application state is never borrowed re-entrantly from inside Win32 callbacks.

use crate::overlay_host::ShellAction;
use naby_core::settings::{presets, Settings};
use naby_core::store::Store;
use naby_render::{Canvas, Fonts, InputState};
use naby_ui::host::{DisplayInfo, Signal};
use std::time::Instant;

/// What the dock window shows. `OverlayHost` implements it; every method but the first five
/// has a do-nothing default so test doubles stay small.
///
/// Frame protocol: the shell calls `tick()` (advance time), then reads `size()` / `placement()`,
/// paints, presents, and arms one timer for `next_frame()` (or none when it is `None`).
pub trait Surface {
    /// Draws the whole surface. `canvas` has exactly `size()` pixels and is cleared to
    /// transparent by the caller.
    fn paint(&mut self, canvas: &mut Canvas, fonts: &Fonts);
    /// Handles one frame of pointer / keyboard input; true when a repaint is needed.
    fn input(&mut self, input: &InputState) -> bool;
    /// Pixel size of the window the surface wants (already DPI scaled); `(0, 0)` while hidden,
    /// which makes the shell release the window and every pixel buffer.
    fn size(&self) -> (u32, u32);
    /// When the surface wants to be painted next while animating or counting down; `None`
    /// when static (no timer is armed then).
    fn next_frame(&self) -> Option<Instant>;
    /// The monitor DPI scale (1.0 = 96 dpi) changed.
    fn set_scale(&mut self, _scale: f32) {}
    /// New settings are in effect.
    fn apply_settings(&mut self, _settings: &Settings) {}
    /// A service called the `Waker`; pull fresh snapshots. True when a repaint is needed.
    fn wake(&mut self) -> bool {
        false
    }
    /// The frame timer fired: advance animations and timers. True when a repaint is needed.
    fn tick(&mut self) -> bool {
        false
    }
    /// Top-left of the window in screen pixels, `None` to let the shell place it.
    fn placement(&self) -> Option<(i32, i32)> {
        None
    }
    /// The monitor the dock lives on changed (bounds, work area, DPI).
    fn set_display(&mut self, _display: DisplayInfo) {}
    /// A hotkey, tray command or environment change. True when a repaint is needed.
    fn signal(&mut self, _signal: Signal) -> bool {
        false
    }
    /// The window should take keyboard focus (panel open).
    fn wants_focus(&self) -> bool {
        false
    }
    /// Install the outside-click hook while a panel or peek is open.
    fn wants_outside_clicks(&self) -> bool {
        false
    }
    /// A drag or edit is in progress (the dock must not follow the pointer to another monitor).
    fn interaction_active(&self) -> bool {
        false
    }
    /// The window was hidden: drop caches.
    fn on_hide(&mut self) {}
    /// The live snapshots the auxiliary windows (Settings) read, when the surface has them.
    fn snapshots(&self) -> Option<naby_ui::host::Snapshots> {
        None
    }
    /// Runs a command another window produced (media, timers, notices, ...). True = repaint.
    fn run_command(&mut self, _command: naby_core::command::Command) -> bool {
        false
    }
    /// Things only the app shell can do (settings window, tray balloons, quit).
    fn take_actions(&mut self) -> Vec<ShellAction> {
        Vec::new()
    }
}

/// Command line of the app (the C# modes that belong to other tasks are not parsed here).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// `--exit`: ask the running instance to quit, then exit.
    pub exit: bool,
    /// `--background`: started at logon; skip the "ready" notification.
    pub background: bool,
    /// `--settings`: open Settings after startup.
    pub settings: bool,
}

impl Args {
    pub fn parse<I, S>(args: I) -> Args
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut parsed = Args::default();
        for arg in args {
            match arg.as_ref() {
                "--exit" => parsed.exit = true,
                "--background" => parsed.background = true,
                "--settings" => parsed.settings = true,
                _ => {}
            }
        }
        parsed
    }
}

/// Loads `settings.json` as `App.OnStartup` does. A missing file yields the recommended
/// preset; an unreadable one yields the recommended preset with clipboard history and
/// download watching off (so nothing sensitive turns on while the user's file is being
/// recovered). The second value is the store's recovery warning to surface.
pub fn load_settings(store: &Store) -> (Settings, Option<String>) {
    match store.load::<Settings>("settings") {
        Ok(Some(mut settings)) => {
            settings.normalize();
            (settings, store.warning())
        }
        Ok(None) => (presets::recommended(), store.warning()),
        Err(error) => {
            let mut settings = presets::recommended();
            settings.clipboard_history = false;
            settings.downloads_enabled = false;
            let warning = store.warning().or_else(|| Some(error.to_string()));
            (settings, warning)
        }
    }
}

#[cfg(windows)]
pub use os::{App, HostHandle};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::event::{AuxWindow, HostEvent, WM_APP_TRAY, WM_APP_WAKE};
    use crate::fullscreen::{foreground_should_hide, ForegroundHooks};
    use crate::hotkeys::{HotkeyAction, Hotkeys};
    use crate::instance::{self, Acquire, Instance};
    use crate::monitors::{
        choose_monitor, cursor_pos, enumerate, monitor_at, refresh_rate, FollowPolicy, MonitorInfo,
    };
    use crate::outside_click::OutsideClickHook;
    use crate::overlay_host::{display_info, OverlayHost};
    use crate::settings_host::{self, AuxContent, SettingsHost, SettingsOutput};
    use crate::startup;
    use crate::timer::AnimationTimer;
    use crate::tray::{Tray, TrayCommand, WakeMenu, TOOLTIP};
    use crate::wake::Waker;
    use crate::window::{ensure_class, set_user_data, user_data, LayeredWindow};
    use naby_core::color::Rgba;
    use naby_core::command::Command;
    use naby_core::settings::DisplayMode;
    use naby_ui::host::Signal;
    use naby_ui::settings::bounds::Bounds;
    use naby_ui::settings::SettingsView;
    use naby_ui::welcome::{WelcomeDone, WelcomeView};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Arc;
    use std::time::Duration;
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WAIT_FAILED, WPARAM};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::INFINITE;
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        MsgWaitForMultipleObjectsEx, PeekMessageW, PostMessageW, RegisterWindowMessageW,
        TranslateMessage, MSG, MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_CONTEXTMENU, WM_DESTROY, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_HOTKEY,
        WM_LBUTTONDBLCLK, WM_QUERYENDSESSION, WM_SETTINGCHANGE, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    const HOST_CLASS: windows::core::PCWSTR = w!("NabyScape.Host");
    /// `SPI_SETWORKAREA`: the taskbar moved or resized.
    const SPI_SETWORKAREA: usize = 0x002F;
    /// How often `Follow` mode looks at the pointer (C# `pointerCheck`).
    const FOLLOW_POLL: Duration = Duration::from_millis(100);

    struct HostShared {
        tx: Sender<HostEvent>,
        wake_pending: Arc<AtomicBool>,
        taskbar_created: u32,
    }

    unsafe extern "system" fn host_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let ptr = user_data(hwnd) as *const HostShared;
        if ptr.is_null() {
            // SAFETY: default handling before the shared state is attached.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
        // SAFETY: set by `App::new` from a leaked Box and cleared before it is freed.
        let s = unsafe { &*ptr };
        let send = |event| {
            let _ = s.tx.send(event);
        };
        match msg {
            WM_APP_WAKE => {
                s.wake_pending.store(false, Ordering::Release);
                send(HostEvent::Wake);
                LRESULT(0)
            }
            WM_APP_TRAY => {
                match (lparam.0 & 0xFFFF) as u32 {
                    WM_CONTEXTMENU => send(HostEvent::TrayMenu),
                    WM_LBUTTONDBLCLK => send(HostEvent::TrayActivated),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_HOTKEY => {
                if let Some(action) = HotkeyAction::from_id(wparam.0 as i32) {
                    send(HostEvent::Hotkey(action));
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                send(HostEvent::DisplayChanged);
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                if wparam.0 == SPI_SETWORKAREA {
                    send(HostEvent::DisplayChanged);
                }
                // SAFETY: default handling.
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_QUERYENDSESSION => LRESULT(1),
            WM_ENDSESSION => {
                if wparam.0 != 0 {
                    send(HostEvent::Quit);
                }
                LRESULT(0)
            }
            m if m == s.taskbar_created && m != 0 => {
                send(HostEvent::TaskbarCreated);
                LRESULT(0)
            }
            WM_DESTROY => LRESULT(0),
            // SAFETY: default handling for everything else.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    /// A `Waker` that posts `WM_APP_WAKE` to `hwnd`, coalescing bursts into one pending message.
    fn make_waker(hwnd: HWND, pending: Arc<AtomicBool>) -> Waker {
        let hwnd = hwnd.0 as isize;
        Arc::new(move || {
            if !pending.swap(true, Ordering::AcqRel) {
                // SAFETY: PostMessage is thread-safe; a destroyed window just fails.
                let posted = unsafe {
                    PostMessageW(
                        Some(HWND(hwnd as *mut _)),
                        WM_APP_WAKE,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
                if posted.is_err() {
                    pending.store(false, Ordering::Release);
                }
            }
        })
    }

    /// What services need to talk to the UI thread: post events and wake the loop.
    #[derive(Clone)]
    pub struct HostHandle {
        pub events: Sender<HostEvent>,
        pub waker: Waker,
    }

    struct ProcessGuard;
    impl Drop for ProcessGuard {
        fn drop(&mut self) {
            // SAFETY: balances the CoInitializeEx in `App::run`.
            unsafe { CoUninitialize() };
        }
    }

    pub struct App {
        args: Args,
        rx: Receiver<HostEvent>,
        tx: Sender<HostEvent>,
        wake_pending: Arc<AtomicBool>,
        host: HWND,
        host_shared: *mut HostShared,
        instance: Instance,
        store: Arc<Store>,
        settings: Settings,
        fonts: Fonts,
        canvas: Canvas,
        window: LayeredWindow,
        surface: Box<dyn Surface>,
        tray: Tray,
        hotkeys: Hotkeys,
        timer: AnimationTimer,
        hooks: Option<ForegroundHooks>,
        outside: Option<OutsideClickHook>,
        monitors: Vec<MonitorInfo>,
        follow: FollowPolicy,
        followed: Option<String>,
        follow_clock: Instant,
        follow_due: Instant,
        /// Where the dock window is on screen (outside-click hit testing).
        window_origin: (i32, i32),
        quit: bool,
        settings_window: Option<SettingsHost>,
        welcome_window: Option<SettingsHost>,
    }

    impl App {
        /// Runs the whole application and returns the process exit code.
        pub fn run() -> i32 {
            let args = Args::parse(std::env::args().skip(1));
            if args.exit {
                instance::signal_exit();
                return 0;
            }
            // SAFETY: process-wide setup before any window exists. The manifest already asks for
            // PerMonitorV2; this covers launchers that strip it (failure is harmless).
            unsafe {
                let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            }
            let instance = match Instance::acquire() {
                Ok(Acquire::First(instance)) => instance,
                Ok(Acquire::AlreadyRunning) => {
                    instance::signal_open();
                    return 0;
                }
                Err(_) => return 1,
            };
            // SAFETY: apartment-threaded COM on the UI thread, released by the guard.
            let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            let _com_guard = com.is_ok().then_some(ProcessGuard);
            let mut app = match App::new(args, instance) {
                Ok(app) => app,
                Err(error) => {
                    Store::open_default().log(&format!("startup failed: {error}"));
                    return 1;
                }
            };
            app.message_loop()
        }

        fn new(args: Args, instance: Instance) -> windows::core::Result<App> {
            let store = Arc::new(Store::open_default());
            let existing_profile = store.dir().join("settings.json").exists();
            let (mut settings, warning) = load_settings(&store);
            settings.auto_start = startup::enabled();

            let (tx, rx) = channel();
            let wake_pending = Arc::new(AtomicBool::new(false));
            ensure_class(HOST_CLASS, Some(host_proc), false);
            // SAFETY: creates a hidden, never-shown top-level window (it must be top-level to
            // receive `TaskbarCreated` and display-change broadcasts); the shared pointer is
            // attached right after and freed in Drop.
            let (host, host_shared) = unsafe {
                let instance_handle = GetModuleHandleW(None)?;
                let host = CreateWindowExW(
                    WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
                    HOST_CLASS,
                    w!("NabyScape Host"),
                    WINDOW_STYLE(WS_POPUP.0),
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    Some(instance_handle.into()),
                    None,
                )?;
                let shared = Box::into_raw(Box::new(HostShared {
                    tx: tx.clone(),
                    wake_pending: wake_pending.clone(),
                    taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
                }));
                set_user_data(host, shared as isize);
                (host, shared)
            };

            let monitors = enumerate();
            let display = pick_monitor(&settings, &monitors, None)
                .map(|i| display_info(&monitors[i], refresh_rate(&monitors[i].id)))
                .unwrap_or_default();
            let waker = make_waker(host, wake_pending.clone());
            let mut overlay = OverlayHost::new(store.clone(), waker, settings.clone(), display);
            let window = LayeredWindow::new(tx.clone())?;
            overlay.attach_window(window.hwnd());
            let surface: Box<dyn Surface> = Box::new(overlay);
            let tray = Tray::new(host, TOOLTIP);
            let timer = AnimationTimer::new()?;

            let mut app = App {
                args,
                rx,
                tx,
                wake_pending,
                host,
                host_shared,
                instance,
                store,
                settings,
                fonts: Fonts::load_system(),
                canvas: Canvas::new(1, 1),
                window,
                surface,
                tray,
                hotkeys: Hotkeys::new(),
                timer,
                hooks: None,
                outside: None,
                monitors,
                follow: FollowPolicy::default(),
                followed: None,
                follow_clock: Instant::now(),
                follow_due: Instant::now(),
                window_origin: (0, 0),
                quit: false,
                settings_window: None,
                welcome_window: None,
            };
            app.canvas.release();
            app.apply_settings();
            if let Some(warning) = warning {
                app.notify("Settings recovered", &warning);
            }
            if !app.args.background {
                app.notify(
                    "NabyScape is ready",
                    "Click the island for your widgets. Right-click the tray icon for settings.",
                );
            }
            if naby_core::settings::welcome::should_offer_stored(&app.store, existing_profile) {
                app.open_welcome();
            }
            if app.args.settings {
                app.open_settings(None);
            }
            Ok(app)
        }

        /// Handle for background services: an event sender plus the shared `Waker`.
        pub fn handle(&self) -> HostHandle {
            HostHandle {
                events: self.tx.clone(),
                waker: self.waker(),
            }
        }

        /// A `Waker` that posts `WM_APP_WAKE` to the host window.
        pub fn waker(&self) -> Waker {
            make_waker(self.host, self.wake_pending.clone())
        }

        /// Shows a tray balloon (the shell-level notification path).
        pub fn notify(&self, title: &str, detail: &str) {
            self.tray.balloon(title, detail);
        }

        /// The work area and scale of the monitor the pointer is on (where windows open).
        fn work_near_cursor(&self) -> (Bounds, f64) {
            let m = cursor_pos()
                .and_then(|(x, y)| monitor_at(&self.monitors, x, y))
                .or_else(|| self.monitor_index())
                .and_then(|i| self.monitors.get(i));
            match m {
                Some(m) => (
                    Bounds::new(
                        f64::from(m.work.left),
                        f64::from(m.work.top),
                        f64::from(m.work.width()),
                        f64::from(m.work.height()),
                    ),
                    f64::from(m.scale()),
                ),
                None => (Bounds::new(0.0, 0.0, 1280.0, 720.0), 1.0),
            }
        }

        fn host_info(&self) -> naby_ui::settings::HostInfo {
            let dock = self.monitor_index().and_then(|i| self.monitors.get(i));
            settings_host::host_info(
                &self.settings,
                &self.monitors,
                self.hotkeys.status_lines(&self.settings),
                &self.store.dir().to_string_lossy(),
                dock,
            )
        }

        /// Opens Settings (or raises it) at an optional page.
        fn open_settings(&mut self, page: Option<String>) {
            if let Some(window) = &mut self.settings_window {
                if let (Some(p), Some(view)) = (&page, window.settings_view()) {
                    view.open_page(p);
                }
                window.raise();
                let _ = self.tx.send(HostEvent::AuxInput(AuxWindow::Settings));
                return;
            }
            let mut view = SettingsView::new(self.settings.clone(), Some(self.store.clone()));
            if let Some(p) = &page {
                view.open_page(p);
            }
            let (work, scale) = self.work_near_cursor();
            let info = self.host_info();
            self.settings_window = SettingsHost::open(
                AuxWindow::Settings,
                AuxContent::Settings(Box::new(view)),
                self.store.clone(),
                self.tx.clone(),
                work,
                scale,
            );
            if let Some(window) = &mut self.settings_window {
                window.set_info(info);
                window.raise();
            }
        }

        /// Opens the first-run guide (or raises it).
        fn open_welcome(&mut self) {
            if let Some(window) = &self.welcome_window {
                window.raise();
                return;
            }
            let view = WelcomeView::new(self.settings.clone(), Some(self.store.clone()));
            let (work, scale) = self.work_near_cursor();
            self.welcome_window = SettingsHost::open(
                AuxWindow::Welcome,
                AuxContent::Welcome(Box::new(view)),
                self.store.clone(),
                self.tx.clone(),
                work,
                scale,
            );
            if let Some(window) = &self.welcome_window {
                window.raise();
            }
        }

        /// Runs a frame of an auxiliary window and carries out what it asked for.
        fn pump_aux(&mut self, which: AuxWindow) {
            let snaps = self.surface.snapshots();
            let info = self.host_info();
            let outputs = {
                let slot = match which {
                    AuxWindow::Settings => &mut self.settings_window,
                    AuxWindow::Welcome => &mut self.welcome_window,
                };
                let Some(window) = slot else { return };
                window.set_info(info);
                window.pump(snaps)
            };
            for output in outputs {
                match output {
                    SettingsOutput::Apply(settings) => {
                        let mut next = *settings;
                        next.normalize();
                        let ok = match self.store.save_settings(&next) {
                            Ok(()) => true,
                            Err(error) => {
                                self.store.log(&error);
                                false
                            }
                        };
                        self.settings = next;
                        self.apply_settings();
                        if let Some(window) = &mut self.settings_window {
                            window.saved(ok);
                        }
                    }
                    SettingsOutput::WelcomeDone(done) => {
                        self.welcome_window = None;
                        // The guide saved its choices itself; reload them into the shell.
                        let (mut loaded, _) = load_settings(&self.store);
                        loaded.auto_start = self.settings.auto_start;
                        self.settings = loaded;
                        self.apply_settings();
                        if matches!(done, WelcomeDone::Finished { setup_ai: true }) {
                            self.notify(
                                "Assistant",
                                "The assistant is not part of this build of the new shell yet.",
                            );
                        }
                    }
                    SettingsOutput::Request(request) => self.settings_request(request),
                    SettingsOutput::Command(command) => self.aux_command(command),
                }
            }
        }

        fn settings_request(&mut self, request: naby_core::command::SettingsRequest) {
            use naby_core::command::SettingsRequest as R;
            match request {
                R::SetAutoStart(on) => {
                    if let Err(error) = startup::set(on) {
                        self.store.log(&error);
                        self.settings.auto_start = startup::enabled();
                        self.notify("Startup", &format!("Could not change startup: {error}"));
                        let _ = self.store.save_settings(&self.settings);
                    }
                }
                R::ShowWelcome => self.open_welcome(),
                R::Update(_) | R::OpenUninstaller => self.notify(
                    "Updates",
                    "Updates and uninstall are handled by the NabyScape setup in this build.",
                ),
                R::PreviewCompletionSound
                | R::PickFile(_)
                | R::PickInstalledApp
                | R::RefreshDiagnostics => {}
            }
        }

        fn aux_command(&mut self, command: Command) {
            match command {
                Command::Quit => self.quit = true,
                Command::OpenAssistant | Command::OpenVoice => self.notify(
                    "Assistant",
                    "The assistant is not part of this build of the new shell yet.",
                ),
                Command::OpenSettings => self.open_settings(None),
                Command::OpenSettingsPage(page) => self.open_settings(Some(page)),
                Command::CopyText(text) => {
                    let _ = crate::text_clipboard::set_text(None, &text);
                }
                other => {
                    let changed = self.surface.run_command(other);
                    self.after_surface(changed);
                }
            }
        }

        // ---- message loop -------------------------------------------------------------

        fn message_loop(&mut self) -> i32 {
            loop {
                let handles = [
                    self.timer.handle(),
                    self.instance.open_event(),
                    self.instance.exit_event(),
                ];
                // SAFETY: waits on handles owned by `self`; wakes for any queued message.
                let signalled = unsafe {
                    MsgWaitForMultipleObjectsEx(
                        Some(&handles),
                        INFINITE,
                        QS_ALLINPUT,
                        MWMO_INPUTAVAILABLE,
                    )
                };
                match signalled.0 {
                    0 => self.on_timer(),
                    1 => self.post(HostEvent::OpenSettings),
                    2 => self.post(HostEvent::Quit),
                    n if n == WAIT_FAILED.0 => return 1,
                    _ => {}
                }
                let mut msg = MSG::default();
                // SAFETY: standard message pump on the thread that owns the windows.
                while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                    // SAFETY: as above.
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                while let Ok(event) = self.rx.try_recv() {
                    self.handle_event(event);
                }
                if self.quit {
                    return 0;
                }
            }
        }

        fn post(&self, event: HostEvent) {
            let _ = self.tx.send(event);
        }

        fn handle_event(&mut self, event: HostEvent) {
            match event {
                HostEvent::Wake => {
                    let changed = self.surface.wake();
                    self.after_surface(changed);
                }
                HostEvent::Hotkey(action) => {
                    let signal = match action {
                        HotkeyAction::Panel => Signal::Panel,
                        HotkeyAction::Visibility => Signal::ToggleVisibility,
                        HotkeyAction::Tuck => Signal::ToggleTuck,
                    };
                    self.signal(signal);
                }
                HostEvent::Tray(command) => self.tray_command(command),
                HostEvent::TrayMenu => {
                    // The assistant (and its wake word) arrives in a later task.
                    if let Some(command) = self.tray.show_menu(WakeMenu::default()) {
                        self.tray_command(command);
                    }
                }
                HostEvent::TaskbarCreated => self.tray.readd(),
                HostEvent::TrayActivated | HostEvent::OpenSettings => self.open_settings(None),
                HostEvent::AuxInput(which) => self.pump_aux(which),
                HostEvent::AuxClosed(which) => match which {
                    AuxWindow::Settings => self.settings_window = None,
                    AuxWindow::Welcome => {
                        if self.welcome_window.take().is_some() {
                            // Closing the guide without a choice counts as skipping it.
                            let _ = naby_core::settings::welcome::complete(&self.store);
                        }
                    }
                },
                HostEvent::Quit => self.quit = true,
                HostEvent::DisplayChanged => {
                    self.monitors = enumerate();
                    self.followed = self
                        .followed
                        .take()
                        .filter(|id| self.monitors.iter().any(|m| &m.id == id));
                    self.sync_display();
                    self.check_environment();
                    self.signal(Signal::DisplayChanged);
                }
                HostEvent::ForegroundChanged => {
                    ForegroundHooks::acknowledge();
                    self.check_environment();
                }
                HostEvent::Input => {
                    let input = self.window.take_input();
                    let changed = self.surface.input(&input);
                    self.after_surface(changed);
                }
                HostEvent::WindowDpi(_) => {
                    self.monitors = enumerate();
                    self.sync_display();
                    self.signal(Signal::DisplayChanged);
                }
                HostEvent::TextFocusLost => self.signal(Signal::FocusLost),
                HostEvent::OutsideClick { x, y } => {
                    let local = (
                        (x - self.window_origin.0) as f32,
                        (y - self.window_origin.1) as f32,
                    );
                    self.signal(Signal::OutsideClick {
                        x: local.0,
                        y: local.1,
                    });
                }
            }
        }

        fn signal(&mut self, signal: Signal) {
            let changed = self.surface.signal(signal);
            self.after_surface(changed);
        }

        fn tray_command(&mut self, command: TrayCommand) {
            match command {
                TrayCommand::OpenIsland => self.signal(Signal::Panel),
                TrayCommand::Settings => self.open_settings(None),
                TrayCommand::HideShow => self.signal(Signal::ToggleVisibility),
                TrayCommand::Tuck => self.signal(Signal::ToggleTuck),
                TrayCommand::Quit => self.quit = true,
                // Assistant- and command-bar-specific commands arrive with those tasks.
                TrayCommand::CommandBar
                | TrayCommand::MoveResize
                | TrayCommand::Assistant
                | TrayCommand::ToggleWake
                | TrayCommand::StopMicrophones => {}
            }
        }

        // ---- settings, monitors, visibility ---------------------------------------------

        /// Pushes `self.settings` into every part of the shell (also the hook for the future
        /// settings window).
        pub fn apply_settings(&mut self) {
            self.hotkeys.configure(self.host, &self.settings);
            self.window.set_opacity(self.settings.opacity as f32);
            self.surface.apply_settings(&self.settings);
            let wants_hooks = self.settings.hide_fullscreen || self.settings.hide_maximized;
            if wants_hooks && self.hooks.is_none() {
                self.hooks = Some(ForegroundHooks::install(self.tx.clone()));
            } else if !wants_hooks {
                self.hooks = None;
                self.signal_environment(false);
            }
            self.sync_display();
            self.check_environment();
            self.repaint();
        }

        /// Current settings (read-only; the settings window will edit and re-apply).
        pub fn settings(&self) -> &Settings {
            &self.settings
        }

        pub fn store(&self) -> &Store {
            &self.store
        }

        fn monitor_index(&self) -> Option<usize> {
            pick_monitor(&self.settings, &self.monitors, self.followed.as_deref())
        }

        /// Tells the surface which monitor it lives on.
        fn sync_display(&mut self) {
            if let Some(i) = self.monitor_index() {
                let m = &self.monitors[i];
                self.surface
                    .set_display(display_info(m, refresh_rate(&m.id)));
            }
        }

        fn signal_environment(&mut self, hide: bool) {
            let changed = self.surface.signal(Signal::EnvironmentHidden(hide));
            self.after_surface(changed);
        }

        fn check_environment(&mut self) {
            let hide = match self.monitor_index() {
                Some(i) if self.hooks.is_some() => foreground_should_hide(
                    &self.monitors[i],
                    self.settings.hide_fullscreen,
                    self.settings.hide_maximized,
                ),
                _ => false,
            };
            self.signal_environment(hide);
        }

        /// Runs the shell actions the surface queued and repaints when it changed.
        fn after_surface(&mut self, changed: bool) {
            for action in self.surface.take_actions() {
                match action {
                    ShellAction::OpenSettings(page) => self.open_settings(page),
                    ShellAction::Quit => self.quit = true,
                    ShellAction::Balloon { title, detail } => self.notify(&title, &detail),
                    ShellAction::OpenAssistant | ShellAction::OpenVoice => self.notify(
                        "Assistant",
                        "The assistant is not part of this build of the new shell yet.",
                    ),
                    ShellAction::SettingsApplied(settings) => {
                        self.settings = *settings;
                        self.hotkeys.configure(self.host, &self.settings);
                        self.window.set_opacity(self.settings.opacity as f32);
                    }
                }
            }
            if changed {
                self.repaint();
            } else {
                self.reschedule();
            }
        }

        fn shown(&self) -> bool {
            self.surface.size().0 > 0
        }

        // ---- painting ---------------------------------------------------------------------

        fn repaint(&mut self) {
            let size = self.surface.size();
            if size.0 == 0 || size.1 == 0 {
                // Hidden (visibility hotkey, fullscreen app): free the window DIB, the canvas,
                // the glyph cache and the pointer hook.
                self.window.release();
                self.canvas.release();
                self.fonts.clear_cache();
                self.outside = None;
                self.surface.on_hide();
                self.reschedule();
                return;
            }
            self.canvas.resize(size.0, size.1);
            self.canvas.clear(Rgba::TRANSPARENT);
            self.surface.paint(&mut self.canvas, &self.fonts);
            let (x, y) = self.surface.placement().unwrap_or((0, 0));
            self.window.present(&self.canvas, x, y);
            self.window_origin = (x, y);
            self.window.set_text_input(self.surface.wants_focus());
            self.sync_outside_hook();
            self.reschedule();
        }

        fn sync_outside_hook(&mut self) {
            if self.surface.wants_outside_clicks() {
                if self.outside.is_none() {
                    self.outside = OutsideClickHook::install(self.tx.clone());
                }
            } else {
                self.outside = None;
            }
        }

        /// Arms the timer for the surface's next frame (and `Follow` polling) or disarms it.
        fn reschedule(&mut self) {
            let follow = (self.settings.display_mode() == DisplayMode::Follow && self.shown())
                .then(|| {
                    let now = Instant::now();
                    if self.follow_due <= now {
                        self.follow_due = now + FOLLOW_POLL;
                    }
                    self.follow_due
                });
            let next = match (self.surface.next_frame(), follow) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            match next {
                Some(at) => self.timer.request_frame_at(at),
                None => self.timer.cancel(),
            }
        }

        fn on_timer(&mut self) {
            if !self.timer.take_due() {
                return;
            }
            self.follow_tick();
            self.follow_due = Instant::now();
            let changed = self.surface.tick();
            self.after_surface(changed);
        }

        /// `Follow` mode: move the dock to the monitor the pointer dwells on.
        fn follow_tick(&mut self) {
            if self.settings.display_mode() != DisplayMode::Follow || !self.shown() {
                self.follow.reset();
                return;
            }
            let (Some(current), Some((x, y))) = (self.monitor_index(), cursor_pos()) else {
                return;
            };
            let Some(under) = monitor_at(&self.monitors, x, y) else {
                return;
            };
            let now_ms = self.follow_clock.elapsed().as_millis() as i64;
            let blocked = self.window.has_text_input() || self.surface.interaction_active();
            let next = self.follow.observe(
                &self.monitors[current].id,
                &self.monitors[under].id,
                now_ms,
                blocked,
                false,
                false,
            );
            if let Some(id) = next {
                self.followed = Some(id);
                self.sync_display();
                self.check_environment();
                self.signal(Signal::DisplayChanged);
            }
        }
    }

    fn pick_monitor(
        settings: &Settings,
        monitors: &[MonitorInfo],
        followed: Option<&str>,
    ) -> Option<usize> {
        choose_monitor(
            monitors,
            settings.display_mode(),
            &settings.monitor,
            followed,
        )
    }

    impl Drop for App {
        fn drop(&mut self) {
            self.hooks = None;
            self.outside = None;
            self.hotkeys.unregister_all();
            self.tray.remove();
            // SAFETY: destroys the host window, then frees the shared state its window
            // procedure was reading (WM_DESTROY handling never touches it).
            unsafe {
                set_user_data(self.host, 0);
                let _ = DestroyWindow(self.host);
                drop(Box::from_raw(self.host_shared));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_flags_and_ignores_the_rest() {
        let args = Args::parse(["--background", "--settings", "--wat"]);
        assert_eq!(
            args,
            Args {
                exit: false,
                background: true,
                settings: true
            }
        );
        assert!(Args::parse(["--exit"]).exit);
        assert_eq!(Args::parse(Vec::<String>::new()), Args::default());
    }

    #[test]
    fn missing_settings_use_the_recommended_preset() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf());
        let (settings, warning) = load_settings(&store);
        assert_eq!(settings, presets::recommended());
        assert!(warning.is_none());
    }

    #[test]
    fn unreadable_settings_fall_back_with_privacy_features_off() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), "{ not json").unwrap();
        let store = Store::open(dir.path().to_path_buf());
        let (settings, warning) = load_settings(&store);
        assert!(!settings.clipboard_history);
        assert!(!settings.downloads_enabled);
        assert!(warning.is_some());
        // The unreadable file was left untouched for recovery.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.json")).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn stored_settings_are_normalized_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf());
        let mut wide = presets::recommended();
        wide.width = 5000.0;
        store.save("settings", &wide).unwrap();
        let (loaded, _) = load_settings(&store);
        assert!(loaded.width <= 400.0);
    }
}
