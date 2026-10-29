//! The inventory offer throttle (`check_offer_throttle`, `gThrottleTimer`,
//! `llviewermessage.cpp`).

use vkstorm_llsd::Value;

use crate::llsd::coerce::insert;

use super::host::{Action, Host};

/// `OFFER_THROTTLE_TIME` (seconds).
pub const OFFER_THROTTLE_TIME: f32 = 10.0;

/// The throttle's state: `gThrottleTimer`'s expiry (an `LLFrameTimer`: it
/// has expired once the frame clock reaches it; a new timer's expiry is 0)
/// and `check_offer_throttle`'s statics.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Throttle {
    pub expiry: f64,
    pub count: u32,
    pub logged: bool,
}

/// `check_offer_throttle(from_name, check_only)`: true when the offer may be
/// shown. `check_only` only asks whether the timer has expired; otherwise an
/// expired timer restarts the count, and once `FSOfferThrottleMaxCount`
/// offers came within the period (after startup) the first throttled one
/// posts a `SystemMessage` (when its `FSNotifyIncomingObjectSpam*` setting is
/// on) and the rest are refused silently.
pub fn check_offer_throttle(host: &mut dyn Host, from_name: &[u8], check_only: bool) -> bool {
    let now = host.frame_time();
    if check_only {
        return now >= host.throttle().expiry;
    }
    if now >= host.throttle().expiry {
        let t = host.throttle();
        t.expiry = now + OFFER_THROTTLE_TIME as f64;
        t.count = 1;
        t.logged = false;
        return true;
    }
    let max = host.setting_u32("FSOfferThrottleMaxCount");
    if host.startup_started() && host.throttle().count >= max {
        if !host.throttle().logged {
            let app = host.trans_app_title();
            let time = b"10".to_vec();
            let mut log_msg = Vec::new();
            if !from_name.is_empty() {
                if host.setting_bool("FSNotifyIncomingObjectSpamFrom") {
                    log_msg = host.trans(
                        "ItemsComingInTooFastFrom",
                        &[
                            ("APP_NAME", app),
                            ("FROM_NAME", from_name.to_vec()),
                            ("TIME", time),
                        ],
                    );
                }
            } else if host.setting_bool("FSNotifyIncomingObjectSpam") {
                log_msg = host.trans("ItemsComingInTooFast", &[("APP_NAME", app), ("TIME", time)]);
            }
            if !log_msg.is_empty() {
                let mut args = Value::Undefined;
                insert(&mut args, "MESSAGE", Value::String(log_msg));
                host.act(Action::Notify {
                    name: "SystemMessage".to_owned(),
                    substitutions: args,
                    payload: Value::Undefined,
                    handler: None,
                });
            }
            host.throttle().logged = true;
        }
        false
    } else {
        host.throttle().count += 1;
        true
    }
}
