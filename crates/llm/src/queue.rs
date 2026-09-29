//! Calls on background threads: the game submits and polls, never waits.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{Budget, Llm, LlmError, Request, Response};

/// Receipt of a submitted request, to match it with its [`Done`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ticket(pub u64);

/// A finished request.
#[derive(Debug)]
pub struct Done {
    pub ticket: Ticket,
    pub result: Result<Response, LlmError>,
}

struct Job {
    ticket: Ticket,
    request: Request,
}

/// A pool of worker threads running requests against one [`Llm`], inside a
/// [`Budget`]. Results arrive in completion order, not submission order:
/// whoever needs determinism applies them at game times fixed in advance.
///
/// Dropping the queue doesn't wait for calls in flight: the workers finish
/// them and exit, and their answers are dropped.
pub struct LlmQueue {
    jobs: Option<Sender<Job>>,
    done: Receiver<Done>,
    budget: Budget,
    next: u64,
    in_flight: usize,
}

impl LlmQueue {
    /// `workers` parallel calls at most (at least 1).
    pub fn new(llm: Arc<dyn Llm>, workers: usize, budget: Budget) -> Self {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done) = mpsc::channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        for i in 0..workers.max(1) {
            let (llm, job_rx, done_tx) = (llm.clone(), job_rx.clone(), done_tx.clone());
            std::thread::Builder::new()
                .name(format!("llm-{i}"))
                .spawn(move || {
                    loop {
                        // The lock is held only to take the next job.
                        let job = job_rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        let Ok(job) = job else { return };
                        let result = llm.complete(&job.request);
                        let done = Done {
                            ticket: job.ticket,
                            result,
                        };
                        if done_tx.send(done).is_err() {
                            return;
                        }
                    }
                })
                .expect("spawn an LLM worker thread");
        }
        Self {
            jobs: Some(jobs),
            done,
            budget,
            next: 0,
            in_flight: 0,
        }
    }

    /// Queues `request`, or refuses it with [`LlmError::BudgetExceeded`]
    /// (the caller then uses its fallback right away).
    pub fn submit(&mut self, request: Request) -> Result<Ticket, LlmError> {
        if !self.budget.try_spend(Instant::now()) {
            return Err(LlmError::BudgetExceeded);
        }
        let ticket = Ticket(self.next);
        self.next += 1;
        let jobs = self.jobs.as_ref().expect("the queue is alive");
        jobs.send(Job { ticket, request })
            .map_err(|_| LlmError::Transport("LLM workers stopped".to_string()))?;
        self.in_flight += 1;
        Ok(ticket)
    }

    /// The requests finished since the last poll, without waiting.
    pub fn poll(&mut self) -> Vec<Done> {
        let mut out = Vec::new();
        loop {
            match self.done.try_recv() {
                Ok(done) => {
                    self.in_flight -= 1;
                    out.push(done);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return out,
            }
        }
    }

    /// Waits up to `timeout` for the next finished request (tools and tests;
    /// the game uses [`Self::poll`]).
    pub fn wait_next(&mut self, timeout: Duration) -> Option<Done> {
        if self.in_flight == 0 {
            return None;
        }
        match self.done.recv_timeout(timeout) {
            Ok(done) => {
                self.in_flight -= 1;
                Some(done)
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
        }
    }

    /// Requests submitted and not yet returned by [`Self::poll`].
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// Calls still allowed in the current hour.
    pub fn budget_left(&mut self) -> u32 {
        self.budget.remaining(Instant::now())
    }
}

impl Drop for LlmQueue {
    fn drop(&mut self) {
        // Closing the channel stops the workers after their current call.
        self.jobs.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockLlm;

    #[test]
    fn results_come_back_without_blocking() {
        let llm = Arc::new(
            MockLlm::new(|r| Ok(r.messages[1].content.to_uppercase()))
                .with_delay(Duration::from_millis(50)),
        );
        let mut q = LlmQueue::new(llm.clone(), 2, Budget::unlimited());
        let a = q.submit(Request::new("s", "uno")).unwrap();
        let b = q.submit(Request::new("s", "due")).unwrap();
        // Submitting returns at once; nothing is ready yet.
        assert!(q.poll().is_empty());
        assert_eq!(q.in_flight(), 2);
        let mut got = Vec::new();
        while let Some(done) = q.wait_next(Duration::from_secs(5)) {
            got.push((done.ticket, done.result.unwrap().text));
        }
        got.sort();
        assert_eq!(got, [(a, "UNO".to_string()), (b, "DUE".to_string())]);
        assert_eq!(q.in_flight(), 0);
        assert_eq!(llm.calls(), 2);
    }

    #[test]
    fn budget_refuses_extra_calls() {
        let llm = Arc::new(MockLlm::fixed("ok"));
        let mut q = LlmQueue::new(llm.clone(), 1, Budget::per_hour(1));
        assert!(q.submit(Request::new("s", "a")).is_ok());
        assert_eq!(
            q.submit(Request::new("s", "b")),
            Err(LlmError::BudgetExceeded)
        );
        assert_eq!(q.budget_left(), 0);
        let done = q.wait_next(Duration::from_secs(5)).unwrap();
        assert_eq!(done.result.unwrap().text, "ok");
        assert_eq!(llm.calls(), 1);
    }

    #[test]
    fn errors_are_delivered() {
        let llm = Arc::new(MockLlm::scripted([Err(LlmError::Timeout)]));
        let mut q = LlmQueue::new(llm, 1, Budget::unlimited());
        q.submit(Request::new("s", "a")).unwrap();
        let done = q.wait_next(Duration::from_secs(5)).unwrap();
        assert_eq!(done.result, Err(LlmError::Timeout));
    }

    #[test]
    fn dropping_does_not_wait_for_slow_calls() {
        let llm = Arc::new(MockLlm::fixed("tardi").with_delay(Duration::from_secs(2)));
        let mut q = LlmQueue::new(llm, 1, Budget::unlimited());
        q.submit(Request::new("s", "a")).unwrap();
        let start = Instant::now();
        drop(q);
        assert!(start.elapsed() < Duration::from_millis(500));
    }
}
