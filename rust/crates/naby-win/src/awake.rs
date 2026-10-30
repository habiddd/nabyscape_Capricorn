//! Keep-awake syscall (C# `KeepAwakeService` talks to `SetThreadExecutionState`). The policy
//! lives in `naby_core::productivity::awake::KeepAwakeState`; this module supplies the call.
//!
//! `ES_CONTINUOUS` requests belong to the calling thread, so a `KeepAwake` must be created and
//! used on the UI thread only (it is `!Send` in practice because the app never moves it).

#[cfg(windows)]
mod os {
    use naby_core::productivity::awake::KeepAwakeState;
    use windows::Win32::System::Power::{SetThreadExecutionState, EXECUTION_STATE};

    /// Calls `SetThreadExecutionState`; 0 means Windows refused.
    pub fn set_thread_execution_state(flags: u32) -> u32 {
        // SAFETY: plain Win32 call with documented flag bits.
        unsafe { SetThreadExecutionState(EXECUTION_STATE(flags)) }.0
    }

    pub type KeepAwake = KeepAwakeState<fn(u32) -> u32>;

    /// A keep-awake state wired to the real syscall.
    pub fn new_keep_awake() -> KeepAwake {
        KeepAwakeState::new(set_thread_execution_state as fn(u32) -> u32)
    }
}

#[cfg(windows)]
pub use os::*;
