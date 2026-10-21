//! Host facilities the C++ supplied to the ported code and that the crate
//! does not own: the log (`LL_DEBUGS`/`LL_INFOS`/`LL_WARNS`) and the wall
//! clock the texture cache stamps entries with (`time(NULL)`).

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Log levels, in the viewer's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
}

/// The log sink. Installed once by the host; messages before that are
/// dropped.
pub type LogFn = Box<dyn Fn(LogLevel, &str) + Send + Sync>;

static LOG: OnceLock<LogFn> = OnceLock::new();

/// Installs the log sink. A second call is ignored.
pub fn set_logger(f: LogFn) {
    let _ = LOG.set(f);
}

pub fn log(level: LogLevel, msg: &str) {
    if let Some(f) = LOG.get() {
        f(level, msg);
    }
}

#[macro_export]
macro_rules! ll_warns {
    ($($arg:tt)*) => { $crate::host::log($crate::host::LogLevel::Warn, &format!($($arg)*)) };
}
#[macro_export]
macro_rules! ll_infos {
    ($($arg:tt)*) => { $crate::host::log($crate::host::LogLevel::Info, &format!($($arg)*)) };
}
#[macro_export]
macro_rules! ll_debugs {
    ($($arg:tt)*) => { $crate::host::log($crate::host::LogLevel::Debug, &format!($($arg)*)) };
}

/// Seconds since 1970 as a `u32`, as the C++ `(U32)time(NULL)`.
pub type ClockFn = fn() -> u32;

/// The default clock: the system time.
pub fn system_seconds() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}
