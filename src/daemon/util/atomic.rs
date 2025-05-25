use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub trait Atomic {
    type Underlying;
    fn new(initial: Self::Underlying) -> Self;
    fn load(&self, ordering: Ordering) -> Self::Underlying;
    fn store(&self, val: Self::Underlying, ordering: Ordering);
}

impl Atomic for AtomicBool {
    type Underlying = bool;
    
    fn new(initial: bool) -> Self {
        AtomicBool::new(initial)
    }
    
    fn load(&self, ordering: Ordering) -> bool {
        self.load(ordering)
    }
    
    fn store(&self, val: bool, ordering: Ordering) {
        self.store(val, ordering);
    }
}

pub struct AtomicF64 {
    inner: AtomicU64,
}

impl Atomic for AtomicF64 {
    type Underlying = f64;
    
    fn new(initial: f64) -> Self {
        Self {
            inner: AtomicU64::new(initial.to_bits()),
        }
    }
    
    fn load(&self, ordering: Ordering) -> f64 {
        f64::from_bits(self.inner.load(ordering))
    }
    
    fn store(&self, val: f64, ordering: Ordering) {
        self.inner.store(val.to_bits(), ordering);
    }
}
