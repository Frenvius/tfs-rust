pub mod dispatcher;
pub mod scheduler;

use std::sync::{Mutex, OnceLock};

use tokio::task::JoinHandle;

use self::dispatcher::Dispatcher;
use self::scheduler::Scheduler;

pub use self::dispatcher::is_dispatcher_thread;

macro_rules! assert_dispatcher_thread {
    () => {
        debug_assert!(
            $crate::runtime::is_dispatcher_thread(),
            "must be called on the dispatcher thread"
        );
    };
}

pub(crate) use assert_dispatcher_thread;

static G_DISPATCHER: OnceLock<Dispatcher> = OnceLock::new();
static G_SCHEDULER: OnceLock<Scheduler> = OnceLock::new();
static G_DISPATCHER_HANDLE: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();

pub fn g_dispatcher() -> &'static Dispatcher {
    G_DISPATCHER.get().expect("dispatcher not initialized")
}

pub fn g_scheduler() -> &'static Scheduler {
    G_SCHEDULER.get().expect("scheduler not initialized")
}

/// Mirror C++ g_dispatcher.join(): waits for the worker task to exit.
/// Call after dispatcher.stop() + dispatcher.shutdown() have been issued.
pub async fn join_dispatcher() {
    let handle = G_DISPATCHER_HANDLE
        .get()
        .and_then(|m| m.lock().unwrap().take());
    if let Some(h) = handle {
        let _ = h.await;
    }
}

pub(crate) fn init_runtime(
    dispatcher: Dispatcher,
    scheduler: Scheduler,
    handle: JoinHandle<()>,
) {
    G_DISPATCHER
        .set(dispatcher)
        .unwrap_or_else(|_| panic!("dispatcher already initialized"));
    G_SCHEDULER
        .set(scheduler)
        .unwrap_or_else(|_| panic!("scheduler already initialized"));
    G_DISPATCHER_HANDLE
        .set(Mutex::new(Some(handle)))
        .unwrap_or_else(|_| panic!("dispatcher handle already stored"));
}
