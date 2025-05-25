use std::sync::atomic::Ordering;

use tokio::sync::{Mutex, MutexGuard};

use super::atomic::Atomic;

pub struct MutexGuardRefWrapper<'a, T: Atomic, K> {
    internal: &'a T,
    pub mutex_guard: MutexGuard<'a, K>,
}

impl<'a, T, K> MutexGuardRefWrapper<'a, T, K>
where
    T: Atomic,
{
    pub fn set(&mut self, new_value: T::Underlying) {
        self.internal.store(new_value, Ordering::Relaxed);
    }

    pub fn get(&self) -> T::Underlying {
        self.internal.load(Ordering::Relaxed)
    }

    pub fn split(&mut self) -> (&T, &mut MutexGuard<'a, K>) {
        (&self.internal, &mut self.mutex_guard)
    }
}

// a version of RWLock that does not block readers from reading while a writer writes
// by providing them with a copy of the internal value
// this is meant to be used with primitives which have cheap Copy implementations
pub struct NonReadBlockingRWLock<T: Atomic, K> {
    internal: T,
    write_mutex: Mutex<K>,
}

impl<T, K> NonReadBlockingRWLock<T, K>
where
    T: Atomic,
{
    pub fn new(initial_value: T::Underlying, mutex_value: K) -> NonReadBlockingRWLock<T, K> {
        NonReadBlockingRWLock {
            internal: T::new(initial_value),
            write_mutex: Mutex::new(mutex_value),
        }
    }

    pub fn get(&self) -> T::Underlying {
        self.internal.load(Ordering::Relaxed)
    }

    pub async fn set_value(&self, new_value: T::Underlying) {
        // acquire lock
        let guard = self.write_mutex.lock().await;

        // update value
        self.internal.store(new_value, Ordering::Relaxed);

        // explicitly drop guard so that it is in scope while setting cell
        drop(guard);
    }

    pub fn try_lock_mut(&self) -> Option<MutexGuardRefWrapper<'_, T, K>> {
        let guard = self.write_mutex.try_lock();

        if let Ok(guard) = guard {
            Some(MutexGuardRefWrapper {
                // need to get a mutable reference to internal without making the
                // function take a mutable reference
                internal: &self.internal,
                mutex_guard: guard,
            })
        } else {
            None
        }
    }

    pub async fn lock_mut(&self) -> MutexGuardRefWrapper<'_, T, K> {
        let guard = self.write_mutex.lock().await;

        MutexGuardRefWrapper {
            // need to get a mutable reference to internal without making the
            // function take a mutable reference
            internal: &self.internal,
            mutex_guard: guard,
        }
    }
}
