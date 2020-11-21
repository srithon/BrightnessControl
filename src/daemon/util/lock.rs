use tokio::{
    sync::{ Mutex, MutexGuard }
};

use std::cell::Cell;

pub struct MutexGuardRefWrapper<'a, T: Copy, K> {
    internal: &'a mut T,
    pub mutex_guard: MutexGuard<'a, K>
}

impl<'a, T, K> MutexGuardRefWrapper<'a, T, K>
where T: Copy {
    pub fn set(&mut self, new_value: T) {
        *self.internal = new_value;
    }
}

// a version of RWLock that does not block readers from reading while a writer writes
// by providing them with a copy of the internal value
// this is meant to be used with primitives which have cheap Copy implementations
pub struct NonReadBlockingRWLock<T: Copy, K> {
    internal: Cell<T>,
    write_mutex: Mutex<K>
}

unsafe impl<T, K> Send for NonReadBlockingRWLock<T, K>
where T: Copy {}

unsafe impl<T, K> Sync for NonReadBlockingRWLock<T, K>
where T: Copy {}

impl<T, K> NonReadBlockingRWLock<T, K>
where T: Copy {
    pub fn new(initial_value: T, mutex_value: K) -> NonReadBlockingRWLock<T, K> {
        NonReadBlockingRWLock {
            internal: Cell::new(initial_value),
            write_mutex: Mutex::new(mutex_value)
        }
    }

    pub fn get(&self) -> T {
        self.internal.get()
    }

    pub async fn set_value(&self, new_value: T) {
        self.write_mutex.lock().await;
        self.internal.set(new_value);
    }

    pub fn try_lock_mut(&self) -> Option<MutexGuardRefWrapper<'_, T, K>> {
        let guard = self.write_mutex.try_lock();

        if let Ok(guard) = guard {
            Some(
                MutexGuardRefWrapper {
                    // need to get a mutable reference to internal without making the
                    // function take a mutable reference
                    internal: unsafe { self.internal.as_ptr().as_mut().unwrap() },
                    mutex_guard: guard
                }
            )
        }
        else {
            None
        }
    }

    pub async fn lock_mut(&self) -> MutexGuardRefWrapper<'_, T, K> {
        let guard = self.write_mutex.lock().await;

        MutexGuardRefWrapper {
            // need to get a mutable reference to internal without making the
            // function take a mutable reference
            internal: unsafe { self.internal.as_ptr().as_mut().unwrap() },
            mutex_guard: guard
        }
    }
}
