use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use crate::{StorageError, StorageResult};

const QUEUE_CAPACITY: usize = 256;
const FAIR_SCHEDULE: [usize; 10] = [0, 1, 0, 2, 1, 3, 0, 1, 2, 3];

#[derive(Debug, Clone, Copy)]
pub enum WritePriority {
    HumanSafety = 0,
    FactOrLease = 1,
    PlanningOrCheckpoint = 2,
    Telemetry = 3,
}

impl WritePriority {
    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StateWriterSnapshot {
    pub status: &'static str,
    pub last_command_kind: Option<String>,
    pub queue_depth: i64,
    pub queue_capacity: i64,
    pub admitted_total: u64,
    pub completed_total: u64,
    pub queue_wait_seconds: f64,
    pub last_transaction_seconds: f64,
}

#[derive(Debug, Default)]
struct Metrics {
    running: AtomicBool,
    queue_depth: AtomicI64,
    admitted_total: AtomicU64,
    completed_total: AtomicU64,
    queue_wait_micros: AtomicU64,
    last_transaction_micros: AtomicU64,
    last_command_kind: Mutex<Option<&'static str>>,
}

struct AdmissionRequest {
    priority: WritePriority,
    command_kind: &'static str,
    enqueued_at: Instant,
    admitted: oneshot::Sender<Instant>,
    completed: oneshot::Receiver<()>,
}

#[derive(Clone)]
pub struct StateWriter {
    sender: mpsc::Sender<AdmissionRequest>,
    metrics: Arc<Metrics>,
}

impl std::fmt::Debug for StateWriter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StateWriter")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

pub struct WriteAdmission {
    completed: Option<oneshot::Sender<()>>,
    admitted_at: Instant,
    metrics: Arc<Metrics>,
}

impl Drop for WriteAdmission {
    fn drop(&mut self) {
        self.metrics
            .last_transaction_micros
            .store(micros(self.admitted_at.elapsed()), Ordering::Relaxed);
        self.metrics.completed_total.fetch_add(1, Ordering::Relaxed);
        if let Some(completed) = self.completed.take() {
            let _ = completed.send(());
        }
    }
}

impl StateWriter {
    pub fn spawn() -> Self {
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
        let metrics = Arc::new(Metrics::default());
        metrics.running.store(true, Ordering::Release);
        tokio::spawn(run_state_writer(receiver, metrics.clone()));
        Self { sender, metrics }
    }

    pub async fn admit(
        &self,
        priority: WritePriority,
        command_kind: &'static str,
    ) -> StorageResult<WriteAdmission> {
        let (admitted_tx, admitted_rx) = oneshot::channel();
        let (completed_tx, completed_rx) = oneshot::channel();
        self.metrics.queue_depth.fetch_add(1, Ordering::Relaxed);
        if self
            .sender
            .send(AdmissionRequest {
                priority,
                command_kind,
                enqueued_at: Instant::now(),
                admitted: admitted_tx,
                completed: completed_rx,
            })
            .await
            .is_err()
        {
            self.metrics.queue_depth.fetch_sub(1, Ordering::Relaxed);
            return Err(StorageError::CorruptData(
                "SQLite StateWriter task is unavailable".into(),
            ));
        }
        let admitted_at = admitted_rx.await.map_err(|_| {
            StorageError::CorruptData("SQLite StateWriter admission was cancelled".into())
        })?;
        Ok(WriteAdmission {
            completed: Some(completed_tx),
            admitted_at,
            metrics: self.metrics.clone(),
        })
    }

    pub fn snapshot(&self) -> StateWriterSnapshot {
        StateWriterSnapshot {
            status: if self.metrics.running.load(Ordering::Acquire) {
                "running"
            } else {
                "stopped"
            },
            last_command_kind: self
                .metrics
                .last_command_kind
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .map(str::to_owned),
            queue_depth: self.metrics.queue_depth.load(Ordering::Relaxed),
            queue_capacity: i64::try_from(QUEUE_CAPACITY).unwrap_or(i64::MAX),
            admitted_total: self.metrics.admitted_total.load(Ordering::Relaxed),
            completed_total: self.metrics.completed_total.load(Ordering::Relaxed),
            queue_wait_seconds: Duration::from_micros(
                self.metrics.queue_wait_micros.load(Ordering::Relaxed),
            )
            .as_secs_f64(),
            last_transaction_seconds: Duration::from_micros(
                self.metrics.last_transaction_micros.load(Ordering::Relaxed),
            )
            .as_secs_f64(),
        }
    }
}

async fn run_state_writer(mut receiver: mpsc::Receiver<AdmissionRequest>, metrics: Arc<Metrics>) {
    let mut queues: [VecDeque<AdmissionRequest>; 4] = std::array::from_fn(|_| VecDeque::new());
    let mut schedule_index = 0_usize;
    loop {
        if queues.iter().all(VecDeque::is_empty) {
            let Some(request) = receiver.recv().await else {
                break;
            };
            queues[request.priority.index()].push_back(request);
        }
        while let Ok(request) = receiver.try_recv() {
            queues[request.priority.index()].push_back(request);
        }
        let request = (0..FAIR_SCHEDULE.len()).find_map(|_| {
            let index = FAIR_SCHEDULE[schedule_index % FAIR_SCHEDULE.len()];
            schedule_index = schedule_index.wrapping_add(1);
            queues[index].pop_front()
        });
        let Some(request) = request else {
            continue;
        };
        metrics.queue_depth.fetch_sub(1, Ordering::Relaxed);
        metrics.admitted_total.fetch_add(1, Ordering::Relaxed);
        *metrics
            .last_command_kind
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(request.command_kind);
        metrics
            .queue_wait_micros
            .fetch_add(micros(request.enqueued_at.elapsed()), Ordering::Relaxed);
        let admitted_at = Instant::now();
        if request.admitted.send(admitted_at).is_err() {
            continue;
        }
        let _ = request.completed.await;
    }
    metrics.running.store(false, Ordering::Release);
}

fn micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}
