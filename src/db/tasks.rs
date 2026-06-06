use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::{DatabaseEngine, DbResult};

pub type DatabaseResultCallback = Box<dyn FnOnce(Option<DbResult>, bool) + Send + 'static>;
pub type DatabaseCallback = Box<dyn FnOnce() + Send + 'static>;
pub type CallbackDispatcher = Arc<dyn Fn(DatabaseCallback) + Send + Sync>;

enum TaskMessage {
    Query(DatabaseTask),
    Flush(oneshot::Sender<()>),
    Shutdown(oneshot::Sender<()>),
}

pub struct DatabaseTask {
    query: String,
    callback: Option<DatabaseResultCallback>,
    store: bool,
}

pub struct DatabaseTasks {
    sender: mpsc::UnboundedSender<TaskMessage>,
    handle: JoinHandle<()>,
}

impl DatabaseTasks {
    pub fn start<D>(database: Arc<D>, dispatcher: CallbackDispatcher) -> Self
    where
        D: DatabaseEngine + 'static,
    {
        let (sender, mut receiver) = mpsc::unbounded_channel::<TaskMessage>();
        let handle = tokio::spawn(async move {
            while let Some(message) = receiver.recv().await {
                match message {
                    TaskMessage::Query(task) => {
                        run_task(database.as_ref(), dispatcher.clone(), task).await
                    }
                    TaskMessage::Flush(reply) => {
                        let _ = reply.send(());
                    }
                    TaskMessage::Shutdown(reply) => {
                        let _ = reply.send(());
                        break;
                    }
                }
            }
        });

        Self { sender, handle }
    }

    pub fn add_task(&self, query: String, callback: Option<DatabaseResultCallback>, store: bool) {
        let _ = self.sender.send(TaskMessage::Query(DatabaseTask {
            query,
            callback,
            store,
        }));
    }

    pub async fn flush(&self) {
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.send(TaskMessage::Flush(sender));
        let _ = receiver.await;
    }

    pub async fn shutdown(self) {
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.send(TaskMessage::Shutdown(sender));
        let _ = receiver.await;
        let _ = self.handle.await;
    }
}

async fn run_task<D>(database: &D, dispatcher: CallbackDispatcher, task: DatabaseTask)
where
    D: DatabaseEngine + ?Sized,
{
    let (result, success) = if task.store {
        match database.store_query(&task.query).await {
            Ok(result) => (result, true),
            Err(_) => (None, false),
        }
    } else {
        match database.execute(&task.query).await {
            Ok(success) => (None, success),
            Err(_) => (None, false),
        }
    };

    if let Some(callback) = task.callback {
        dispatcher(Box::new(move || callback(result, success)));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use super::{CallbackDispatcher, DatabaseTasks};
    use crate::db::{DatabaseEngine, DatabaseError, DbResult};

    #[derive(Default)]
    struct MockDatabase {
        executed: Mutex<Vec<String>>,
        results: Mutex<VecDeque<Option<DbResult>>>,
    }

    impl DatabaseEngine for MockDatabase {
        fn execute<'a>(
            &'a self,
            query: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<bool, DatabaseError>> + Send + 'a>> {
            Box::pin(async move {
                self.executed
                    .lock()
                    .expect("executed queries should lock")
                    .push(query.to_owned());
                Ok(true)
            })
        }

        fn store_query<'a>(
            &'a self,
            _query: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<DbResult>, DatabaseError>> + Send + 'a>>
        {
            Box::pin(async move {
                Ok(self
                    .results
                    .lock()
                    .expect("results should lock")
                    .pop_front()
                    .unwrap_or(None))
            })
        }

        fn escape_string(&self, value: &str) -> String {
            format!("'{value}'")
        }

        fn escape_blob(&self, value: &[u8]) -> String {
            format!("'{}'", String::from_utf8_lossy(value))
        }

        fn max_packet_size(&self) -> u64 {
            1_048_576
        }
    }

    #[tokio::test]
    async fn add_task_should_execute_queries_and_dispatch_callbacks() {
        let database = Arc::new(MockDatabase::default());
        let hits = Arc::new(Mutex::new(Vec::new()));
        let hits_for_dispatcher = hits.clone();
        let dispatcher: CallbackDispatcher = Arc::new(move |callback| {
            callback();
            hits_for_dispatcher
                .lock()
                .expect("callback hits should lock")
                .push(String::from("callback"));
        });

        let tasks = DatabaseTasks::start(database.clone(), dispatcher);
        let callback_hits = hits.clone();
        tasks.add_task(
            String::from("DELETE FROM players"),
            Some(Box::new(move |_result, success| {
                if success {
                    callback_hits
                        .lock()
                        .expect("callback hits should lock")
                        .push(String::from("success"));
                }
            })),
            false,
        );
        tasks.flush().await;

        assert!(database
            .executed
            .lock()
            .expect("executed queries should lock")
            .iter()
            .any(|query| query == "DELETE FROM players"));
        assert!(hits
            .lock()
            .expect("callback hits should lock")
            .contains(&String::from("success")));

        tasks.shutdown().await;
    }
}
