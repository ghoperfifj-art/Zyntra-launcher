//! Shared state of the single background job (install / launch) shown in the UI.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_LOG: usize = 3000;

#[derive(Default, Clone)]
pub struct State {
    pub stage: String,
    pub error: Option<String>,
    pub log: Vec<String>,
}

#[derive(Default)]
pub struct Task {
    state: Mutex<State>,
    running: AtomicBool,
    /// Total bytes downloaded since start, sampled by the UI for the speed chart.
    pub bytes: AtomicU64,
    done: AtomicU64,
    total: AtomicU64,
}

impl Task {
    /// Marks the task as running. Returns false if another job is already in progress.
    pub fn begin(&self, stage: &str) -> bool {
        if self.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        {
            let mut st = self.state.lock().unwrap();
            st.error = None;
        }
        self.stage(stage);
        true
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn stage(&self, s: impl Into<String>) {
        let s = s.into();
        self.done.store(0, Ordering::SeqCst);
        self.total.store(0, Ordering::SeqCst);
        self.state.lock().unwrap().stage = s.clone();
        self.log(s);
    }

    pub fn log(&self, s: impl Into<String>) {
        let mut st = self.state.lock().unwrap();
        st.log.push(s.into());
        if st.log.len() > MAX_LOG {
            let extra = st.log.len() - MAX_LOG;
            st.log.drain(..extra);
        }
    }

    pub fn set_total(&self, total: u64) {
        self.done.store(0, Ordering::SeqCst);
        self.total.store(total, Ordering::SeqCst);
    }

    pub fn inc(&self) {
        self.done.fetch_add(1, Ordering::SeqCst);
    }

    pub fn add_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
    }

    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::SeqCst);
        (total > 0).then(|| self.done.load(Ordering::SeqCst) as f32 / total as f32)
    }

    pub fn counts(&self) -> (u64, u64) {
        (self.done.load(Ordering::SeqCst), self.total.load(Ordering::SeqCst))
    }

    pub fn finish(&self, msg: impl Into<String>) {
        self.stage(msg);
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn fail(&self, err: String) {
        {
            let mut st = self.state.lock().unwrap();
            st.stage = "Ошибка".into();
            st.error = Some(err.clone());
        }
        self.log(format!("ОШИБКА: {err}"));
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn snapshot(&self) -> State {
        self.state.lock().unwrap().clone()
    }
}
