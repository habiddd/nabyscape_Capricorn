//! Brightness change feedback (`BrightnessService.cs`).
//!
//! Only subscribes to Windows' own `WmiMonitorBrightnessEvent`; displays without that WMI
//! provider (most external monitors) stay unsupported. There are no writes and no polling
//! fallback. The thread exists only while feedback is enabled and sleeps inside WMI; it
//! checks the stop flag once a second, so a disabled or dropped service releases its COM
//! objects within about a second without blocking the caller.
#![allow(dead_code)] // consumed by the app shell, which is wired separately

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use windows::core::{BSTR, PCWSTR};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Com::{
    CoCreateInstance, CoSetProxyBlanket, CLSCTX_INPROC_SERVER, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
    RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Variant::{
    VariantClear, VARIANT, VT_I2, VT_I4, VT_UI1, VT_UI2, VT_UI4,
};
use windows::Win32::System::Wmi::{
    IEnumWbemClassObject, IWbemClassObject, IWbemLocator, IWbemServices, WbemLocator,
    WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_S_TIMEDOUT,
};

use crate::devices::{wide, ComApartment};
use crate::wake::Waker;

pub const STATUS_OFF: &str = "Brightness feedback off";
pub const STATUS_UNAVAILABLE: &str = "Brightness feedback unavailable on this display";
pub const STATUS_ACTIVE: &str = "Brightness events active";

/// Latest event; `sequence` increments for every brightness event (also repeats of a level).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrightnessSnapshot {
    pub status: String,
    /// 0..=100 from the last event, if any arrived.
    pub level: Option<u8>,
    pub sequence: u64,
}

impl Default for BrightnessSnapshot {
    fn default() -> Self {
        BrightnessSnapshot {
            status: STATUS_UNAVAILABLE.to_owned(),
            level: None,
            sequence: 0,
        }
    }
}

pub struct BrightnessService {
    wake: Waker,
    shared: Arc<Mutex<BrightnessSnapshot>>,
    stop: Option<Arc<AtomicBool>>,
    attempted: bool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl BrightnessService {
    /// `enabled` mirrors the BrightnessFeedback setting; when false nothing is started.
    pub fn start(wake: Waker, enabled: bool) -> Self {
        let mut s = BrightnessService {
            wake,
            shared: Arc::new(Mutex::new(BrightnessSnapshot {
                status: STATUS_OFF.to_owned(),
                ..BrightnessSnapshot::default()
            })),
            stop: None,
            attempted: false,
        };
        s.set_enabled(enabled);
        s
    }

    pub fn snapshot(&self) -> BrightnessSnapshot {
        lock(&self.shared).clone()
    }

    /// Starts or stops the subscription. An unsupported display is probed once per enable.
    pub fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.halt();
            self.attempted = false;
            let changed = {
                let mut s = lock(&self.shared);
                let next = BrightnessSnapshot {
                    status: STATUS_OFF.to_owned(),
                    level: None,
                    sequence: s.sequence,
                };
                let changed = *s != next;
                *s = next;
                changed
            };
            if changed {
                (self.wake)();
            }
            return;
        }
        if self.stop.is_some() || self.attempted {
            return;
        }
        self.attempted = true;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let shared = Arc::clone(&self.shared);
        let wake = Arc::clone(&self.wake);
        if std::thread::Builder::new()
            .name("naby-brightness".into())
            .spawn(move || run(thread_stop, shared, wake))
            .is_ok()
        {
            self.stop = Some(stop);
        }
    }

    fn halt(&mut self) {
        if let Some(stop) = self.stop.take() {
            // The thread notices within one WMI timeout and releases its COM objects itself.
            stop.store(true, Ordering::Release);
        }
    }
}

impl Drop for BrightnessService {
    fn drop(&mut self) {
        self.halt();
    }
}

fn publish(
    shared: &Mutex<BrightnessSnapshot>,
    wake: &Waker,
    update: impl FnOnce(&mut BrightnessSnapshot),
) {
    let changed = {
        let mut s = lock(shared);
        let before = s.clone();
        update(&mut s);
        *s != before
    };
    if changed {
        wake();
    }
}

fn run(stop: Arc<AtomicBool>, shared: Arc<Mutex<BrightnessSnapshot>>, wake: Waker) {
    let _com = ComApartment::mta();
    if let Ok((_services, events)) = subscribe() {
        {
            publish(&shared, &wake, |s| s.status = STATUS_ACTIVE.to_owned());
            while !stop.load(Ordering::Acquire) {
                let mut objects: [Option<IWbemClassObject>; 1] = [None];
                let mut returned = 0u32;
                // SAFETY: blocks inside WMI for at most one second; `objects` and `returned`
                // outlive the call.
                let hr = unsafe { events.Next(1000, &mut objects, &mut returned) };
                if hr.0 == WBEM_S_TIMEDOUT.0 {
                    continue;
                }
                if hr.is_err() {
                    break;
                }
                if returned == 0 {
                    continue;
                }
                if stop.load(Ordering::Acquire) {
                    break;
                }
                if let Some(level) = objects[0].as_ref().and_then(read_brightness) {
                    publish(&shared, &wake, |s| {
                        s.level = Some(level);
                        s.sequence += 1;
                    });
                }
            }
        }
    }
    if !stop.load(Ordering::Acquire) {
        // The subscription ended on its own (unsupported or the provider went away).
        publish(&shared, &wake, |s| {
            s.status = STATUS_UNAVAILABLE.to_owned();
            s.level = None;
        });
    }
}

type Subscription = (IWbemServices, IEnumWbemClassObject);

fn subscribe() -> Result<Subscription, windows::core::Error> {
    let flags = WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY;
    let language = BSTR::from("WQL");
    // SAFETY: standard WMI client setup on an initialized apartment; every interface is an
    // owned smart pointer, and the proxy blanket is applied to the services pointer we own.
    unsafe {
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let services = locator.ConnectServer(
            &BSTR::from("ROOT\\WMI"),
            &BSTR::new(),
            &BSTR::new(),
            &BSTR::new(),
            0,
            &BSTR::new(),
            None,
        )?;
        CoSetProxyBlanket(
            &services,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            None,
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )?;
        // Only displays whose provider exposes brightness are supported.
        let present = services.ExecQuery(
            &language,
            &BSTR::from("SELECT InstanceName FROM WmiMonitorBrightness"),
            flags,
            None,
        )?;
        let mut probe: [Option<IWbemClassObject>; 1] = [None];
        let mut found = 0u32;
        let hr = present.Next(5000, &mut probe, &mut found);
        if hr.is_err() || found == 0 {
            return Err(windows::core::Error::from(E_FAIL));
        }
        drop(probe);
        let events = services.ExecNotificationQuery(
            &language,
            &BSTR::from("SELECT * FROM WmiMonitorBrightnessEvent"),
            flags,
            None,
        )?;
        Ok((services, events))
    }
}

fn read_brightness(object: &IWbemClassObject) -> Option<u8> {
    let name = wide("Brightness");
    let mut value = VARIANT::default();
    // SAFETY: `Get` fills the VARIANT, which is read according to its tag and then cleared.
    unsafe {
        object
            .Get(PCWSTR(name.as_ptr()), 0, &mut value, None, None)
            .ok()?;
        let inner = &value.Anonymous.Anonymous;
        let raw = match inner.vt {
            VT_UI1 => Some(i32::from(inner.Anonymous.bVal)),
            VT_I2 => Some(i32::from(inner.Anonymous.iVal)),
            VT_UI2 => Some(i32::from(inner.Anonymous.uiVal)),
            VT_I4 => Some(inner.Anonymous.lVal),
            VT_UI4 => i32::try_from(inner.Anonymous.ulVal).ok(),
            _ => None,
        };
        let _ = VariantClear(&mut value);
        raw.map(|v| v.clamp(0, 100) as u8)
    }
}
