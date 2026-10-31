//! `HttpPolicy`: the per-class ready and retry queues, the concurrency
//! limit and the request-rate throttle of each class, the retry decision
//! with its backoff, and the stall used while a class's multi handle is
//! reconfigured.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};

use crate::hooks::{Hooks, LogLevel, Stat};
use crate::options::{
    ClassOptions, GlobalOptions, INVALID_POLICY_ID, POLICY_CLASS_LIMIT, TRACE_OFF,
};
use crate::request::RequestOp;
use crate::status::{Status, he};
use crate::transport::Transport;

/// How soon the service loop should run again.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LoopSpeed {
    /// Keep polling the queues.
    Normal = 0,
    /// Nothing pending: wait for a request.
    RequestSleep = 1,
}

/// The retry queue is the C++ `std::priority_queue` with the comparison
/// `lhs.retry_at < rhs.retry_at`, so its top is the request with the
/// **latest** retry time, and the queue is served only once that one is
/// due. Kept as it was; see the rung notes.
struct RetryEntry(Box<RequestOp>);

impl PartialEq for RetryEntry {
    fn eq(&self, other: &Self) -> bool {
        self.0.retry_at == other.0.retry_at
    }
}
impl Eq for RetryEntry {}
impl PartialOrd for RetryEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for RetryEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.retry_at.cmp(&other.0.retry_at)
    }
}

/// `HttpPolicy::ClassState`.
#[derive(Default)]
pub struct ClassState {
    pub ready: VecDeque<Box<RequestOp>>,
    retry: BinaryHeap<RetryEntry>,
    pub options: ClassOptions,
    pub throttle_end: u64,
    pub throttle_left: i64,
    pub request_count: i64,
    pub stall_staging: bool,
}

impl ClassState {
    pub fn retry_len(&self) -> usize {
        self.retry.len()
    }
}

pub struct Policy {
    classes: Vec<ClassState>,
    pub global: GlobalOptions,
}

impl Default for Policy {
    fn default() -> Self {
        Policy::new()
    }
}

impl Policy {
    pub fn new() -> Policy {
        Policy {
            classes: vec![ClassState::default()],
            global: GlobalOptions::default(),
        }
    }

    pub fn class_count(&self) -> usize {
        self.classes.len()
    }

    /// `HttpPolicy::createPolicyClass`.
    pub fn create_class(&mut self) -> u32 {
        let id = self.classes.len();
        if id >= POLICY_CLASS_LIMIT {
            return INVALID_POLICY_ID;
        }
        self.classes.push(ClassState::default());
        id as u32
    }

    pub fn class(&self, pclass: u32) -> &ClassState {
        &self.classes[pclass as usize]
    }

    pub fn class_mut(&mut self, pclass: u32) -> &mut ClassState {
        &mut self.classes[pclass as usize]
    }

    pub fn class_options(&self, pclass: u32) -> &ClassOptions {
        &self.classes[pclass as usize].options
    }

    pub fn class_options_mut(&mut self, pclass: u32) -> &mut ClassOptions {
        &mut self.classes[pclass as usize].options
    }

    /// `HttpPolicy::addOp`: a new request goes to the end of its class's
    /// ready queue with its retry counts reset.
    pub fn add_op(&mut self, mut op: Box<RequestOp>) {
        op.retries = 0;
        op.retries_503 = 0;
        let pclass = op.policy() as usize;
        self.classes[pclass].ready.push_back(op);
    }

    /// `HttpPolicy::retryOp`: schedules the next attempt with exponential
    /// backoff between the request's limits, or at the server's
    /// `Retry-After` delay when it is under 30 seconds.
    pub fn retry_op(&mut self, mut op: Box<RequestOp>, now: u64, hooks: &dyn Hooks) {
        let delta_factor: u64 = if op.retries <= 10 {
            1 << op.retries
        } else {
            1024
        };
        let mut delta = (op.min_backoff.saturating_mul(delta_factor)).min(op.max_backoff);
        let mut external = false;
        if op.retry_after > 0 && op.retry_after < 30 {
            delta = op.retry_after as u64 * 1_000_000;
            external = true;
        }
        op.retry_at = now + delta;
        op.retries += 1;
        if op.status == Status::http(503) {
            op.retries_503 += 1;
        }
        hooks.log(
            LogLevel::Debug,
            &format!(
                "HTTP request {} retry {} scheduled in {} mS ({}).  Status:  {}",
                op.handle(),
                op.retries,
                delta / 1000,
                if external { "external" } else { "internal" },
                op.status.terse()
            ),
        );
        if op.common.tracing > TRACE_OFF {
            hooks.log(
                LogLevel::Info,
                &format!(
                    "TRACE, ToRetryQueue, Handle:  {}, Delta:  {}, Retries:  {}",
                    op.handle(),
                    delta / 1000,
                    op.retries
                ),
            );
        }
        let pclass = op.policy() as usize;
        self.classes[pclass].retry.push(RetryEntry(op));
    }

    /// `HttpPolicy::processReadyQueue`: hands requests to the transport up
    /// to each class's concurrency limit, retries that are due first, under
    /// the class's request-rate throttle.
    pub fn process_ready_queue(
        &mut self,
        transport: &mut Transport,
        now: u64,
        hooks: &dyn Hooks,
    ) -> LoopSpeed {
        let mut result = LoopSpeed::RequestSleep;
        for pclass in 0..self.classes.len() {
            let mut staged: Vec<Box<RequestOp>> = Vec::new();
            {
                let state = &mut self.classes[pclass];
                if state.stall_staging {
                    result = LoopSpeed::Normal;
                    continue;
                }
                if state.retry.is_empty() && state.ready.is_empty() {
                    continue;
                }
                let throttle_enabled = state.options.throttle_rate > 0;
                let throttle_current = throttle_enabled && now < state.throttle_end;
                if throttle_current && state.throttle_left <= 0 {
                    result = LoopSpeed::Normal;
                    continue;
                }
                let active = transport.active_count_in_class(pclass as u32) as i64;
                let active_limit = if state.options.pipelining > 1 {
                    state.options.per_host_connection_limit * state.options.pipelining
                } else {
                    state.options.connection_limit
                };
                let mut needed = active_limit - active;
                if needed > 0 {
                    let mut throttled = false;
                    while needed > 0 && !state.retry.is_empty() {
                        if state.retry.peek().is_none_or(|e| e.0.retry_at > now) {
                            break;
                        }
                        let op = state.retry.pop().map(|e| e.0).expect("non-empty");
                        staged.push(op);
                        state.request_count += 1;
                        needed -= 1;
                        if throttle_enabled && Self::count_throttle(state, now, hooks) {
                            throttled = true;
                            break;
                        }
                    }
                    if !throttled {
                        while needed > 0 && !state.ready.is_empty() {
                            let op = state.ready.pop_front().expect("non-empty");
                            staged.push(op);
                            state.request_count += 1;
                            needed -= 1;
                            if throttle_enabled && Self::count_throttle(state, now, hooks) {
                                break;
                            }
                        }
                    }
                }
                if !state.ready.is_empty() || !state.retry.is_empty() {
                    result = LoopSpeed::Normal;
                }
            }
            for op in staged {
                transport.add_op(op, self, hooks);
            }
        }
        result
    }

    /// The throttle bookkeeping after one request is issued: opens a new
    /// one-second window when the last expired and returns true when the
    /// window's allowance is used up.
    fn count_throttle(state: &mut ClassState, now: u64, hooks: &dyn Hooks) -> bool {
        if now >= state.throttle_end {
            hooks.log(
                LogLevel::Debug,
                &format!(
                    "Throttle expired with {} requests to go and {} requests issued.",
                    state.throttle_left, state.request_count
                ),
            );
            state.throttle_left = state.options.throttle_rate;
            state.throttle_end = now + 1_000_000;
        }
        state.throttle_left -= 1;
        state.throttle_left <= 0
    }

    /// `HttpPolicy::cancel`: removes the request from the retry or ready
    /// queue of its class, returning it for cancellation.
    pub fn cancel(&mut self, handle: u64) -> Option<Box<RequestOp>> {
        for state in &mut self.classes {
            if state.retry.iter().any(|e| e.0.handle() == handle) {
                let mut entries: Vec<RetryEntry> = std::mem::take(&mut state.retry).into_vec();
                let pos = entries.iter().position(|e| e.0.handle() == handle)?;
                let found = entries.remove(pos).0;
                state.retry = entries.into_iter().collect();
                return Some(found);
            }
            if let Some(pos) = state.ready.iter().position(|op| op.handle() == handle) {
                return state.ready.remove(pos);
            }
        }
        None
    }

    /// `HttpPolicy::stageAfterCompletion`: a failed request that may still
    /// be retried goes back to the retry queue (returns `None`); otherwise
    /// the request is returned for delivery.
    pub fn stage_after_completion(
        &mut self,
        op: Box<RequestOp>,
        now: u64,
        hooks: &dyn Hooks,
    ) -> Option<Box<RequestOp>> {
        if !op.status.is_ok() && op.retries < op.retry_limit && op.status.is_retryable() {
            self.retry_op(op, now, hooks);
            return None;
        }
        if !op.status.is_ok() {
            hooks.log(
                LogLevel::Warn,
                &format!(
                    "HTTP request {} failed after {} retries.  Reason:  {} ({})",
                    op.handle(),
                    op.retries,
                    op.status.message(),
                    op.status.terse()
                ),
            );
        } else if op.retries != 0 {
            hooks.log(
                LogLevel::Debug,
                &format!(
                    "HTTP request {} succeeded on retry {}.",
                    op.handle(),
                    op.retries
                ),
            );
        }
        hooks.stat(Stat::ResultCode, op.status.kind as u64);
        Some(op)
    }

    /// `HttpPolicy::getReadyCount`.
    pub fn ready_count(&self, pclass: u32) -> usize {
        self.classes
            .get(pclass as usize)
            .map_or(0, |s| s.ready.len() + s.retry.len())
    }

    /// `HttpPolicy::stallPolicy`: sets the class's stall and returns the
    /// previous value.
    pub fn stall_policy(&mut self, pclass: u32, stall: bool) -> bool {
        match self.classes.get_mut(pclass as usize) {
            Some(s) => std::mem::replace(&mut s.stall_staging, stall),
            None => false,
        }
    }

    /// `HttpPolicy::shutdown`: every queued request, for cancellation.
    pub fn shutdown(&mut self) -> Vec<Box<RequestOp>> {
        let mut out = Vec::new();
        for state in &mut self.classes {
            while let Some(e) = state.retry.pop() {
                out.push(e.0);
            }
            out.extend(state.ready.drain(..));
        }
        out
    }
}

/// The status of a cancelled request.
pub const CANCELED: Status = Status::llcore(he::OP_CANCELED);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::NoHooks;
    use crate::request::{Method, OpCommon, Request};
    use crate::sys::UserPtr;

    fn op(handle: u64) -> Box<RequestOp> {
        Box::new(RequestOp::new(
            OpCommon {
                handle,
                user: UserPtr::null(),
                reply: None,
                tracing: 0,
            },
            Request::new(0, Method::Get, "http://x/"),
        ))
    }

    #[test]
    fn retry_backoff_is_exponential_and_capped() {
        let mut p = Policy::new();
        let h = NoHooks;
        let mut o = op(1);
        o.status = Status::http(503);
        o.min_backoff = 1_000_000;
        o.max_backoff = 5_000_000;
        p.retry_op(o, 100, &h);
        let o = p.cancel(1).unwrap();
        assert_eq!(o.retry_at, 100 + 1_000_000);
        assert_eq!((o.retries, o.retries_503), (1, 1));
        let mut o = o;
        o.status = Status::http(500);
        p.retry_op(o, 100, &h);
        let o = p.cancel(1).unwrap();
        assert_eq!(o.retry_at, 100 + 2_000_000);
        assert_eq!((o.retries, o.retries_503), (2, 1));
        let mut o = o;
        o.retries = 12;
        p.retry_op(o, 0, &h);
        assert_eq!(p.cancel(1).unwrap().retry_at, 5_000_000);
    }

    #[test]
    fn retry_after_overrides_within_30s() {
        let mut p = Policy::new();
        let h = NoHooks;
        let mut o = op(1);
        o.retry_after = 2;
        p.retry_op(o, 0, &h);
        assert_eq!(p.cancel(1).unwrap().retry_at, 2_000_000);
        let mut o = op(2);
        o.retry_after = 30;
        p.retry_op(o, 0, &h);
        assert_eq!(p.cancel(2).unwrap().retry_at, 1_000_000);
    }

    #[test]
    fn stage_after_completion_retries_then_finishes() {
        let mut p = Policy::new();
        let h = NoHooks;
        let mut o = op(1);
        o.status = Status::http(503);
        o.retry_limit = 1;
        assert!(p.stage_after_completion(o, 0, &h).is_none());
        let mut o = p.cancel(1).unwrap();
        o.status = Status::http(503);
        assert!(p.stage_after_completion(o, 0, &h).is_some());
        let mut o = op(2);
        o.status = Status::http(404);
        assert!(p.stage_after_completion(o, 0, &h).is_some());
    }

    #[test]
    fn retry_queue_top_is_latest() {
        let mut p = Policy::new();
        let h = NoHooks;
        let mut a = op(1);
        a.min_backoff = 1_000_000;
        p.retry_op(a, 0, &h);
        let mut b = op(2);
        b.min_backoff = 3_000_000;
        p.retry_op(b, 0, &h);
        let top = p.classes[0].retry.peek().unwrap().0.handle();
        assert_eq!(top, 2);
    }

    #[test]
    fn class_limit() {
        let mut p = Policy::new();
        for i in 1..POLICY_CLASS_LIMIT {
            assert_eq!(p.create_class(), i as u32);
        }
        assert_eq!(p.create_class(), INVALID_POLICY_ID);
    }
}
