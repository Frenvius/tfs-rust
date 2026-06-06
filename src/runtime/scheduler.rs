use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use super::dispatcher::{DispatcherMsg, Task};

pub const SCHEDULER_MINTICKS: u32 = 50;

pub struct SchedulerTask {
    pub event_id: u32,
    pub delay: u32,
    func: Box<dyn FnOnce() + Send + 'static>,
}

impl SchedulerTask {
    pub fn new(delay: u32, f: impl FnOnce() + Send + 'static) -> Self {
        Self {
            event_id: 0,
            delay,
            func: Box::new(f),
        }
    }
}

struct Inner {
    timers: Mutex<HashMap<u32, tokio::task::AbortHandle>>,
    dispatcher_tx: mpsc::UnboundedSender<DispatcherMsg>,
    last_event_id: AtomicU32,
}

pub struct Scheduler {
    inner: Arc<Inner>,
}

impl Scheduler {
    pub(crate) fn new(dispatcher_tx: mpsc::UnboundedSender<DispatcherMsg>) -> Self {
        Self {
            inner: Arc::new(Inner {
                timers: Mutex::new(HashMap::new()),
                dispatcher_tx,
                last_event_id: AtomicU32::new(0),
            }),
        }
    }

    pub fn add_event(&self, mut task: SchedulerTask) -> u32 {
        if task.event_id == 0 {
            task.event_id = self.inner.last_event_id.fetch_add(1, Ordering::Relaxed) + 1;
        }
        let event_id = task.event_id;
        let delay = task.delay;
        let func = task.func;

        let inner = Arc::clone(&self.inner);
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay as u64)).await;
            inner.timers.lock().unwrap().remove(&event_id);
            let _ = inner
                .dispatcher_tx
                .send(DispatcherMsg::Run(Task::from_box(func)));
        });

        self.inner
            .timers
            .lock()
            .unwrap()
            .insert(event_id, handle.abort_handle());

        event_id
    }

    pub fn stop_event(&self, event_id: u32) {
        if event_id == 0 {
            return;
        }
        if let Some(handle) = self.inner.timers.lock().unwrap().remove(&event_id) {
            handle.abort();
        }
    }

    pub fn shutdown(&self) {
        let mut timers = self.inner.timers.lock().unwrap();
        for (_, handle) in timers.drain() {
            handle.abort();
        }
    }
}
