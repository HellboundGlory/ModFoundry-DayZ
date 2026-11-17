use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// Maps `items` on scoped threads; results come back in item order, each with its index.
pub(crate) fn parallel_map<T: Sync, R: Send>(
    items: &[T],
    f: impl Fn(&T) -> R + Sync,
) -> Vec<(usize, R)> {
    if items.is_empty() {
        return Vec::new();
    }
    let workers = thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(items.len());
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        local.push((i, f(item)));
                    }
                    local
                })
            })
            .collect();
        let mut all = Vec::with_capacity(items.len());
        for handle in handles {
            all.extend(handle.join().expect("a parallel map thread panicked"));
        }
        all.sort_by_key(|(i, _)| *i);
        all
    })
}
