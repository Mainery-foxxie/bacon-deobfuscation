//! Bounded FIFO worker admission. Reservations survive until cancellation has reaped the worker.
use crate::{
    error::{Error, Result},
    model::{Options, Output},
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
type Notify = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
struct State {
    queued: usize,
    running: usize,
    users: HashMap<u64, Arc<AtomicBool>>,
    closed: bool,
    last: HashMap<u64, Instant>,
    ok: u64,
    failed: u64,
    rejected: u64,
}
pub struct Service {
    workers: usize,
    max_queue: usize,
    cooldown: Duration,
    timeout: Duration,
    slots: Arc<Semaphore>,
    state: Arc<Mutex<State>>,
    drained: Arc<tokio::sync::Notify>,
}
struct Reservation {
    state: Arc<Mutex<State>>,
    drained: Arc<tokio::sync::Notify>,
    stop: Arc<AtomicBool>,
    user: u64,
    running: bool,
    permit: Option<OwnedSemaphorePermit>,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if self.running {
            state.running -= 1;
        } else {
            state.queued -= 1;
        }
        state.users.remove(&self.user);
        if state.users.is_empty() {
            self.drained.notify_waiters();
        }
    }
}
struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Service {
    pub fn new(workers: usize, max_queue: usize, cooldown: Duration, timeout: Duration) -> Self {
        let workers = workers.max(1);
        Self {
            workers,
            max_queue,
            cooldown,
            timeout,
            slots: Arc::new(Semaphore::new(workers)),
            state: Arc::new(Mutex::new(State::default())),
            drained: Arc::new(tokio::sync::Notify::new()),
        }
    }
    pub fn queued(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).queued
    }
    pub fn running(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).running
    }
    pub fn stats(&self) -> serde_json::Value {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::json!({"ok":state.ok,"failed":state.failed,"rejected":state.rejected})
    }
    /// Stop admissions and return after all queued/running jobs have released their workers.
    pub async fn close(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.closed = true;
            self.slots.close();
            for stop in state.users.values() {
                stop.store(true, Ordering::Release);
            }
        }
        loop {
            // Subscribe before checking so the final reservation cannot race this wait.
            let drained = self.drained.notified();
            if self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .users
                .is_empty()
            {
                return;
            }
            drained.await;
        }
    }
    fn reserve(&self, user: u64) -> Result<Reservation> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let reason = if state.closed {
            Some(("internal", "the job service is closed".into()))
        } else if state.users.contains_key(&user) {
            Some(("user_busy", "this user already has a job".into()))
        } else if state.running + state.queued >= self.workers.saturating_add(self.max_queue) {
            Some(("queue_full", "the job queue is full".into()))
        } else {
            state
                .last
                .get(&user)
                .and_then(|last| self.cooldown.checked_sub(last.elapsed()))
                .filter(|d| !d.is_zero())
                .map(|left| {
                    (
                        "cooldown",
                        format!(
                            "wait {:.1} seconds before submitting another job",
                            left.as_secs_f64()
                        ),
                    )
                })
        };
        if let Some((code, message)) = reason {
            state.rejected += 1;
            return Err(Error::new(code, message));
        }
        let stop = Arc::new(AtomicBool::new(false));
        state.users.insert(user, stop.clone());
        state.last.insert(user, Instant::now());
        state.queued += 1;
        // Expired cooldown entries have no further purpose; bound this map across users.
        state.last.retain(|_, last| last.elapsed() < self.cooldown);
        Ok(Reservation {
            state: self.state.clone(),
            drained: self.drained.clone(),
            stop,
            user,
            running: false,
            permit: None,
        })
    }
    pub async fn submit(
        &self,
        user: u64,
        data: Vec<u8>,
        mut opts: Options,
        progress: Option<Notify>,
    ) -> Result<Output> {
        let mut reservation = self.reserve(user)?;
        if self.running() >= self.workers {
            if let Some(note) = &progress {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    note(&format!("queued:{}", self.queued()))
                }));
            }
        }
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::new("internal", "the job service is closed"))?;
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Err(Error::new("internal", "the job service is closed"));
            }
            state.queued -= 1;
            state.running += 1;
        }
        reservation.running = true;
        reservation.permit = Some(permit);
        if opts.sandbox_only {
            opts.trace = false;
            opts.moonveil = false;
        }
        let stop = reservation.stop.clone();
        let _cancel = Cancel(stop.clone());
        let timeout = self.timeout;
        tokio::task::spawn_blocking(move || {
            let _reservation = reservation;
            let note = |stage: &str| {
                if let Some(note) = &progress {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| note(stage)));
                }
            };
            let result = crate::runner::run(&data, &opts, timeout, Some(&note), Some(&stop));
            let mut state = _reservation.state.lock().unwrap_or_else(|e| e.into_inner());
            if result.is_ok() {
                state.ok += 1;
            } else if !stop.load(Ordering::Acquire) {
                state.failed += 1;
            }
            result
        })
        .await
        .map_err(|e| Error::new("internal", format!("job execution failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_users_cooldown_and_cancelled_queue_reservations() {
        let service = Service::new(1, 1, Duration::from_millis(50), Duration::from_secs(1));
        let a = service.reserve(1).unwrap();
        assert_eq!(service.reserve(1).err().unwrap().code, "user_busy");
        let b = service.reserve(2).unwrap();
        assert_eq!(service.reserve(3).err().unwrap().code, "queue_full");
        drop(b);
        drop(a);
        assert_eq!(service.queued(), 0);
        assert_eq!(service.reserve(1).err().unwrap().code, "cooldown");
        std::thread::sleep(Duration::from_millis(60));
        drop(service.reserve(1).unwrap());
        assert_eq!(service.stats()["rejected"], 3);
    }
}
