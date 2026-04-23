use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Registry of per-env locks. Acquiring a lock for an env id serializes all
/// holders of that same id; different ids run in parallel. Empty entries are
/// evicted when the last holder drops, so the map never grows unboundedly.
#[derive(Clone, Default)]
pub struct LockRegistry {
    inner: Arc<DashMap<String, Arc<Entry>>>,
}

struct Entry {
    mutex: Arc<Mutex<()>>,
    holders: AtomicUsize,
}

pub struct LockGuard {
    _permit: OwnedMutexGuard<()>,
    registry: LockRegistry,
    key: String,
}

impl LockRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn acquire(&self, id: &str) -> LockGuard {
        let entry = self
            .inner
            .entry(id.to_string())
            .or_insert_with(|| {
                Arc::new(Entry {
                    mutex: Arc::new(Mutex::new(())),
                    holders: AtomicUsize::new(0),
                })
            })
            .clone();
        entry.holders.fetch_add(1, SeqCst);
        let mutex = entry.mutex.clone();
        // Drop the dashmap ref before awaiting to avoid holding shard locks.
        drop(entry);
        let permit = mutex.lock_owned().await;
        LockGuard {
            _permit: permit,
            registry: self.clone(),
            key: id.to_string(),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        self.registry
            .inner
            .remove_if(&self.key, |_, entry| {
                let prev = entry.holders.fetch_sub(1, SeqCst);
                prev == 1
            });
    }
}
