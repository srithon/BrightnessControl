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

/// Used in conjunction with ForwardedBrightnessInput
/// Allows the daemon to reuse computed monitor indices after failing to lock the mutex and sending the
/// input over the channel
pub enum BrightnessInputInfo {
    UnProcessed,
    Processed {
        relevant_monitor_indices: Vec<usize>,
    }
}

impl BrightnessInputInfo {
    // dont do any checking here because you are passing in the fields
    pub fn transform_unprocessed(&mut self, relevant_monitor_indices: Vec<usize>) {
        // TODO remove this checking
        if let BrightnessInputInfo::Processed { .. } = self {
            panic!("Tried to transform_unprocessed a Processed BrightnessInputInfo!");
        }

        *self = BrightnessInputInfo::Processed {
            relevant_monitor_indices,
        };
    }

    pub fn is_unprocessed(&self) -> bool {
        matches!(self, BrightnessInputInfo::UnProcessed)
    }
}

/// Struct holding all relevant variables for a client instruction
/// Used to pass off the input to the async task locking the mutex required to process the
/// BrightnessInput
/// This is an alternative to waiting for the mutex to be released and not being guaranteed the
/// lock afterwards
pub struct ForwardedBrightnessInput {
    pub brightness_input: BrightnessInput,
    pub info: BrightnessInputInfo,
    pub socket_message_holder: SocketMessageHolder
}

impl ForwardedBrightnessInput {
    pub fn new_unprocessed(brightness_input: BrightnessInput, socket_message_holder: SocketMessageHolder) -> ForwardedBrightnessInput {
        ForwardedBrightnessInput {
            brightness_input,
            socket_message_holder,
            info: BrightnessInputInfo::UnProcessed
        }
    }

    pub fn new_processed(brightness_input: BrightnessInput, socket_message_holder: SocketMessageHolder, relevant_monitor_indices: Vec<usize>) -> ForwardedBrightnessInput {
        ForwardedBrightnessInput {
            brightness_input,
            socket_message_holder,
            info: BrightnessInputInfo::Processed {
                relevant_monitor_indices,
            }
        }
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
