//! The generic asset fetch's bookkeeping (`LLAssetStorage`,
//! `LLViewerAssetStorage`): the capability URL of an asset, the pending
//! request list's duplicate, timeout and completion selection, the toxic
//! asset map and the error strings. The request objects themselves (with
//! their callbacks and user data), the coroutine that performs the HTTP
//! GET through `LLCore::HttpRequest` and the response's write into the
//! disk cache stay in the viewer.

use std::collections::BTreeMap;

use crate::{Uuid, uuid_to_string};

/// The legacy error codes the callbacks receive.
pub const LL_ERR_NOERR: i32 = 0;
pub const LL_ERR_ASSET_REQUEST_FAILED: i32 = -1;
pub const LL_ERR_ASSET_REQUEST_NONEXISTENT_FILE: i32 = -3;
pub const LL_ERR_ASSET_REQUEST_NOT_IN_DATABASE: i32 = -4;
pub const LL_ERR_EOF: i32 = -39;
pub const LL_ERR_CANNOT_OPEN_FILE: i32 = -42;
pub const LL_ERR_FILE_NOT_FOUND: i32 = -43;
pub const LL_ERR_TCP_TIMEOUT: i32 = -23016;
pub const LL_ERR_CIRCUIT_GONE: i32 = -23017;
pub const LL_ERR_PRICE_MISMATCH: i32 = -23018;
pub const LL_ERR_NO_CAP: i32 = -6;

/// `LL_ASSET_STORAGE_TIMEOUT`: five minutes, in seconds.
pub const ASSET_STORAGE_TIMEOUT_SECS: f64 = 5.0 * 60.0;
/// `TOXIC_ASSET_LIFETIME` in microseconds.
pub const TOXIC_ASSET_LIFETIME_US: u64 = 120 * 1_000_000;

/// `LLAssetStorage::getErrorString`.
pub fn error_string(status: i32) -> &'static str {
    match status {
        LL_ERR_NOERR => "No error",
        LL_ERR_ASSET_REQUEST_FAILED => "Asset request: failed",
        LL_ERR_ASSET_REQUEST_NONEXISTENT_FILE => "Asset request: non-existent file",
        LL_ERR_ASSET_REQUEST_NOT_IN_DATABASE => "Asset request: asset not found in database",
        LL_ERR_NO_CAP => "Asset request: region or asset capability not available",
        LL_ERR_EOF => "End of file",
        LL_ERR_CANNOT_OPEN_FILE => "Cannot open file",
        LL_ERR_FILE_NOT_FOUND => "File not found",
        LL_ERR_TCP_TIMEOUT => "File transfer timeout",
        LL_ERR_CIRCUIT_GONE => "Circuit gone",
        LL_ERR_PRICE_MISMATCH => "Viewer and server do not agree on price",
        _ => "Unknown status",
    }
}

/// `LLViewerAssetStorage::getAssetURL`: `<cap>/?<type>_id=<uuid>`.
pub fn asset_url(cap_url: &str, type_name: &str, id: &Uuid) -> String {
    format!("{cap_url}/?{type_name}_id={}", uuid_to_string(id))
}

/// `mToxicAssetMap`: assets that caused trouble when rezzed recently.
#[derive(Debug, Default, Clone)]
pub struct ToxicMap {
    map: BTreeMap<Uuid, u64>,
}

impl ToxicMap {
    /// `isAssetToxic`: also renews the entry's expiry.
    pub fn is_toxic(&mut self, id: &Uuid, now_us: u64) -> bool {
        if *id == [0; 16] {
            return false;
        }
        match self.map.get_mut(id) {
            Some(expiry) => {
                *expiry = now_us + TOXIC_ASSET_LIFETIME_US;
                true
            }
            None => false,
        }
    }

    /// `flushOldToxicAssets`.
    pub fn flush_old(&mut self, now_us: u64, force: bool) {
        self.map.retain(|_, expiry| !force && *expiry >= now_us);
    }

    /// `markAssetToxic`.
    pub fn mark(&mut self, id: &Uuid, now_us: u64) {
        if *id != [0; 16] {
            self.map.insert(*id, now_us + TOXIC_ASSET_LIFETIME_US);
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The identity of a pending request as the list logic sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RequestKey {
    pub id: Uuid,
    pub asset_type: i32,
    /// The callback and user data identities (`mDownCallback` target and
    /// `mUserData`), for duplicate detection.
    pub callback: u64,
    pub user_data: u64,
    /// `mTime`: message-system seconds when queued.
    pub time: f64,
}

/// `getAssetData`'s verdict on a new request against the pending downloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Same asset, same callback and user data: discarded.
    Discard,
    /// Same asset already pending for another caller: queued without a
    /// new fetch.
    Duplicate,
    /// A new fetch.
    New,
}

/// `getAssetData`'s duplicate scan over the pending downloads.
pub fn duplicate_verdict(
    pending: &[RequestKey],
    id: &Uuid,
    asset_type: i32,
    callback: u64,
    user_data: u64,
) -> Verdict {
    let mut duplicate = false;
    for r in pending {
        if r.asset_type == asset_type && r.id == *id {
            if r.callback == callback && r.user_data == user_data {
                return Verdict::Discard;
            }
            duplicate = true;
        }
    }
    if duplicate {
        Verdict::Duplicate
    } else {
        Verdict::New
    }
}

/// `removeAndCallbackPendingDownloads`'s selection: the indices of the
/// pending downloads for the asset, in the order their callbacks run (the
/// C++ pushed each to the front of a list, so the latest matching request
/// is called back first).
pub fn completion_order(pending: &[RequestKey], id: &Uuid, asset_type: i32) -> Vec<usize> {
    let mut out: Vec<usize> = pending
        .iter()
        .enumerate()
        .filter(|(_, r)| r.id == *id && r.asset_type == asset_type)
        .map(|(i, _)| i)
        .collect();
    out.reverse();
    out
}

/// `_cleanupRequests`'s selection for one list: the indices to time out
/// (every request when `all`, else downloads older than the timeout), in
/// callback order (reversed, as above).
pub fn timed_out_order(
    requests: &[RequestKey],
    is_download_list: bool,
    all: bool,
    now_secs: f64,
    timeout_secs: f64,
) -> Vec<usize> {
    let mut out: Vec<usize> = requests
        .iter()
        .enumerate()
        .filter(|(_, r)| all || (is_download_list && timeout_secs < (now_secs - r.time)))
        .map(|(i, _)| i)
        .collect();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_strings() {
        let id = crate::uuid_from_string("01234567-89ab-cdef-1032-547698badcfe").unwrap();
        assert_eq!(
            asset_url("http://cap/va", "texture", &id),
            "http://cap/va/?texture_id=01234567-89ab-cdef-1032-547698badcfe"
        );
        assert_eq!(
            error_string(LL_ERR_NO_CAP),
            "Asset request: region or asset capability not available"
        );
        assert_eq!(error_string(12345), "Unknown status");
    }

    #[test]
    fn toxic() {
        let mut t = ToxicMap::default();
        let id = [5; 16];
        assert!(!t.is_toxic(&id, 0));
        t.mark(&id, 1_000_000);
        assert!(t.is_toxic(&id, 2_000_000));
        t.flush_old(2_000_000 + TOXIC_ASSET_LIFETIME_US - 1, false);
        assert!(t.is_toxic(&id, 0));
        t.flush_old(u64::MAX, false);
        assert!(!t.is_toxic(&id, 0));
        t.mark(&[0; 16], 0);
        assert!(t.is_empty());
    }

    #[test]
    fn list_policies() {
        let a = [1; 16];
        let b = [2; 16];
        let pending = vec![
            RequestKey {
                id: a,
                asset_type: 0,
                callback: 10,
                user_data: 1,
                time: 0.0,
            },
            RequestKey {
                id: b,
                asset_type: 0,
                callback: 10,
                user_data: 2,
                time: 100.0,
            },
            RequestKey {
                id: a,
                asset_type: 0,
                callback: 11,
                user_data: 1,
                time: 400.0,
            },
        ];
        assert_eq!(duplicate_verdict(&pending, &a, 0, 10, 1), Verdict::Discard);
        assert_eq!(
            duplicate_verdict(&pending, &a, 0, 12, 1),
            Verdict::Duplicate
        );
        assert_eq!(duplicate_verdict(&pending, &a, 1, 10, 1), Verdict::New);
        assert_eq!(completion_order(&pending, &a, 0), vec![2, 0]);
        assert_eq!(
            timed_out_order(&pending, true, false, 401.0, 300.0),
            vec![1, 0]
        );
        assert_eq!(
            timed_out_order(&pending, false, false, 401.0, 300.0),
            Vec::<usize>::new()
        );
        assert_eq!(
            timed_out_order(&pending, false, true, 0.0, 300.0),
            vec![2, 1, 0]
        );
    }
}
