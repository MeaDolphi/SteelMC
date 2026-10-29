//! Off-tick I/O with a non-cancellable, tick-owned completion.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use tokio::task::{JoinError, JoinHandle};

use super::{JobPoll, ServerJob, ServerJobContext, ServerJobQueue};

struct IoCompletionJob<T, F> {
    task: Option<JoinHandle<T>>,
    completion: Option<F>,
}

impl ServerJobQueue {
    /// Runs I/O off-tick and applies its result at a job boundary. Shutdown
    /// joins it before final saves; ordinary job cancellation cannot abandon it.
    pub fn spawn_io<T, F>(&self, work: impl Future<Output = T> + Send + 'static, completion: F)
    where
        T: Send + 'static,
        F: FnOnce(Result<T, JoinError>) + Send + 'static,
    {
        self.spawn(IoCompletionJob {
            task: Some(tokio::spawn(work)),
            completion: Some(completion),
        });
    }
}

impl<T, F> ServerJob for IoCompletionJob<T, F>
where
    T: Send + 'static,
    F: FnOnce(Result<T, JoinError>) + Send + 'static,
{
    fn poll(&mut self, _context: &mut ServerJobContext) -> JobPoll {
        let Some(task) = &mut self.task else {
            return JobPoll::Finished;
        };
        let mut context = Context::from_waker(Waker::noop());
        let Poll::Ready(result) = Pin::new(task).poll(&mut context) else {
            return JobPoll::Pending;
        };
        self.task = None;
        if let Some(completion) = self.completion.take() {
            completion(result);
        }
        JobPoll::Finished
    }

    fn is_cancellable(&self) -> bool {
        false
    }

    fn finish_shutdown(mut self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            if let Some(task) = self.task.take() {
                let result = task.await;
                if let Some(completion) = self.completion.take() {
                    completion(result);
                }
            }
        })
    }
}
