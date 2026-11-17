use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

struct Inner<T> {
    value: Option<Arc<T>>,
    generation: u64,
}

/// The newest finished load: a load that began before a newer one never replaces it.
pub(crate) struct Latest<T> {
    inner: Mutex<Inner<T>>,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                value: None,
                generation: 0,
            }),
        }
    }
}

impl<T> Latest<T> {
    /// Starts a load; a later `begin` supersedes it.
    pub(crate) fn begin(&self) -> u64 {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.generation
    }

    /// Swaps in a load's result unless a newer load has begun since.
    pub(crate) fn install(&self, generation: u64, value: Arc<T>) -> bool {
        let mut inner = self.lock();
        if inner.generation != generation {
            return false;
        }
        inner.value = Some(value);
        true
    }

    /// The load a newer one hasn't superseded.
    pub(crate) fn get(&self) -> Option<Arc<T>> {
        self.lock().value.clone()
    }

    fn lock(&self) -> MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_superseded_load_never_replaces_the_newer_one() {
        let slot = Latest::<u32>::default();
        let older = slot.begin();
        let newer = slot.begin();
        assert!(slot.install(newer, Arc::new(2)));
        assert!(!slot.install(older, Arc::new(1)));
        assert_eq!(slot.get().as_deref(), Some(&2));
    }
}
