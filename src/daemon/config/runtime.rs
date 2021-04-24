use super::super::util::{
    lock::*,
    io::*
};

use super::super::super::shared::*;

use tokio::sync::mpsc;

use fnv::FnvHashMap;

use serde::{Serialize, Deserialize};

use std::cell::Cell;

#[derive(Serialize, Deserialize)]
pub struct CachedState {
    pub brightness_states: FnvHashMap<String, f64>,
    pub nightlight: bool,
    pub active_monitor: usize
}

impl Default for CachedState {
    fn default() -> Self {
        CachedState {
            brightness_states: FnvHashMap::default(),
            nightlight: false,
            active_monitor: 0
        }
    }
}

impl CachedState {
    pub fn validate(&self) -> bool {
        self.brightness_states.iter().all(|(_, &brightness)| brightness >= 0.0 && brightness <= 1.0)
    }
}

pub struct BrightnessState {
    // receiver end of channel in mutex
    pub brightness: NonReadBlockingRWLock<f64, mpsc::UnboundedReceiver<(BrightnessInput, SocketMessageHolder)>>,
    pub fade_notifier: mpsc::UnboundedSender<(BrightnessInput, SocketMessageHolder)>,
    pub is_fading: Cell<bool>
}

unsafe impl Send for BrightnessState {}
unsafe impl Sync for BrightnessState {}

impl BrightnessState {
    pub fn new(initial_brightness: f64) -> BrightnessState {
        let (tx, rx) = mpsc::unbounded_channel::<(BrightnessInput, SocketMessageHolder)>();

        BrightnessState {
            brightness: NonReadBlockingRWLock::new(initial_brightness, rx),
            fade_notifier: tx,
            is_fading: Cell::new(false)
        }
    }

    pub fn get(&self) -> f64 {
        self.brightness.get()
    }

    pub fn get_fade_notifier(&self) -> mpsc::UnboundedSender<(BrightnessInput, SocketMessageHolder)> {
        self.fade_notifier.clone()
    }

    pub fn try_lock_brightness(&self) -> Option<MutexGuardRefWrapper<'_, f64, mpsc::UnboundedReceiver<(BrightnessInput, SocketMessageHolder)>>> {
        self.brightness.try_lock_mut()
    }

    pub async fn lock_brightness(&self) -> MutexGuardRefWrapper<'_, f64, mpsc::UnboundedReceiver<(BrightnessInput, SocketMessageHolder)>> {
        self.brightness.lock_mut().await
    }
}
