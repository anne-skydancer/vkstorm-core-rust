//! `HttpService` and `HttpRequestQueue`: the request queue shared between
//! the consumer threads and the service thread, the service thread's loop
//! (request queue, then the policy's ready queues, then libcurl), the
//! policy option interface with its validation, and the life cycle
//! (`createService`, `startThread`, `requestStopThread`, `destroyService`).

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::hooks::{Hooks, LogLevel};
use crate::options::{check_option, PolicyOption, GLOBAL_POLICY_ID, SERVICE_LOOP_SLEEP_NORMAL_MS, TRACE_OFF};
use crate::policy::{LoopSpeed, Policy};
use crate::request::{reply_plain, Op, OpCommon, Reply, ReplyKind, ReplyQueue, Request, RequestOp, SetGetOp};
use crate::status::{he, Status};
use crate::sys::UserPtr;
use crate::transport::{deliver_cancelled, Transport};

/// `HttpService::EState`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum State {
    NotInitialized = -1,
    Initialized = 0,
    Running = 1,
    Stopped = 2,
}

impl State {
    fn from_i32(v: i32) -> State {
        match v {
            0 => State::Initialized,
            1 => State::Running,
            2 => State::Stopped,
            _ => State::NotInitialized,
        }
    }
}

/// Microseconds since the first call: the service's time base for retry
/// scheduling and throttle windows.
pub fn now_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// `HttpRequestQueue`.
struct RequestQueue {
    queue: Mutex<(VecDeque<Op>, bool)>,
    cv: Condvar,
}

impl RequestQueue {
    fn new() -> RequestQueue {
        RequestQueue { queue: Mutex::new((VecDeque::new(), false)), cv: Condvar::new() }
    }

    fn lock(&self) -> MutexGuard<'_, (VecDeque<Op>, bool)> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn add(&self, op: Op) -> Result<(), Op> {
        let wake;
        {
            let mut q = self.lock();
            if q.1 {
                return Err(op);
            }
            wake = q.0.is_empty();
            q.0.push_back(op);
        }
        if wake {
            self.cv.notify_all();
        }
        Ok(())
    }

    fn fetch_all(&self, wait: bool) -> Vec<Op> {
        let mut q = self.lock();
        while q.0.is_empty() {
            if !wait || q.1 {
                return Vec::new();
            }
            q = self.cv.wait(q).unwrap_or_else(|e| e.into_inner());
        }
        q.0.drain(..).collect()
    }

    /// Returns true when the queue was not stopped before.
    fn stop(&self) -> bool {
        let was_running;
        {
            let mut q = self.lock();
            was_running = !q.1;
            q.1 = true;
        }
        self.cv.notify_all();
        was_running
    }
}

/// The state only the service thread touches once it runs.
struct Worker {
    policy: Policy,
    transport: Transport,
}

struct Inner {
    hooks: Arc<dyn Hooks>,
    queue: RequestQueue,
    state: AtomicI32,
    exit_requested: AtomicBool,
    next_handle: AtomicU64,
    /// Taken by the service thread while it runs; back when it stops.
    worker: Mutex<Option<Worker>>,
    last_policy: AtomicI32,
}

impl Inner {
    fn state(&self) -> State {
        State::from_i32(self.state.load(Ordering::Acquire))
    }

    fn set_state(&self, s: State) {
        self.state.store(s as i32, Ordering::Release);
    }

    fn worker(&self) -> MutexGuard<'_, Option<Worker>> {
        self.worker.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The HTTP service: one per process in the viewer, any number in tests.
/// Dropping it is `destroyService`.
pub struct Service {
    inner: Arc<Inner>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Service {
    /// `HttpRequest::createService`. `user_agent` is the `User-Agent`
    /// header value (the viewer's application name).
    pub fn new(hooks: Arc<dyn Hooks>, user_agent: &str) -> Service {
        let worker = Worker { policy: Policy::new(), transport: Transport::new(hooks.clone(), user_agent) };
        let inner = Arc::new(Inner {
            hooks,
            queue: RequestQueue::new(),
            state: AtomicI32::new(State::Initialized as i32),
            exit_requested: AtomicBool::new(false),
            next_handle: AtomicU64::new(1),
            worker: Mutex::new(Some(worker)),
            last_policy: AtomicI32::new(0),
        });
        Service { inner, thread: None }
    }

    pub fn state(&self) -> State {
        self.inner.state()
    }

    /// `HttpService::isStopped`.
    pub fn is_stopped(&self) -> bool {
        self.inner.state() == State::Stopped
    }

    /// `HttpRequest::createPolicyClass`: a new class id, 0 once the
    /// service runs, `INVALID_POLICY_ID` past the class limit.
    pub fn create_policy_class(&self) -> u32 {
        if self.inner.state() == State::Running {
            return 0;
        }
        let mut w = self.inner.worker();
        let Some(worker) = w.as_mut() else {
            return 0;
        };
        let id = worker.policy.create_class();
        if id != crate::options::INVALID_POLICY_ID {
            self.inner.last_policy.store(id as i32, Ordering::Release);
        }
        id
    }

    fn last_policy(&self) -> u32 {
        self.inner.last_policy.load(Ordering::Acquire) as u32
    }

    /// `HttpRequest::setStaticPolicyOption` with a long: refused with
    /// `HE_OPT_NOT_DYNAMIC` while running. Returns the value as stored.
    pub fn set_option_long(&self, opt: i32, pclass: u32, value: i64) -> Result<i64, Status> {
        if self.inner.state() == State::Running {
            return Err(Status::llcore(he::OPT_NOT_DYNAMIC));
        }
        let mut w = self.inner.worker();
        let worker = w.as_mut().ok_or(Status::llcore(he::OPT_NOT_DYNAMIC))?;
        set_long(&mut worker.policy, &mut worker.transport, opt, pclass, value, self.last_policy(), false)
    }

    /// `HttpRequest::setStaticPolicyOption` with a string.
    pub fn set_option_string(&self, opt: i32, pclass: u32, value: &str) -> Result<String, Status> {
        if self.inner.state() == State::Running {
            return Err(Status::llcore(he::OPT_NOT_DYNAMIC));
        }
        let mut w = self.inner.worker();
        let worker = w.as_mut().ok_or(Status::llcore(he::OPT_NOT_DYNAMIC))?;
        set_string(&mut worker.policy, opt, pclass, value, self.last_policy(), false)
    }

    /// `HttpRequest::setStaticPolicyOption` with the SSL verification
    /// callback: the callback lives with the host; the service records
    /// that it is set.
    pub fn set_option_callback(&self, opt: i32, pclass: u32, set: bool) -> Result<bool, Status> {
        if self.inner.state() == State::Running {
            return Err(Status::llcore(he::OPT_NOT_DYNAMIC));
        }
        let mut w = self.inner.worker();
        let worker = w.as_mut().ok_or(Status::llcore(he::OPT_NOT_DYNAMIC))?;
        let opt = check_option(PolicyOption::from_i32(opt), false, pclass, self.last_policy(), true, false)?;
        if pclass != GLOBAL_POLICY_ID {
            return Err(Status::llcore(he::INVALID_ARG));
        }
        let status = worker.policy.global.set_callback(opt, set);
        if !status.is_ok() {
            return Err(status);
        }
        Ok(worker.policy.global.ssl_ctx_callback)
    }

    /// `HttpService::getPolicyOption` with a long (before the thread runs).
    pub fn get_option_long(&self, opt: i32, pclass: u32) -> Result<i64, Status> {
        let mut w = self.inner.worker();
        let worker = w.as_mut().ok_or(Status::llcore(he::INVALID_ARG))?;
        get_long(&worker.policy, opt, pclass, self.last_policy())
    }

    /// `HttpService::getPolicyOption` with a string (before the thread runs).
    pub fn get_option_string(&self, opt: i32, pclass: u32) -> Result<String, Status> {
        let mut w = self.inner.worker();
        let worker = w.as_mut().ok_or(Status::llcore(he::INVALID_ARG))?;
        get_string(&worker.policy, opt, pclass, self.last_policy())
    }

    /// A reply queue for one `HttpRequest` instance.
    pub fn reply_queue(&self) -> Arc<ReplyQueue> {
        ReplyQueue::new()
    }

    /// `HttpRequest::startThread`.
    pub fn start_thread(&mut self) {
        let state = self.inner.state();
        debug_assert!(state == State::Initialized || state == State::Stopped);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        {
            let mut w = self.inner.worker();
            if let Some(worker) = w.as_mut() {
                let count = self.last_policy() as usize + 1;
                if !worker.transport.started() {
                    worker.transport.start(count, &mut worker.policy);
                }
            }
        }
        self.inner.exit_requested.store(false, Ordering::Release);
        self.inner.set_state(State::Running);
        let inner = self.inner.clone();
        self.thread = Some(std::thread::Builder::new().name("HttpService".into()).spawn(move || thread_run(inner)).expect("spawn HttpService thread"));
    }

    fn next_handle(&self) -> u64 {
        self.inner.next_handle.fetch_add(1, Ordering::Relaxed)
    }

    fn common(&self, reply: &Arc<ReplyQueue>, user: UserPtr) -> OpCommon {
        OpCommon { handle: self.next_handle(), user, reply: Some(reply.clone()), tracing: TRACE_OFF }
    }

    fn enqueue(&self, op: Op) -> Result<u64, Status> {
        let handle = op.common().handle;
        match self.inner.queue.add(op) {
            Ok(()) => Ok(handle),
            Err(op) => {
                self.inner.hooks.release_user(op.common().user);
                Err(Status::llcore(he::SHUTTING_DOWN))
            }
        }
    }

    /// `HttpRequest::requestGet` and the other request methods.
    pub fn submit(&self, reply: &Arc<ReplyQueue>, user: UserPtr, req: Request) -> Result<u64, Status> {
        if req.policy > self.last_policy() {
            self.inner.hooks.release_user(user);
            return Err(Status::llcore(he::INVALID_ARG));
        }
        let common = self.common(reply, user);
        self.enqueue(Op::Request(Box::new(RequestOp::new(common, req))))
    }

    /// `HttpRequest::requestCancel`.
    pub fn cancel(&self, reply: &Arc<ReplyQueue>, user: UserPtr, target: u64) -> Result<u64, Status> {
        let common = self.common(reply, user);
        self.enqueue(Op::Cancel { common, target })
    }

    /// `HttpRequest::requestNoOp`.
    pub fn noop(&self, reply: &Arc<ReplyQueue>, user: UserPtr) -> Result<u64, Status> {
        let common = self.common(reply, user);
        self.enqueue(Op::NoOp(common))
    }

    /// `HttpRequest::requestStopThread`.
    pub fn stop_thread(&self, reply: &Arc<ReplyQueue>, user: UserPtr) -> Result<u64, Status> {
        let common = self.common(reply, user);
        self.enqueue(Op::Stop(common))
    }

    /// `HttpRequest::requestSpin`.
    pub fn spin(&self, reply: &Arc<ReplyQueue>, mode: i32) -> Result<u64, Status> {
        let common = self.common(reply, UserPtr::null());
        self.enqueue(Op::Spin { common, mode })
    }

    /// `HttpRequest::setPolicyOption` with a long: the dynamic path,
    /// through the service thread. The pre-checks of
    /// `HttpOpSetGet::setupSet` are applied here.
    pub fn set_option_long_dynamic(&self, reply: &Arc<ReplyQueue>, user: UserPtr, opt: i32, pclass: u32, value: i64) -> Result<u64, Status> {
        let Some(option) = PolicyOption::from_i32(opt) else {
            self.inner.hooks.release_user(user);
            return Err(Status::llcore(he::INVALID_ARG));
        };
        let d = option.descriptor();
        if !d.is_long {
            self.inner.hooks.release_user(user);
            return Err(Status::llcore(he::INVALID_ARG));
        }
        if !d.is_dynamic {
            self.inner.hooks.release_user(user);
            return Err(Status::llcore(he::OPT_NOT_DYNAMIC));
        }
        let common = self.common(reply, user);
        self.enqueue(Op::SetGet(SetGetOp { common, option, pclass, do_set: true, long_value: value, string_value: String::new() }))
    }

    /// `HttpRequest::setPolicyOption` with a string.
    pub fn set_option_string_dynamic(&self, reply: &Arc<ReplyQueue>, user: UserPtr, opt: i32, pclass: u32, value: &str) -> Result<u64, Status> {
        let Some(option) = PolicyOption::from_i32(opt) else {
            self.inner.hooks.release_user(user);
            return Err(Status::llcore(he::INVALID_ARG));
        };
        let d = option.descriptor();
        if d.is_long {
            self.inner.hooks.release_user(user);
            return Err(Status::llcore(he::INVALID_ARG));
        }
        if !d.is_dynamic {
            self.inner.hooks.release_user(user);
            return Err(Status::llcore(he::OPT_NOT_DYNAMIC));
        }
        let common = self.common(reply, user);
        self.enqueue(Op::SetGet(SetGetOp { common, option, pclass, do_set: true, long_value: 0, string_value: value.to_owned() }))
    }

    /// `HttpService::term`: asks a running thread to exit and waits up to
    /// a second for it, as the C++ did.
    pub fn destroy(mut self) {
        self.terminate();
    }

    fn terminate(&mut self) {
        let inner = &self.inner;
        if inner.state() == State::Running {
            inner.exit_requested.store(true, Ordering::Release);
            inner.queue.stop();
            for _ in 0..10 {
                if inner.state() != State::Running {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        if let Some(t) = self.thread.take() {
            if inner.state() != State::Running {
                let _ = t.join();
            } else {
                inner.hooks.log(LogLevel::Warn, "Destroying HttpService with running thread.  Expect problems.");
            }
        }
        // Cancel what was never picked up.
        for op in inner.queue.fetch_all(false) {
            cancel_op(op, &*inner.hooks);
        }
        if let Some(mut worker) = inner.worker().take() {
            shutdown_worker(&mut worker);
        }
        inner.set_state(State::NotInitialized);
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn set_long(policy: &mut Policy, transport: &mut Transport, opt: i32, pclass: u32, value: i64, last_policy: u32, running: bool) -> Result<i64, Status> {
    let opt = check_option(PolicyOption::from_i32(opt), true, pclass, last_policy, true, running)?;
    if pclass == GLOBAL_POLICY_ID {
        let status = policy.global.set_long(opt, value);
        if !status.is_ok() {
            return Err(status);
        }
        policy.global.get_long(opt)
    } else {
        let status = policy.class_options_mut(pclass).set(opt, value);
        if !status.is_ok() {
            return Err(status);
        }
        transport.policy_updated(pclass, policy);
        policy.class_options(pclass).get(opt)
    }
}

fn set_string(policy: &mut Policy, opt: i32, pclass: u32, value: &str, last_policy: u32, running: bool) -> Result<String, Status> {
    let opt = check_option(PolicyOption::from_i32(opt), false, pclass, last_policy, true, running)?;
    if pclass != GLOBAL_POLICY_ID {
        return Err(Status::llcore(he::INVALID_ARG));
    }
    let status = policy.global.set_string(opt, value);
    if !status.is_ok() {
        return Err(status);
    }
    policy.global.get_string(opt)
}

fn get_long(policy: &Policy, opt: i32, pclass: u32, last_policy: u32) -> Result<i64, Status> {
    let opt = check_option(PolicyOption::from_i32(opt), true, pclass, last_policy, false, false)?;
    if pclass == GLOBAL_POLICY_ID {
        policy.global.get_long(opt)
    } else {
        policy.class_options(pclass).get(opt)
    }
}

fn get_string(policy: &Policy, opt: i32, pclass: u32, last_policy: u32) -> Result<String, Status> {
    let opt = check_option(PolicyOption::from_i32(opt), false, pclass, last_policy, false, false)?;
    if pclass != GLOBAL_POLICY_ID {
        return Err(Status::llcore(he::INVALID_ARG));
    }
    policy.global.get_string(opt)
}

/// `HttpOperation::cancel` on an operation still on the request queue:
/// requests are delivered as cancelled; the other operations give no
/// reply (as before).
fn cancel_op(op: Op, hooks: &dyn Hooks) {
    match op {
        Op::Request(r) => deliver_cancelled(r),
        other => hooks.release_user(other.common().user),
    }
}

fn shutdown_worker(worker: &mut Worker) {
    worker.transport.shutdown();
    for op in worker.policy.shutdown() {
        deliver_cancelled(op);
    }
}

/// `HttpService::threadRun`.
fn thread_run(inner: Arc<Inner>) {
    inner.hooks.thread_start();
    let mut worker = match inner.worker().take() {
        Some(w) => w,
        None => {
            inner.hooks.log(LogLevel::Warn, "HttpService thread started without a worker state.");
            inner.set_state(State::Stopped);
            inner.hooks.thread_stop();
            return;
        }
    };
    let mut speed = LoopSpeed::RequestSleep;
    let last_policy = inner.last_policy.load(Ordering::Acquire) as u32;
    while !inner.exit_requested.load(Ordering::Acquire) {
        let r = catch_unwind(AssertUnwindSafe(|| {
            speed = process_request_queue(&inner, &mut worker, speed, last_policy);
            let now = now_us();
            let new_speed = worker.policy.process_ready_queue(&mut worker.transport, now, &*inner.hooks);
            speed = speed.min(new_speed);
            let new_speed = worker.transport.process(&mut worker.policy, now_us());
            speed = speed.min(new_speed);
            if speed != LoopSpeed::RequestSleep {
                std::thread::sleep(Duration::from_millis(SERVICE_LOOP_SLEEP_NORMAL_MS));
            }
        }));
        if let Err(e) = r {
            let what = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            inner.hooks.log(LogLevel::Warn, &format!("HttpService: internal error in the service loop: {what}"));
            speed = LoopSpeed::Normal;
        }
    }
    // HttpService::shutdown
    inner.queue.stop();
    for op in inner.queue.fetch_all(false) {
        cancel_op(op, &*inner.hooks);
    }
    shutdown_worker(&mut worker);
    *inner.worker() = Some(worker);
    inner.set_state(State::Stopped);
    inner.hooks.thread_stop();
}

/// `HttpService::processRequestQueue`: stages every queued operation.
fn process_request_queue(inner: &Arc<Inner>, worker: &mut Worker, speed: LoopSpeed, last_policy: u32) -> LoopSpeed {
    let wait = speed == LoopSpeed::RequestSleep;
    let ops = inner.queue.fetch_all(wait);
    for mut op in ops {
        if inner.exit_requested.load(Ordering::Acquire) {
            cancel_op(op, &*inner.hooks);
            continue;
        }
        let tracing = worker.policy.global.trace as i32;
        {
            let c = op.common_mut();
            c.tracing = c.tracing.max(tracing);
            if c.tracing > TRACE_OFF {
                inner.hooks.log(LogLevel::Info, &format!("TRACE, FromRequestQueue, Handle:  {}", c.handle));
            }
        }
        stage_from_request(inner, worker, op, last_policy);
    }
    LoopSpeed::RequestSleep
}

/// `HttpOperation::stageFromRequest` of every operation kind.
fn stage_from_request(inner: &Arc<Inner>, worker: &mut Worker, op: Op, last_policy: u32) {
    match op {
        Op::Request(r) => worker.policy.add_op(r),
        Op::Cancel { common, target } => {
            let found = cancel_target(worker, target);
            let status = if found { Status::OK } else { Status::llcore(he::HANDLE_NOT_FOUND) };
            reply_plain(&common, ReplyKind::Cancel, status);
        }
        Op::SetGet(s) => {
            let d = s.option.descriptor();
            let (status, long_value, string_value) = if s.do_set {
                if d.is_long {
                    match set_long(&mut worker.policy, &mut worker.transport, s.option as i32, s.pclass, s.long_value, last_policy, true) {
                        Ok(v) => (Status::OK, v, String::new()),
                        Err(e) => (e, 0, String::new()),
                    }
                } else {
                    match set_string(&mut worker.policy, s.option as i32, s.pclass, &s.string_value, last_policy, true) {
                        Ok(v) => (Status::OK, 0, v),
                        Err(e) => (e, 0, String::new()),
                    }
                }
            } else if d.is_long {
                match get_long(&worker.policy, s.option as i32, s.pclass, last_policy) {
                    Ok(v) => (Status::OK, v, String::new()),
                    Err(e) => (e, 0, String::new()),
                }
            } else {
                match get_string(&worker.policy, s.option as i32, s.pclass, last_policy) {
                    Ok(v) => (Status::OK, 0, v),
                    Err(e) => (e, 0, String::new()),
                }
            };
            if let Some(q) = &s.common.reply {
                let mut reply = Reply::plain(&s.common, ReplyKind::SetGet, status);
                reply.long_value = long_value;
                reply.string_value = string_value;
                q.push(reply);
            }
        }
        Op::Stop(common) => {
            inner.exit_requested.store(true, Ordering::Release);
            reply_plain(&common, ReplyKind::Stop, Status::OK);
        }
        Op::NoOp(common) => reply_plain(&common, ReplyKind::NoOp, Status::OK),
        Op::Spin { common, mode } => {
            if mode == 0 {
                loop {
                    std::thread::sleep(Duration::from_millis(100));
                }
            } else {
                std::thread::sleep(Duration::from_millis(1));
                let _ = inner.queue.add(Op::Spin { common, mode });
            }
        }
    }
}

/// `HttpService::cancel`: the policy's queues first, then the transport.
fn cancel_target(worker: &mut Worker, target: u64) -> bool {
    if let Some(op) = worker.policy.cancel(target) {
        deliver_cancelled(op);
        return true;
    }
    worker.transport.cancel(target)
}
