//! The inventory background fetch: the scheduler and request building of
//! `LLInventoryModelBackgroundFetch` (`newview/llinventorymodelbackgroundfetch.cpp`)
//! and the fetch side of `AISAPI` (`newview/llaisapi.cpp`), recorder R3b
//! (`tests/fixtures/ais/fetch/README.md`), work items K5, K10, W3, W4.
//!
//! What moved here is the decision making: the folder and item queues and
//! their priorities, the forced set, the subset batches and their
//! boundaries, the depth limit, the fallbacks between full, folder-and-
//! content and content fetches, the concurrency limit, the frame budget, the
//! choice between AIS, the legacy capabilities and nothing, the legacy batches
//! and their 403 split, the completion bookkeeping, and the bytes of every
//! request (AIS URLs, legacy POST bodies).
//!
//! What stays C++: the coroutines and the coprocedure pool, the HTTP
//! transport (and its 503 retries), the model's containers and observers,
//! the AIS reply parse (R3a, `ais/update`) and its applier `AISUpdate::doUpdate`
//! with its `checkTimeout()` yields and the `LLCoros::checkStop()` after them,
//! the legacy reply applier (`BGFolderHttpHandler::processData`'s model
//! writes), and the UDP messages.
//!
//! # Shape
//!
//! [`scheduler::BackgroundFetch`] is driven by explicit calls (the C++
//! entry points: `start`, `scheduleFolderFetch`, the idle callback, …) and by
//! events (an AIS command completed, a legacy handler finished). It reads the
//! model through [`scheduler::FetchModel`] (the C++ model behind a narrow
//! view; the two category writes the scheduler makes, the fetch state and the
//! version 0 of a failed folder, go through it as well, because later reads
//! in the same loop must see them), the environment through
//! [`scheduler::Env`] (settings, region, capabilities), and the frame budget's
//! clock through [`scheduler::BudgetClock`], read at the same three points
//! as `LLTimer::getTotalSeconds()` in `bulkFetchViaAis` (the K5 seam). Every
//! effect on the outside is an [`scheduler::Action`] in an ordered list the
//! caller drains: enqueue this AIS request, post this legacy body, run this
//! callback, raise this notification, fire the completion signal.
//!
//! An AIS command that cannot be sent (no capability, an empty subset) calls
//! back with the null id at once, inside the call that issued it, exactly as
//! the C++ does: the scheduler's own failure branches only queue at the back
//! (s17).
//!
//! The shutdown seam (K10): `AISUpdate::checkTimeout()` stays C++ with its
//! `LLCoros::checkStop()`. When it throws, the command's completion never
//! runs; the caller reports that with
//! [`scheduler::BackgroundFetch::ais_abandoned`], which drops the completion
//! without running it, so the fetch count stays raised as in the C++ (s24).
//!
//! UUIDs are `[u8; 16]` ([`crate::perms::Uuid`]), ordered byte-wise as
//! `LLUUID::operator<`.

pub mod ais;
pub mod legacy;
pub mod scheduler;
pub mod timer;

pub use crate::perms::Uuid;

/// `LLUUID::null`.
pub const NULL_ID: Uuid = [0; 16];

/// `LLFolderType::FT_MARKETPLACE_LISTINGS`.
pub const FT_MARKETPLACE_LISTINGS: i8 = 53;

/// `LLViewerInventoryCategory::VERSION_UNKNOWN`.
pub const VERSION_UNKNOWN: i32 = -1;

/// `LLUUID::asString`: lower-case hex, dashed.
pub fn uuid_string(id: &Uuid) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(36);
    for (i, b) in id.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 15) as usize] as char);
    }
    s
}

/// `LLInventoryModelBackgroundFetch::EFetchType`, the queue entry's kind.
/// The order matters: the scheduler compares them (`>= FT_CONTENT_RECURSIVE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum FetchType {
    /// `FT_DEFAULT`
    Default = 0,
    /// `FT_FORCED`: request non-recursively even if already loaded.
    Forced = 1,
    /// `FT_CONTENT_RECURSIVE`: request content recursively.
    ContentRecursive = 2,
    /// `FT_FOLDER_AND_CONTENT`: request folder, then content recursively.
    FolderAndContent = 3,
    /// `FT_RECURSIVE`: request everything recursively.
    Recursive = 4,
}

/// `LLViewerInventoryCategory::EFetchType`, a category's fetch state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum CategoryFetch {
    /// `FETCH_NONE`
    None = 0,
    /// `FETCH_NORMAL`
    Normal = 1,
    /// `FETCH_RECURSIVE`
    Recursive = 2,
    /// `FETCH_FAILED`: back off.
    Failed = 3,
}

/// One entry of the folder or item queue (`FetchQueueInfo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueEntry {
    pub id: Uuid,
    pub is_category: bool,
    pub fetch_type: FetchType,
}
