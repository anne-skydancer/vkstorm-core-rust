//! `LLAdaptiveRetryPolicy` (`llhttpretrypolicy.cpp`): the backoff the
//! texture fetcher applies to server-bake fetches that fail.

use std::time::{Duration, Instant};

use crate::ll_infos;

/// `isHttpServerErrorStatus`: 500 (`HTTP_INTERNAL_ERROR`) or any 5xx.
pub fn is_http_server_error_status(status: i32) -> bool {
    status == 500 || (500..600).contains(&status)
}

/// `getSecondsUntilRetryAfter`: a `Retry-After` value in delta seconds
/// (any `strtod`-parsable number taking the whole string).
pub fn seconds_until_retry_after(retry_after: &str) -> Option<f32> {
    let t = retry_after.trim_start();
    // strtod accepts leading whitespace, a sign, decimals, exponents, and
    // "inf"/"nan"; the C++ then required the end of the string.
    let parsed: Result<f64, _> = t.parse();
    match parsed {
        Ok(v) => Some(v as f32),
        Err(_) => {
            // strtod also accepts forms Rust's parser rejects (e.g. "1." or
            // ".5e1" are accepted by both; hex floats are not used here).
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdaptiveRetryPolicy {
    min_delay: f32,
    max_delay: f32,
    backoff_factor: f32,
    max_retries: u32,
    retry_on_4xx: bool,
    delay: f32,
    retry_count: u32,
    /// `mRetryTimer`: expiry of the current wait.
    retry_deadline: Option<Instant>,
    retry_started: Instant,
    should_retry: bool,
}

impl AdaptiveRetryPolicy {
    pub fn new(
        min_delay: f32,
        max_delay: f32,
        backoff_factor: f32,
        max_retries: u32,
        retry_on_4xx: bool,
    ) -> Self {
        AdaptiveRetryPolicy {
            min_delay,
            max_delay,
            backoff_factor,
            max_retries,
            retry_on_4xx,
            delay: min_delay,
            retry_count: 0,
            retry_deadline: None,
            retry_started: Instant::now(),
            should_retry: true,
        }
    }

    fn init(&mut self) {
        self.delay = self.min_delay;
        self.retry_count = 0;
        self.should_retry = true;
    }

    pub fn reset(&mut self) {
        self.init();
    }

    pub fn on_success(&mut self) {
        self.init();
    }

    /// `onFailureCommon`: `status` is the HTTP status (or the `HttpStatus`
    /// type for transport errors), `retry_header` a parsed `Retry-After`.
    pub fn on_failure(&mut self, status: i32, retry_header: Option<f32>) {
        if !self.should_retry {
            ll_infos!("keep on failing");
            return;
        }
        if self.retry_count > 0 {
            self.delay = (self.delay * self.backoff_factor).clamp(self.min_delay, self.max_delay);
        }
        let wait_time = retry_header.unwrap_or(self.delay);
        if self.retry_count >= self.max_retries {
            ll_infos!("Too many retries {}, will not retry", self.retry_count);
            self.should_retry = false;
        }
        if !self.retry_on_4xx && !is_http_server_error_status(status) {
            ll_infos!("Non-server error {}, will not retry", status);
            self.should_retry = false;
        }
        if self.should_retry {
            ll_infos!(
                "Retry count {} should retry after {}",
                self.retry_count,
                wait_time
            );
            self.retry_started = Instant::now();
            self.retry_deadline =
                Some(self.retry_started + Duration::from_secs_f32(wait_time.max(0.0)));
        }
        self.retry_count += 1;
    }

    /// `shouldRetry`: whether a retry is due and in how many seconds (0 or
    /// negative when already due; `f32::MAX` when never).
    pub fn should_retry(&self) -> (bool, f32) {
        if self.retry_count == 0 {
            return (false, f32::MAX);
        }
        let seconds = if self.should_retry {
            match self.retry_deadline {
                Some(d) => {
                    let now = Instant::now();
                    if d > now {
                        (d - now).as_secs_f32()
                    } else {
                        -(now - d).as_secs_f32()
                    }
                }
                None => 0.0,
            }
        } else {
            f32::MAX
        };
        (self.should_retry, seconds)
    }

    pub fn retry_count(&self) -> u32 {
        self.retry_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_parsing() {
        assert_eq!(seconds_until_retry_after("5"), Some(5.0));
        assert_eq!(seconds_until_retry_after("2.5"), Some(2.5));
        assert_eq!(
            seconds_until_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"),
            None
        );
        assert_eq!(seconds_until_retry_after(""), None);
    }

    #[test]
    fn backoff_sequence() {
        let mut p = AdaptiveRetryPolicy::new(1.0, 8.0, 2.0, 3, false);
        assert_eq!(p.should_retry(), (false, f32::MAX));
        p.on_failure(503, None);
        let (ok, secs) = p.should_retry();
        assert!(ok && secs > 0.9 && secs <= 1.0);
        p.on_failure(503, None);
        let (ok, secs) = p.should_retry();
        assert!(ok && secs > 1.9 && secs <= 2.0);
        p.on_failure(503, Some(0.5));
        let (ok, secs) = p.should_retry();
        assert!(ok && secs > 0.4 && secs <= 0.5);
        p.on_failure(503, None);
        assert!(!p.should_retry().0);
        p.on_success();
        p.on_failure(404, None);
        assert!(!p.should_retry().0);
    }
}
