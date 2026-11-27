use super::{turn::fingerprint, SmartConfig, Turn};
use crate::sync_utils::MutexExt;
use serde::Serialize;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{OnceCell, Semaphore};

type Key = [u8; 32];
const KEEP: Duration = Duration::from_secs(600);
const REST: Duration = Duration::from_secs(30);
const CAP: usize = 512;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Trace {
    pub status: &'static str,
    /// One-based indexes only, never messages or the classifier response.
    pub category: Option<usize>,
    pub classifier: Option<usize>,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Decision {
    pub category: Option<usize>,
    pub trace: Trace,
}

impl Decision {
    pub(super) fn new(
        status: &'static str,
        category: Option<usize>,
        classifier: Option<usize>,
    ) -> Self {
        Self {
            category,
            trace: Trace {
                status,
                category: category.map(|i| i + 1),
                classifier: classifier.map(|i| i + 1),
                elapsed_ms: 0,
            },
        }
    }
}

#[derive(Default)]
struct State {
    turns: HashMap<Key, (Instant, Arc<OnceCell<Decision>>)>,
    previous: HashMap<Key, (Instant, Option<usize>)>,
    failed: HashMap<Key, Instant>,
}

pub(crate) struct Engine {
    state: Mutex<State>,
    slots: Semaphore,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            slots: Semaphore::new(12),
        }
    }
}

impl Engine {
    /// The caller owns authorization, request normalization and transport. Cache only
    /// hashes and category indexes. Single-flight is scoped to the app/session/config.
    pub(crate) async fn decide<F, Fut>(
        &self,
        cfg: &SmartConfig,
        app: &str,
        session: impl AsRef<[u8]>,
        turn: &Turn,
        ask: F,
    ) -> Decision
    where
        F: Fn(usize, Option<usize>) -> Fut,
        Fut: Future<Output = Result<Option<usize>, ()>>,
    {
        let now = Instant::now();
        let revision = serde_json::to_vec(cfg).unwrap_or_default();
        let domain = fingerprint(&[app.as_bytes(), &revision]);
        let thread = fingerprint(&[&domain, session.as_ref()]);
        let key = fingerprint(&[&thread, &turn.fingerprint]);
        let (cell, previous) = {
            let mut st = self.state.lock_recover();
            st.turns.retain(|_, v| v.0.elapsed() < KEEP);
            st.previous.retain(|_, v| v.0.elapsed() < KEEP);
            st.failed.retain(|_, at| at.elapsed() < REST);
            let previous = if turn.exact_only {
                None
            } else {
                st.previous.get(&thread).and_then(|v| v.1)
            };
            if turn.within {
                let held = st
                    .turns
                    .get(&key)
                    .and_then(|(_, c)| c.get())
                    .and_then(|d| d.category);
                // Incremental tool-only requests have no historical text to hash.
                let category = if turn.text.is_empty() { previous } else { held };
                return Decision::new(
                    if category.is_some() {
                        "held"
                    } else {
                        "unknownTurn"
                    },
                    category,
                    None,
                );
            }
            if let Some((_, cell)) = st.turns.get(&key) {
                if let Some(previous) = cell.get() {
                    let mut d = previous.clone();
                    d.trace.status = "cached";
                    d.trace.elapsed_ms = 0;
                    return d;
                }
            } else if st.turns.len() >= CAP {
                // Evict completed entries only; never create duplicate in-flight work.
                if let Some(old) = st
                    .turns
                    .iter()
                    .filter(|(_, v)| v.1.initialized() || Arc::strong_count(&v.1) == 1)
                    .min_by_key(|(_, v)| v.0)
                    .map(|(k, _)| *k)
                {
                    st.turns.remove(&old);
                } else {
                    return Decision::new("busy", previous, None);
                }
            }
            let cell = st
                .turns
                .entry(key)
                .or_insert_with(|| (now, Arc::new(OnceCell::new())))
                .1
                .clone();
            (cell, previous)
        };
        let max_wait = Duration::from_millis(cfg.timeout_ms);
        let start = tokio::time::Instant::now();
        let value = cell.get_or_init(|| async {
            let Ok(_permit) = self.slots.try_acquire() else {
                return Decision::new("busy", previous, None);
            };
            let mut decision = Decision::new(
                if previous.is_some() {
                    "previous"
                } else {
                    "unavailable"
                },
                previous,
                None,
            );
            for (idx, target) in cfg.classifiers.iter().enumerate() {
                let rest = fingerprint(&[
                    &domain,
                    target.provider_id.as_bytes(),
                    target.model.as_bytes(),
                ]);
                if self
                    .state
                    .lock_recover()
                    .failed
                    .get(&rest)
                    .is_some_and(|at| at.elapsed() < REST)
                {
                    continue;
                }
                let remaining = max_wait.saturating_sub(start.elapsed());
                if remaining.is_zero() {
                    break;
                }
                let share = remaining / (cfg.classifiers.len() - idx) as u32;
                match tokio::time::timeout(share, ask(idx, previous)).await {
                    Ok(Ok(result)) if result.is_none_or(|i| i < cfg.categories.len()) => {
                        decision = Decision::new(
                            if result.is_some() {
                                "classified"
                            } else {
                                "noMatch"
                            },
                            result,
                            Some(idx),
                        );
                        break;
                    }
                    _ => {
                        let mut st = self.state.lock_recover();
                        if st.failed.len() < CAP {
                            st.failed.insert(rest, Instant::now());
                        }
                    }
                }
            }
            decision.trace.elapsed_ms = now.elapsed().as_millis() as u64;
            let mut st = self.state.lock_recover();
            if st.previous.len() >= CAP && !st.previous.contains_key(&thread) {
                if let Some(k) = st.previous.iter().min_by_key(|(_, v)| v.0).map(|(k, _)| *k) {
                    st.previous.remove(&k);
                }
            }
            if st.previous.get(&thread).is_none_or(|(at, _)| *at <= now) {
                st.previous.insert(thread, (now, decision.category));
            }
            decision
        });
        // A cancelled leader releases its OnceCell. A waiter never inherits a new
        // full timeout: the deadline is from admission of its own request.
        tokio::time::timeout_at(start + max_wait + Duration::from_millis(5), value)
            .await
            .cloned()
            .unwrap_or_else(|_| Decision::new("timeout", previous, None))
    }
}
