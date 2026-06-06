use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;

pub const DISPATCHER_TASK_EXPIRATION: u64 = 2000;

pub struct Task {
    expiration: Option<Instant>,
    pub(crate) func: Box<dyn FnOnce() + Send + 'static>,
}

impl Task {
    pub fn new(f: impl FnOnce() + Send + 'static) -> Self {
        Self {
            expiration: None,
            func: Box::new(f),
        }
    }

    pub fn with_expiration(ms: u64, f: impl FnOnce() + Send + 'static) -> Self {
        Self {
            expiration: Some(Instant::now() + std::time::Duration::from_millis(ms)),
            func: Box::new(f),
        }
    }

    pub(crate) fn from_box(func: Box<dyn FnOnce() + Send + 'static>) -> Self {
        Self {
            expiration: None,
            func,
        }
    }

    pub fn set_dont_expire(&mut self) {
        self.expiration = None;
    }

    fn has_expired(&self) -> bool {
        match self.expiration {
            None => false,
            Some(exp) => exp < Instant::now(),
        }
    }
}

pub(crate) enum DispatcherMsg {
    Run(Task),
    Shutdown,
}

pub struct Dispatcher {
    tx: mpsc::UnboundedSender<DispatcherMsg>,
    cycle: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
}

impl Dispatcher {
    pub fn new() -> (Self, DispatcherWorker) {
        let (tx, rx) = mpsc::unbounded_channel();
        let cycle = Arc::new(AtomicU64::new(0));
        let running = Arc::new(AtomicBool::new(true));
        let dispatcher = Self {
            tx,
            cycle: Arc::clone(&cycle),
            running: Arc::clone(&running),
        };
        let worker = DispatcherWorker { rx, cycle, running };
        (dispatcher, worker)
    }

    /// Mirror C++ addTask: drops the task when state is not RUNNING (i.e. after stop()).
    pub fn add_task(&self, task: Task) {
        if self.running.load(Ordering::Relaxed) {
            let _ = self.tx.send(DispatcherMsg::Run(task));
        }
        // else: drop — matches C++ behavior when THREAD_STATE_CLOSING
    }

    /// Mirror C++ stop(): prevent new tasks from being accepted.
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }

    /// Mirror C++ shutdown(): push a sentinel that terminates the worker loop.
    /// Must be called AFTER stop() so no new tasks race in behind the sentinel.
    pub fn shutdown(&self) {
        let _ = self.tx.send(DispatcherMsg::Shutdown);
    }

    pub fn get_dispatcher_cycle(&self) -> u64 {
        self.cycle.load(Ordering::Relaxed)
    }

    pub(crate) fn sender(&self) -> mpsc::UnboundedSender<DispatcherMsg> {
        self.tx.clone()
    }
}

pub struct DispatcherWorker {
    rx: mpsc::UnboundedReceiver<DispatcherMsg>,
    cycle: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
}

impl DispatcherWorker {
    pub async fn run(mut self) {
        let mut batch: Vec<Task> = Vec::new();
        loop {
            let Some(msg) = self.rx.recv().await else {
                break;
            };
            match msg {
                DispatcherMsg::Shutdown => break,
                DispatcherMsg::Run(task) => batch.push(task),
            }
            while let Ok(msg) = self.rx.try_recv() {
                match msg {
                    DispatcherMsg::Shutdown => {
                        self.execute_batch(&mut batch);
                        return;
                    }
                    DispatcherMsg::Run(task) => batch.push(task),
                }
            }
            self.execute_batch(&mut batch);
        }
        self.running.store(false, Ordering::Relaxed);
    }

    fn execute_batch(&mut self, batch: &mut Vec<Task>) {
        for task in batch.drain(..) {
            if !task.has_expired() {
                self.cycle.fetch_add(1, Ordering::Relaxed);
                (task.func)();
            }
        }
    }
}
