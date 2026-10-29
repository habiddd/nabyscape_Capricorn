//! Shared Core Audio helpers: COM apartment guard, endpoint enumeration, names, and the
//! undocumented `IPolicyConfig` used to switch the default output (`AudioDevices.cs`).
#![allow(dead_code)] // consumed by the app shell, which is wired separately

use std::ffi::c_void;

use naby_core::media::AudioOutput;
use windows::core::{
    interface, IUnknown, IUnknown_Vtbl, Interface, BOOL, GUID, HRESULT, PCWSTR, PWSTR,
};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};

use crate::audio_logic::{self, DeviceRole};

/// Joins the multithreaded COM apartment for the current thread until dropped.
/// Every service thread creates one before touching COM or WinRT.
pub struct ComApartment(bool);

impl ComApartment {
    pub fn mta() -> Self {
        // SAFETY: plain apartment initialization on the calling thread; balanced in Drop
        // only when it succeeded (a thread already in another mode keeps working).
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        ComApartment(hr.is_ok())
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx in `mta`.
            unsafe { CoUninitialize() };
        }
    }
}

/// NUL-terminated UTF-16 copy for `PCWSTR` arguments.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Copies a COM-allocated wide string and frees it.
pub fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: `p` is a valid NUL-terminated string allocated with CoTaskMemAlloc by the
    // callee; it is read once and freed exactly once here.
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

/// Copies a borrowed wide string (callback arguments).
pub fn read_pcwstr(p: &PCWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: Core Audio passes valid NUL-terminated strings to notification callbacks.
    unsafe { p.to_string().unwrap_or_default() }
}

pub fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    // SAFETY: CoCreateInstance for a registered in-proc class; the apartment is initialized by the caller.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

pub fn device_id(device: &IMMDevice) -> windows::core::Result<String> {
    // SAFETY: GetId returns a CoTaskMem string that `take_pwstr` frees.
    unsafe { device.GetId().map(take_pwstr) }
}

/// Friendly name from the endpoint property store; empty when unavailable.
pub fn friendly_name(device: &IMMDevice) -> String {
    // SAFETY: property store access on a live endpoint; the PROPVARIANT is cleared and the
    // converted string freed before returning.
    unsafe {
        let Ok(store) = device.OpenPropertyStore(STGM_READ) else {
            return String::new();
        };
        let Ok(mut value) =
            store.GetValue(&windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName)
        else {
            return String::new();
        };
        let name = PropVariantToStringAlloc(&value)
            .map(take_pwstr)
            .unwrap_or_default();
        let _ = PropVariantClear(&mut value);
        name
    }
}

pub fn default_endpoint(
    en: &IMMDeviceEnumerator,
    flow: EDataFlow,
    role: ERole,
) -> windows::core::Result<IMMDevice> {
    // SAFETY: plain COM call on a live enumerator.
    unsafe { en.GetDefaultAudioEndpoint(flow, role) }
}

pub fn device_by_id(en: &IMMDeviceEnumerator, id: &str) -> windows::core::Result<IMMDevice> {
    let id = wide(id);
    // SAFETY: `id` is NUL-terminated and outlives the call.
    unsafe { en.GetDevice(PCWSTR(id.as_ptr())) }
}

/// Active endpoints of one direction.
pub fn active_devices(
    en: &IMMDeviceEnumerator,
    flow: EDataFlow,
) -> windows::core::Result<Vec<IMMDevice>> {
    // SAFETY: plain COM calls; every item is an owned interface pointer.
    unsafe {
        let list = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
        let count = list.GetCount()?;
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            if let Ok(d) = list.Item(i) {
                out.push(d);
            }
        }
        Ok(out)
    }
}

pub fn is_active(device: &IMMDevice) -> bool {
    // SAFETY: plain COM call.
    unsafe { device.GetState().is_ok_and(|s| s == DEVICE_STATE_ACTIVE) }
}

/// `(id, friendly name)` of every active render endpoint, sorted by name.
pub fn render_outputs(en: &IMMDeviceEnumerator) -> windows::core::Result<Vec<AudioOutput>> {
    let mut outputs = Vec::new();
    for d in active_devices(en, eRender)? {
        if let Ok(id) = device_id(&d) {
            outputs.push(AudioOutput {
                id,
                name: friendly_name(&d),
            });
        }
    }
    audio_logic::sort_outputs(&mut outputs);
    Ok(outputs)
}

/// Default capture endpoint: Communications when present, else Multimedia (`MicrophoneEndpoint`).
pub fn microphone_endpoint(en: &IMMDeviceEnumerator) -> windows::core::Result<IMMDevice> {
    default_endpoint(en, eCapture, eCommunications)
        .or_else(|_| default_endpoint(en, eCapture, eMultimedia))
}

/// Whether `id` names an active render endpoint (rejects stale or capture ids).
pub fn is_active_render(en: &IMMDeviceEnumerator, id: &str) -> bool {
    use windows::Win32::Media::Audio::IMMEndpoint;
    let Ok(device) = device_by_id(en, id) else {
        return false;
    };
    let Ok(endpoint) = device.cast::<IMMEndpoint>() else {
        return false;
    };
    // SAFETY: plain COM call.
    let flow = unsafe { endpoint.GetDataFlow() };
    flow.is_ok_and(|f| f == eRender) && is_active(&device)
}

/// Full path of a process image via a limited-rights handle (`AudioMixer.LocalProcessImage`);
/// works across bitness without reading modules.
pub fn process_image(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if pid == 0 {
        return None;
    }
    // SAFETY: the handle is opened with query-only rights, used for one call, and closed
    // on every path; the buffer length passed matches the allocation.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Process name without extension, as `Process.ProcessName`.
pub fn process_name(pid: u32) -> Option<String> {
    process_image(pid).map(|p| audio_logic::file_stem(&p).to_owned())
}

#[allow(non_snake_case)]
mod policy {
    use super::*;

    // Methods preceding SetDefaultEndpoint keep the native vtable order of the undocumented interface.
    #[interface("F8679F50-850A-41CF-9C72-430F290290C8")]
    pub unsafe trait IPolicyConfig: IUnknown {
        unsafe fn GetMixFormat(&self, id: PCWSTR, format: *mut c_void) -> HRESULT;
        unsafe fn GetDeviceFormat(&self, id: PCWSTR, default: BOOL, format: *mut c_void)
            -> HRESULT;
        unsafe fn ResetDeviceFormat(&self, id: PCWSTR) -> HRESULT;
        unsafe fn SetDeviceFormat(
            &self,
            id: PCWSTR,
            endpoint: *mut c_void,
            mix: *mut c_void,
        ) -> HRESULT;
        unsafe fn GetProcessingPeriod(
            &self,
            id: PCWSTR,
            default: BOOL,
            period: *mut c_void,
            minimum: *mut c_void,
        ) -> HRESULT;
        unsafe fn SetProcessingPeriod(&self, id: PCWSTR, period: *mut c_void) -> HRESULT;
        unsafe fn GetShareMode(&self, id: PCWSTR, mode: *mut c_void) -> HRESULT;
        unsafe fn SetShareMode(&self, id: PCWSTR, mode: *mut c_void) -> HRESULT;
        unsafe fn GetPropertyValue(
            &self,
            id: PCWSTR,
            key: *mut c_void,
            value: *mut c_void,
        ) -> HRESULT;
        unsafe fn SetPropertyValue(
            &self,
            id: PCWSTR,
            key: *mut c_void,
            value: *mut c_void,
        ) -> HRESULT;
        unsafe fn SetDefaultEndpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
        unsafe fn SetEndpointVisibility(&self, id: PCWSTR, visible: BOOL) -> HRESULT;
    }

    /// `id` must be NUL-terminated.
    pub fn set_default_endpoint(policy: &IPolicyConfig, id: &[u16], role: ERole) -> HRESULT {
        // SAFETY: `id` is a NUL-terminated UTF-16 slice that outlives the call; the vtable
        // order matches the declaration above.
        unsafe { policy.SetDefaultEndpoint(PCWSTR(id.as_ptr()), role) }
    }
}
use policy::IPolicyConfig;

const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

fn native_role(role: DeviceRole) -> ERole {
    match role {
        DeviceRole::Console => eConsole,
        DeviceRole::Multimedia => eMultimedia,
        DeviceRole::Communications => eCommunications,
    }
}

/// Makes `id` the default output for all three roles. Every role is attempted;
/// failures are collected (`AudioOutputPolicy.SetDefault`).
pub fn set_default_output(id: &str) -> Result<(), Vec<String>> {
    // SAFETY: CoCreateInstance for the PolicyConfig client class on an initialized apartment.
    let policy: IPolicyConfig =
        match unsafe { CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) } {
            Ok(p) => p,
            Err(e) => return Err(vec![e.to_string()]),
        };
    let wide_id = wide(id);
    audio_logic::apply_roles(|role| {
        policy::set_default_endpoint(&policy, &wide_id, native_role(role))
            .ok()
            .map_err(|e| e.to_string())
    })
}
