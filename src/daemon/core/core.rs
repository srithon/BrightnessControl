use bincode::Options;
use daemonize::Daemonize;

use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    process::Command,
    runtime::{self, Runtime},
    select,
    sync::{mpsc, RwLock},
    task::JoinError,
    time, try_join,
};

use tokio_stream::{wrappers::UnixListenerStream, StreamExt};

use std::{io::{Error, ErrorKind, Result}, sync::atomic::Ordering};

use std::cell::Cell;
use std::sync::Arc;

use std::collections::{BTreeMap, VecDeque};

use futures::stream::{FuturesOrdered, FuturesUnordered};

use signal_hook::consts::*;
use signal_hook_tokio::Signals;

use crate::{
    daemon::{
        config::{persistent::*, runtime::*},
        core::display::*,
        fs::*,
        util::{atomic::{Atomic, AtomicNumber}, io::*},
    },
    shared::*,
};

tokio::task_local! {
    static SOCKET_MESSAGE_HOLDER: SocketMessageHolder;
}

enum ProcessInputExitCode {
    Normal,
    Shutdown,
}

// separate RwLocks so they can be modified concurrently
struct Daemon {
    // these are primitives, so it doesn't matter
    monitor_states: CollectiveMonitorState,
    config: RwLock<DaemonOptions>,
    file_utils: FileUtils,
}

impl Daemon {
    async fn run(self) -> Result<()> {
        self.refresh_configuration().await?;

        register_sigterm_handler()?;

        let listener = match UnixListener::bind(SOCKET_PATH) {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("Error binding listener: {e}");
                fs::remove_file(SOCKET_PATH).await?;
                UnixListener::bind(SOCKET_PATH)?
            }
        };

        // this is used as a shutdown signal
        // if any message is sent over the channel, the dameon starts shutting down
        let (tx, mut rx) = mpsc::channel::<()>(30);

        println!(
            "{}",
            self
                .monitor_states
                .get_formatted_display_states(Some(&MonitorOverride::All))
                .await
        );

        try_join!(
            async move {
                let mut listener_stream = UnixListenerStream::new(listener);

                let self_arc = Arc::new(self);

                while let Some(stream) = listener_stream.next().await {
                    println!("Stream!");

                    let daemon = self_arc.clone();

                    let shutdown_channel = tx.clone();

                    let local_set = tokio::task::LocalSet::new();
                    fn local_run<T: 'static, U: Future<Output = T> + 'static>(
                        local_set: &tokio::task::LocalSet,
                        closure: U,
                    ) -> impl Future<Output = std::result::Result<T, JoinError>> + use<'_, T, U>
                    {
                        local_set.run_until(async move { tokio::task::spawn_local(closure).await })
                    }

                    let result = local_run(&local_set, async move {
                        match stream {
                            Ok(mut stream) => {
                                // Rust is amazing
                                // the compiler figured out the type of program_input based on the call to
                                // daemon.process_input 5 lines below

                                // https://github.com/bincode-org/bincode
                                // "The size of the encoded object will be the same or smaller than the size that the object takes up in memory in a running Rust program."
                                const STREAM_BUFFER_SIZE: usize =
                                    std::mem::size_of::<ProgramInput>();
                                let mut stream_buffer: [u8; STREAM_BUFFER_SIZE] =
                                    [0; STREAM_BUFFER_SIZE];

                                let num_bytes: usize = stream.read(&mut stream_buffer[..]).await?;

                                let program_input =
                                    BINCODE_OPTIONS.deserialize_from(&stream_buffer[..num_bytes]);
                                match program_input {
                                    Ok(program_input) => {
                                        println!("Deserialized ProgramInput: {program_input:?}");
                                        let res = daemon
                                            .process_input(
                                                program_input,
                                                SocketMessageHolder::new(stream),
                                            )
                                            .await;
                                        if let ProcessInputExitCode::Shutdown = res {
                                            // leave the loop
                                            // TODO see if you can just break
                                            match shutdown_channel.send(()).await {
                                                Ok(_) => println!("Sent shutdown signal!"),
                                                Err(e) => {
                                                    eprintln!("Failed to send shutdown signal! {e}")
                                                }
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!("Error deserializing: {err}");
                                    }
                                }
                            }
                            Err(_) => match shutdown_channel.send(()).await {
                                Ok(_) => println!("Sent shutdown signal!"),
                                Err(e) => eprintln!("Failed to send shutdown signal! {e}"),
                            },
                        }

                        std::io::Result::Ok(())
                    })
                    .await?;

                    if let Err(e) = result {
                        eprintln!("Error processing input: {e}");
                    }
                }

                Ok(())
            },
            async move {
                rx.recv().await;
                std::io::Result::<()>::Err(Error::new(
                    ErrorKind::ConnectionAborted,
                    "Shutting down daemon!",
                ))
            }
        )?;

        println!("Successfully exitting run function");

        Ok(())
    }

    fn start(self, tokio_runtime: Runtime) {
        println!("{:?}", tokio_runtime.block_on(self.run()));
        tokio_runtime.shutdown_timeout(std::time::Duration::from_millis(1000));
        let _ = std::fs::remove_file(SOCKET_PATH);
        println!("Shutdown tokio runtime!");
    }
}

impl Daemon {
    async fn new(file_utils: FileUtils) -> Result<Daemon> {
        file_utils.update_config_template().await?;

        let config: DaemonOptions = {
            let (mut config_file, file_existed) = file_utils.open_configuration_file().await?;

            let configuration: DaemonOptions = async move {
                // file exists
                if file_existed {
                    match get_configuration_from_file(&mut config_file).await {
                        Err(e) => {
                            eprintln!("Error getting configuration from file (initial): {e}")
                        }
                        // rewrapping Result with different Err type
                        Ok(c) => return Ok(c),
                    }
                } else {
                    overwrite_file_with_content(&mut config_file, CONFIG_TEMPLATE).await?;
                }

                let config = DaemonOptions::default();

                // saves creating another instance of DaemonOptions::default()
                std::io::Result::<DaemonOptions>::Ok(config)
            }
            .await?;

            configuration
        };

        println!("Loaded configuration: {config:?}");

        let monitor_states = {
            let cached_state = file_utils.get_cached_state().await?;

            CollectiveMonitorState::new(cached_state.active_monitor, cached_state.brightness_states)
                .await
        };

        let config = RwLock::new(config);

        Ok(Daemon {
            monitor_states,
            config,
            file_utils,
        })
    }

    async fn save_configuration(&self) -> Result<()> {
        let monitor_states = self.monitor_states.read().await;

        let iterator = monitor_states.iter_all_monitor_states();

        let mut map = fnv::FnvHashMap::default();
        for monitor_state in iterator {
            let brightness_state = monitor_state.get_brightness_state();

            let brightness_state_internal = BrightnessStateInternal {
                brightness: brightness_state.get(),
                nightlight: brightness_state.nightlight.get(),
            };

            map.insert(
                monitor_state.get_monitor_name().to_owned(),
                brightness_state_internal,
            );
        }

        let cached_state = CachedState {
            brightness_states: map,
            active_monitor: *monitor_states.get_active_monitor_index(),
        };

        let res = try_join!(self.file_utils.write_cached_state(&cached_state));

        match res {
            Ok(_) => Ok(()),
            // this unpacks std::io::Result<( (), (), () )>
            // and repacks it into a std::io::Result<( () )>
            Err(e) => Err(e),
        }
    }

    async fn refresh_brightness_all(&self) -> Result<bool> {
        self.refresh_brightness(
            self.monitor_states
                .read()
                .await
                .get_monitor_override_indices(&MonitorOverride::Enabled)
                .into_iter(),
            true,
        )
        .await
    }

    // boolean signals whether the function removed any monitors from self.displays
    async fn refresh_brightness(
        &self,
        monitors: impl Iterator<Item = usize>,
        auto_remove_displays: bool,
    ) -> Result<bool> {
        let commands = self.create_xrandr_commands(monitors).await;

        if auto_remove_displays {
            // we use FuturesOrdered because we want the output to line up with
            // active_monitor_indices_iterator, so we can identify the indices of the displays
            // which threw an error
            let (active_monitor_indices, mut command_handles): (Vec<usize>, Vec<_>) = commands
                .into_iter()
                .filter_map(|(index, mut command)| {
                    if let Ok(call_handle) = command.spawn() {
                        Some((index, call_handle))
                    } else {
                        None
                    }
                })
                .rev()
                .unzip();

            let mut ordered_futures = command_handles
                .iter_mut()
                .map(|handle| handle.wait())
                .collect::<FuturesOrdered<_>>();

            let mut removed_display = false;

            let mut active_monitor_indices_iterator = active_monitor_indices.into_iter();
            while let Some(exit_status) = ordered_futures.next().await {
                // guaranteed to work
                let index = active_monitor_indices_iterator.next().unwrap();

                let exit_status = exit_status?;

                // if the call fails, then the configuration is no longer valid
                // remove the monitor from "displays"
                if !exit_status.success() {
                    // remove index'th display from list
                    let mut displays_write_guard = self.monitor_states.write().await;
                    displays_write_guard.remove_enabled_monitor_by_index(&index);

                    removed_display = true;
                }
            }

            Ok(removed_display)
        } else {
            // wait for it on its own
            for (_, mut command) in commands {
                let mut call_handle = command.spawn()?;
                tokio::spawn(async move { call_handle.wait().await });
            }

            Ok(false)
        }
    }

    async fn refresh_configuration(&self) -> Result<()> {
        // don't need the early return flag here
        let _ = self.refresh_brightness_all().await?;

        Ok(())
    }

    async fn process_input(
        &self,
        program_input: ProgramInput,
        mut socket_holder: SocketMessageHolder,
    ) -> ProcessInputExitCode {
        match program_input {
            ProgramInput::ToggleNightlight(monitor_override) => {
                let monitor_override = {
                    if let Some(monitor_override) = monitor_override {
                        monitor_override
                    } else {
                        self.config.read().await.monitor_default_behavior.clone()
                    }
                };

                let monitor_override_indices = self
                    .monitor_states
                    .read()
                    .await
                    .get_monitor_override_indices(&monitor_override);

                // return error if monitor_override_indices is empty
                if monitor_override_indices.is_empty() {
                    socket_holder.queue_error("No monitors found to toggle nightlight");
                    return ProcessInputExitCode::Normal;
                }

                for &monitor_index in &monitor_override_indices {
                    let monitor_state_guard = self.monitor_states.read().await;

                    let monitor_state = monitor_state_guard
                        .get_monitor_state_by_index(monitor_index)
                        .expect("Index must exist");
                    let brightness_state = monitor_state.get_brightness_state();

                    // NOTE: we want the nightlight state to be locked until we are done refreshing the
                    // nightlight state
                    let mut nightlight_lock = brightness_state.nightlight.lock_mut().await;

                    // invert nightlight state
                    let original_nightlight_state = nightlight_lock.get();
                    let new_nightlight_state = !original_nightlight_state;

                    nightlight_lock.set(new_nightlight_state);

                    // janky alternative to an async closure
                    // this allows us to early-return from this block
                    // can't use a regular closure because then we wouldnt be able to use async/await
                    // inside of it
                    // can't use an async block inside of a regular one because then we would need to move all the
                    // captured variables into the block
                    // this is ugly but it works
                    #[allow(clippy::never_loop)]
                    loop {
                        if let Err(e) = self
                            .refresh_brightness(monitor_override_indices.iter().copied(), true)
                            .await
                        {
                            socket_holder.queue_error(format!("Failed to refresh xrandr: {e}"));
                            break;
                        }

                        // could have used format! to make this a one-liner, but this allows the strings to be
                        // stored in static memory instead of having to be generated at runtime
                        if new_nightlight_state {
                            socket_holder.queue_success("Enabled nightlight");
                        } else {
                            socket_holder.queue_success("Disabled nightlight");
                        }

                        break;
                    }
                }
            }
            ProgramInput::Get(property) => {
                // TODO create macro that yields an iterator over MonitorState's
                let property_value = match property {
                    GetProperty::Brightness(optional_monitor_override) => {
                        self.monitor_states
                            .get_formatted_display_states(optional_monitor_override.as_ref())
                            .await
                    }
                    GetProperty::Displays => {
                        // TODO take a MonitorOverride for this one too
                        self.monitor_states
                            .get_formatted_display_names(Some(&MonitorOverride::All))
                            .await
                    }
                    GetProperty::Mode(optional_monitor_override) => {
                        self.monitor_states
                            .get_formatted_display_states_with_format(
                                optional_monitor_override.as_ref(),
                                |monitor_state| {
                                    format!(
                                        "{}",
                                        monitor_state.brightness_state.nightlight.get() as i32
                                    )
                                },
                            )
                            .await
                    }
                    GetProperty::Config => {
                        format!("{:?}", *self.config.read().await)
                    }
                    GetProperty::IsFading(optional_monitor_override) => {
                        // return "1" if currently fading
                        // else "0"
                        self.monitor_states
                            .get_formatted_display_states_with_format(
                                optional_monitor_override.as_ref(),
                                |monitor_state| {
                                    let is_fading = monitor_state.brightness_state.is_fading.get();

                                    if is_fading {
                                        "1"
                                    } else {
                                        "0"
                                    }
                                },
                            )
                            .await
                    }
                    GetProperty::ActiveMonitor => {
                        let monitors = self.monitor_states.read().await;
                        let active_index = *monitors.get_active_monitor_index();
                        let active_monitor_state =
                            monitors.get_monitor_state_by_index(active_index).unwrap();
                        active_monitor_state.get_monitor_name().to_string()
                    }
                };

                socket_holder.queue_success(property_value);
            }
            ProgramInput::ConfigureDisplay => {
                if let Err(e) = self.reconfigure_displays().await {
                    socket_holder.queue_error(format!("Failed to reconfigure displays: {e}"));
                } else {
                    socket_holder.queue_success("Successfully reconfigured displays!");
                }
            }
            ProgramInput::ReloadConfiguration => {
                match self.file_utils.open_configuration_file().await {
                    Ok((mut configuration_file, _)) => {
                        let config_result = get_configuration_from_file(&mut configuration_file);

                        match config_result.await {
                            Result::<DaemonOptions>::Ok(config) => {
                                *self.config.write().await = config;

                                socket_holder.queue_success("Successfully reloaded configuration!");
                            }
                            Err(error) => {
                                socket_holder.queue_error(format!(
                                    "Failed to parse configuration file: {error}",
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        socket_holder.queue_error(format!(
                            "Failed to open configuration file for reloading: {e}",
                        ));
                    }
                }
            }
            ProgramInput::Shutdown => {
                if let Err(e) = self.save_configuration().await {
                    socket_holder.queue_error(format!("Failed to save configuration: {e}"));
                } else {
                    socket_holder.queue_success("Successfully saved configuration!");
                }

                return ProcessInputExitCode::Shutdown;
            }
            ProgramInput::Brightness(brightness_change) => {
                if brightness_change.is_active() {
                    // let process_brightness_input handle the socket holder
                    self.process_brightness_input(brightness_change, socket_holder)
                        .await;

                    // early return so we don't double-move the socket holder
                    return ProcessInputExitCode::Normal;
                }
            }
            ProgramInput::ChangeActiveMonitor(active_monitor_change) => {
                match active_monitor_change {
                    ActiveMonitorChange::SetActive(new_monitor) => {
                        let mut monitor_states_guard = self.monitor_states.write().await;

                        // Result<monitor_name: &String, error_message: &'static str>
                        let result = {
                            if let Ok(index) = new_monitor.parse::<usize>() {
                                let result =
                                    monitor_states_guard.set_active_monitor_by_index(index);

                                // if set_active_monitor_by_index is successful, set the thing to
                                // the name of the thing
                                result.map(|_| monitor_states_guard.get_monitor_state_by_index(index).expect("Monitor state is None even though it was just set to the active monitor").get_monitor_name())
                            } else {
                                let result =
                                    monitor_states_guard.set_active_monitor_by_name(&new_monitor);
                                result.map(|_| &new_monitor)
                            }
                        };

                        match result {
                            Ok(monitor_name) => socket_holder
                                .queue_success(format!("Set active monitor to {monitor_name}!")),
                            Err(message) => socket_holder.queue_error(message),
                        }
                    }
                }
            }
        };

        socket_holder.consume();

        ProcessInputExitCode::Normal
    }

    async fn process_brightness_input(
        &self,
        brightness_change: BrightnessInput,
        socket_holder: SocketMessageHolder,
    ) {
        println!("In processbrigtnesinput!");

        // push to this queue whenever new input comes in
        let mut inputs = VecDeque::<ForwardedBrightnessInput>::with_capacity(2);
        inputs.push_back(ForwardedBrightnessInput::new_unprocessed(
            brightness_change,
            socket_holder,
        ));

        struct BrightnessChangeInfo {
            brightness_step: f64,
            end_brightness: f64,
            fade: bool,
        }

        struct MonitorInfo<'a> {
            current_brightness: BrightnessGuard<'a>,
            is_enabled: bool,
            is_fading: &'a Cell<bool>,
            brightness_change_info: BrightnessChangeInfo,
        }

        // use the read() and write() functions
        // this used to be on the inside of the loop so we would have to retake the lock every time
        // should we do it like that again?
        let monitor_states_guard = self.monitor_states.read().await;

        // NOTE: with a new Rust update, this declaration has to come after monitor_states_guard
        // so that they can be dropped in the same order as they were created.

        // map between monitor index and BrightnessChangeInfo
        let mut intermediate_brightness_states: BTreeMap<usize, MonitorInfo> = BTreeMap::new();

        // process inputs queue
        'base_loop: loop {
            let head = inputs.pop_front();

            if head.is_none() {
                break;
            }

            let ForwardedBrightnessInput {
                brightness_input,
                mut socket_message_holder,
                mut info,
            } = head.unwrap();

            if info.is_unprocessed() {
                let monitor_indices = {
                    let config = self.config.read().await;

                    let override_monitor = {
                        if let Some(ref override_monitor) = &brightness_input.override_monitor {
                            override_monitor
                        } else {
                            &config.monitor_default_behavior
                        }
                    };

                    monitor_states_guard.get_monitor_override_indices(override_monitor)
                };

                println!("Monitor indices: {monitor_indices:#?}");

                info.transform_unprocessed(monitor_indices);
            }

            let relevant_monitor_indices = match info {
                BrightnessInputInfo::Processed {
                    relevant_monitor_indices,
                } => relevant_monitor_indices,
                _ => unreachable!("Info should have been transformed!"),
            };

            // return error if relevant_monitor_indices is empty
            if relevant_monitor_indices.is_empty() {
                socket_message_holder.queue_error("No monitors found to change brightness");
                socket_message_holder.consume();
                continue;
            }

            // Printing out the starting brightness for all affected monitors
            {
                let mut using_disconnected_monitor = false;

                for &i in relevant_monitor_indices.iter() {
                    let f = monitor_states_guard.get_monitor_state_by_index(i).unwrap();

                    if !monitor_states_guard.is_monitor_index_enabled(i) {
                        socket_message_holder.queue_error(format!(
                            "Monitor {} is not connected",
                            f.get_monitor_name()
                        ));

                        using_disconnected_monitor = true;
                    }

                    println!(
                        "Original Brightness: {}",
                        f.get_brightness_state().brightness.get()
                    );
                }

                if using_disconnected_monitor {
                    socket_message_holder.consume();
                    continue;
                }
            }

            // remove all irrelevant intermediate_brightness_states
            // this releases the mutices associated with them
            // TODO can we assume that relevant_monitor_indices is sorted?
            // TODO improve complexity
            // TODO consider using a BTreeSet for relevant_monitor_indices?
            {
                let keys_to_remove: Vec<_> = intermediate_brightness_states
                    .keys()
                    .copied()
                    .filter(|index| !relevant_monitor_indices.contains(index))
                    .collect();

                for key in keys_to_remove {
                    intermediate_brightness_states.remove(&key);
                }
            }

            let optional_guards: Vec<(usize, Option<BrightnessGuard>)> = relevant_monitor_indices
                .iter()
                .map(|monitor_index| {
                    (
                        monitor_index,
                        intermediate_brightness_states.get(monitor_index),
                    )
                })
                // take only the ones not present in intermediate_brightness_states
                .filter(|(_, optional_monitor_info)| optional_monitor_info.is_none())
                // try to take the brightness guard from monitor_states_guard
                .map(|(&index, _)| {
                    (
                        index,
                        monitor_states_guard
                            .get_monitor_state_by_index(index)
                            .expect("monitor state not in vector")
                            .get_brightness_state()
                            .try_lock_brightness(),
                    )
                })
                .collect();

            // TODO make this random so there is no preference towards those in the beginning of
            // the list
            // this finds the first brightness guard that we could not secure
            let first_none_guard_index = optional_guards
                .iter()
                .find(|(_, monitor_info)| monitor_info.is_none())
                .map(|(i, _)| *i);

            if let Some(index) = first_none_guard_index {
                // pass everything along to the owner of the receiver
                // someone else has the lock
                // they may be fading
                // try sending input over the mpsc channel
                // respond?
                let brightness_state = monitor_states_guard
                    .get_monitor_state_by_index(index)
                    .expect("Monitor state found from find() not in the list?")
                    .get_brightness_state();

                let send_channel = brightness_state.get_fade_notifier();

                let forwarded_brightness_input = ForwardedBrightnessInput::new_processed(
                    brightness_input,
                    socket_message_holder,
                    relevant_monitor_indices,
                );

                let _ = send_channel.send(forwarded_brightness_input);

                // send all the ones that are queued
                // theoretically this should always be empty
                for input in inputs.into_iter() {
                    let _ = send_channel.send(input);
                }

                return;
            }

            // this inserts everything in optional_guards into intermediate_brightness_states
            // later on, we update the brightness_change_info for each one according to the input
            for (monitor_index, guard) in optional_guards {
                // we made sure they were all Some using first_none_guard_index
                let guard = guard.unwrap();

                let brightness_state = monitor_states_guard
                    .get_monitor_state_by_index(monitor_index)
                    .expect("monitor state index not found")
                    .get_brightness_state();

                // we do not need to put meaningless default values into the BrightnessChangeInfo
                // that we insert
                // instead, we set it equal to uninitialized memory
                // we need to refresh the field values in the following loop
                // it doesn't make sense to do it here as well
                macro_rules! garbage {
                    () => {{
                        #[allow(invalid_value)]
                        unsafe {
                            std::mem::MaybeUninit::<_>::uninit().assume_init()
                        }
                    }};
                }

                // we initialize the current_brightness and is_fading fields because all of the other ones
                // will be overwritten
                // is_fading contains a REFERENCE to monitor_states_guard, so we do not need to
                // update it after its initial initialization
                // therefore, we can initialize it here and write to it again
                let partially_initialized_monitor_info = MonitorInfo {
                    current_brightness: guard,
                    is_fading: &brightness_state.is_fading,

                    // GARBAGE DEFAULT VALUES
                    is_enabled: garbage!(),
                    brightness_change_info: garbage!(),
                };

                // add each one to intermediate_brightness_states
                intermediate_brightness_states
                    .insert(monitor_index, partially_initialized_monitor_info);
            }

            // TODO don't clone this
            let fade_options = &self.config.read().await.fade_options.clone();

            // fade
            let total_num_steps = fade_options.total_duration / fade_options.step_duration;

            // populate intermediate_brightness_states with current brightnesses
            intermediate_brightness_states
                .iter_mut()
                .for_each(|(&monitor_index, monitor_info)| {
                    println!("Iterating intermediate_brightness_states! {monitor_index}");

                    let brightness_guard = &monitor_info.current_brightness;

                    let current_brightness = brightness_guard.get();

                    let new_brightness = {
                        let integer_representation = match brightness_input.brightness {
                            Some(BrightnessChange::Set(new_brightness)) => new_brightness,
                            Some(BrightnessChange::Adjustment(brightness_shift)) => {
                                (brightness_shift as i16 + current_brightness as i16).clamp(10, 100)
                                    as u8
                            }
                            None => current_brightness as u8,
                        };

                        integer_representation as f64
                    };

                    let total_brightness_shift = new_brightness - current_brightness;

                    let is_enabled = monitor_states_guard.is_monitor_index_enabled(monitor_index);

                    // don't waste time fading if the monitor isn't on
                    let fade = if is_enabled {
                        match &brightness_input.override_fade {
                            None => total_brightness_shift.abs() as u8 > fade_options.threshold,
                            Some(x) => *x,
                        }
                    } else {
                        false
                    };

                    let brightness_step = total_brightness_shift / (total_num_steps as f64);

                    let brightness_change_info = BrightnessChangeInfo {
                        end_brightness: new_brightness,
                        brightness_step,
                        fade,
                    };

                    // these fields are not references, so we can drop them uninitialized without
                    // worrying about a panic
                    // previously, we had to use std::mem::addr_of_mut and write_unaligned to write
                    // directly to the struct field addresses without dropping the old values
                    // however, this is only necessary for pointers
                    // now that is_fading (which is a reference) is not uninitialized in the
                    // beginning, we never have to write to raw pointers and can instead write normally
                    // like this
                    monitor_info.brightness_change_info = brightness_change_info;
                    monitor_info.is_enabled = is_enabled;
                });

            // print out intermediate brightness states
            for i in &intermediate_brightness_states {
                println!(
                    "{}: {} -> {}",
                    i.0,
                    i.1.current_brightness.get(),
                    i.1.brightness_change_info.end_brightness
                );
            }

            // TODO drain filter all the ones that will not be faded
            // alternative: partition into fade and non-fade
            // used relevant_monitor_indices instead of intermediate_brightness_states for
            // partition because you cannot map after partition
            let (to_fade, mut to_not_fade): (Vec<_>, Vec<_>) = {
                intermediate_brightness_states
                    .iter_mut()
                    .partition(|(_, monitor_info)| monitor_info.brightness_change_info.fade)
            };

            for (_, entry) in to_not_fade.iter_mut() {
                entry.current_brightness.set(entry.brightness_change_info.end_brightness);
                // TODO
                // actually probably don't want to remove this here
                // you should wait till the other monitors are done fading
                // intermediate_brightness_states.remove(index);
            }

            let auto_remove_displays = self.config.read().await.auto_remove_displays;

            // this returns true if refresh_brightness reconfigured the display automatically
            // dont want to reconfigure AGAIN
            // TODO consider doing this individually for each monitor
            // so that we can get different messages for each one
            //
            // filter out disabled monitors when calling refresh_brightness, but not when printing
            // out "set brightness"
            match self
                .refresh_brightness(
                    to_not_fade
                        .iter()
                        .filter(|(_, monitor_info)| monitor_info.is_enabled)
                        .map(|(&i, _)| i),
                    auto_remove_displays,
                )
                .await
            {
                Ok(_) => {
                    for (brightness, adapter_name) in
                        to_not_fade.into_iter().map(|(&i, monitor_info)| {
                            (
                                monitor_info.brightness_change_info.end_brightness,
                                monitor_states_guard
                                    .get_monitor_state_by_index(i)
                                    .unwrap()
                                    .get_monitor_name(),
                            )
                        })
                    {
                        socket_message_holder
                            .queue_success(format!("{adapter_name}: {brightness}"));
                    }
                    // socket_message_holder.queue_success(format!("Successfully modified brightness"));
                }
                Err(e) => {
                    socket_message_holder.queue_error(format!("Failed to refresh brightness: {e}"));
                }
            };

            struct SplitMonitorInfo<'a> {
                monitor_index: usize,
                is_fading: &'a Cell<bool>,
                brightness_change_info: &'a BrightnessChangeInfo,
            }

            let (to_fade_immutable, to_fade_mutexes, mut to_fade_brightnesses) = {
                let (mut to_fade_immutable, mut to_fade_mutexes, mut to_fade_brightnesses): (
                    Vec<_>,
                    Vec<_>,
                    Vec<_>,
                ) = Default::default();
                for (i, monitor_info) in to_fade.into_iter() {
                    to_fade_immutable.push(SplitMonitorInfo {
                        monitor_index: *i,
                        is_fading: monitor_info.is_fading,
                        brightness_change_info: &monitor_info.brightness_change_info,
                    });

                    let (brightness, mutex_guard) = monitor_info.current_brightness.split();
                    to_fade_brightnesses.push(brightness);
                    to_fade_mutexes.push(mutex_guard);
                }

                (to_fade_immutable, to_fade_mutexes, to_fade_brightnesses)
            };

            if !to_fade_immutable.is_empty() {
                macro_rules! set_fading_status {
                    ($status:expr) => {
                        for monitor_info in to_fade_immutable.iter() {
                            monitor_info.is_fading.set($status)
                        }
                    };
                }

                set_fading_status!(true);

                // the last step is dedicated to setting the brightness exactly to
                // new_brightness
                // if we only went by adding brightness_step, we would not end up exactly where
                // we wanted to be
                let iterator_num_steps = total_num_steps - 1;

                let fade_step_delay =
                    std::time::Duration::from_millis(fade_options.step_duration as u64);

                macro_rules! fade_iterator {
                    () => {
                        to_fade_immutable
                            .iter()
                            .map(|monitor_info| monitor_info.monitor_index)
                    };
                }

                if let Err(e) = self
                    .refresh_brightness(fade_iterator!(), auto_remove_displays)
                    .await
                {
                    socket_message_holder.queue_error(format!("Error refreshing brightness: {e}"));
                }

                // the problem here is that in order to get the futures from the mutex recv() function,
                // we need a mutable reference to the mutex guard, and as a result a mutable reference
                // to (eventually) the to_fade iterator
                // is there a way to iterate immutably over to_fade while still getting a mutable
                // reference to mutex_guard?
                //
                // FuturesUnordered because we only care if ONE OF the futures resolves
                // the actual "order" of the futures is meaningless since that is just the order of the
                // adapters
                let mut receiver_futures = {
                    to_fade_mutexes
                        .into_iter()
                        .map(|mutex_guard| mutex_guard.recv())
                        .collect::<FuturesUnordered<_>>()
                };

                let mut timer_interval = time::interval(fade_step_delay);
                // the first tick completes immediately
                // we tick here so that there is a delay between the first and second iteration of
                // the fade loop
                timer_interval.tick().await;

                for iter in 0..iterator_num_steps {
                    println!("Fade loop! {iter}");
                    for (i, (split_monitor_info, brightness)) in to_fade_immutable
                        .iter()
                        .zip(to_fade_brightnesses.iter_mut())
                        .enumerate()
                    {
                        let starting_brightness = brightness.load(Ordering::Relaxed);

                        println!("Starting {i} brightness: {starting_brightness}");

                        let ending_brightness = brightness.fetch_add(split_monitor_info.brightness_change_info.brightness_step, Ordering::Relaxed);

                        println!("{i} brightness: {ending_brightness}");
                    }

                    // do not autoremove displays because we do not want to slow it down
                    // autoremove means that we will have to wait for the exit codes from each xrandr
                    // process to return before moving on
                    // we optimize the loop by only looking at the exit codes in the very beginning
                    if let Err(e) = self.refresh_brightness(fade_iterator!(), false).await {
                        socket_message_holder
                            .queue_error(format!("Failed to set brightness during fade: {e}"));
                    }

                    // monitors 2 futures
                    // timer_interval.tick(): checks to see if the fade delay is up
                    // receiver_futures.next(): checks to see if any ForwardedBrightnessInput's have
                    // been sent over the mutex's channel
                    // if it receives a ForwardedBrightnessInput, it will add it to the queue or
                    // process it immediately if its terminate_fade flag is set to true
                    loop {
                        select! {
                            _ = timer_interval.tick() => break,
                            Some( Some( forwarded_brightness_input ) ) = receiver_futures.next() => {
                                println!("Received future!");

                                let terminate_fade = forwarded_brightness_input.brightness_input.terminate_fade;

                                inputs.push_back(forwarded_brightness_input);

                                // interrupt current fade by continuing base loop
                                // if terminate_fade is true
                                //
                                // otherwise the queued input will be processed in the next
                                // iteration of the loop
                                if terminate_fade {
                                    socket_message_holder.consume();
                                    println!("Terminating!");
                                    set_fading_status!(false);
                                    continue 'base_loop;
                                }
                            }
                        };
                    }
                }

                // reset fading back to false
                set_fading_status!(false);

                for monitor_info in to_fade_immutable {
                    let monitor_name = monitor_states_guard
                        .get_monitor_state_by_index(monitor_info.monitor_index)
                        .unwrap()
                        .get_monitor_name();
                    socket_message_holder.queue_success(format!(
                        "{monitor_name}: {}",
                        monitor_info.brightness_change_info.end_brightness
                    ));
                }
            }

            // send messages to client
            socket_message_holder.consume();
        }
    }

    async fn reconfigure_displays(&self) -> Result<()> {
        self.monitor_states.write().await.refresh_displays().await?;

        Ok(())
    }

    async fn create_xrandr_commands(
        &self,
        monitors: impl Iterator<Item = usize>,
    ) -> Vec<(usize, Command)> {
        let config = self.config.read().await;

        let monitor_states = self.monitor_states.read().await;

        monitors
            .map(move |monitor_state_index| {
                // TODO safety
                // filter_map instead?
                let monitor_state = monitor_states
                    .get_monitor_state_by_index(monitor_state_index)
                    .unwrap();

                let monitor = &monitor_state.monitor_data;

                let brightness_state = &monitor_state.brightness_state;
                let brightness = brightness_state.get();

                let nightlight_on = brightness_state.nightlight.get();

                // TODO don't waste memory on another copy of the brightness
                // maybe pass it in from the calling method?
                let brightness_string = format!("{:.5}", brightness / 100.0);

                let command = if config.use_redshift {
                    let mut redshift_call = Command::new("redshift");

                    redshift_call.arg("-PO");
                    redshift_call.arg(format!(
                        "{}",
                        if nightlight_on {
                            config.nightlight_options.redshift_temperature
                        } else {
                            6500
                        }
                    ));
                    redshift_call.arg("-m");
                    // TODO: is monitor_state_index the correct crtc?
                    redshift_call.arg(format!("randr:crtc={}", monitor.crtc_number()));
                    redshift_call.arg("-b").arg(brightness_string);

                    redshift_call
                } else {
                    let mut xrandr_call = Command::new("xrandr");

                    xrandr_call.arg("--output");
                    xrandr_call.arg(monitor_state.get_monitor_name());

                    xrandr_call.arg("--brightness").arg(brightness_string);

                    // not using redshift AND nightlight on
                    if nightlight_on {
                        xrandr_call
                            .arg("--gamma")
                            .arg(&config.nightlight_options.xrandr_gamma);
                    }

                    xrandr_call
                };

                (monitor_state_index, command)
            })
            .collect()
    }
}

async fn send_shutdown_signal() {
    // SEND INPUT TO DAEMON
    match UnixStream::connect(SOCKET_PATH).await {
        Ok(mut sock) => {
            let mock_save_daemon_input = ProgramInput::Shutdown;

            if let Ok(binary_encoded_input) = BINCODE_OPTIONS.serialize(&mock_save_daemon_input) {
                let write_result = sock.write_all(&binary_encoded_input).await;
                match write_result {
                    Ok(_) => {
                        println!("Successfully wrote save command to socket");
                    }
                    Err(e) => {
                        eprintln!("Failed to write save command to socket: {e}");
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Couldn't connect: {e:?}");
        }
    };
}

async fn handle_signals(signals: Signals) {
    let mut signals = signals.fuse();

    while let Some(signal) = signals.next().await {
        match signal {
            SIGINT | SIGTERM | SIGQUIT => {
                eprintln!("Received shutdown signal!");
                tokio::spawn(send_shutdown_signal());
            }
            _ => unreachable!("Received unidentified signal: {}", signal),
        }
    }
}

fn register_sigterm_handler() -> Result<()> {
    let signals_to_monitor = Signals::new([SIGTERM, SIGINT, SIGQUIT])?;

    tokio::spawn(handle_signals(signals_to_monitor));

    Ok(())
}

pub fn daemon(fork: bool) -> Result<()> {
    let file_utils = FileUtils::new()?;

    let pid_file_path = file_utils.get_daemon_pid_file();

    let cache_dir = file_utils.project_directory.cache_dir();

    if fork {
        let (stdout, stderr) = {
            let mut open_options = std::fs::OpenOptions::new();
            open_options.append(true).create(true);

            let stdout = open_options.open(cache_dir.join("daemon_stdout.out"))?;
            let stderr = open_options.open(cache_dir.join("daemon_stderr.err"))?;

            (stdout, stderr)
        };

        let daemonize = Daemonize::new()
            .pid_file(pid_file_path)
            .working_directory(cache_dir)
            // have to do this because the tokio runtime isnt created yet
            // the corresponding functions in FileUtils are async
            .stdout(stdout)
            .stderr(stderr);

        match daemonize.start() {
            Ok(_) => println!("Success, daemonized"),
            Err(e) => {
                let stringified_error = e.to_string();

                if stringified_error.contains("unable to lock pid file") {
                    eprintln!("Daemon is already running!");
                    eprintln!("To restart, run \"killall brightness_control\" before relaunching the daemon");
                    // explicit exit to prevent the raw error from being printed
                    std::process::exit(1);
                }

                return Err(Error::new(
                    ErrorKind::Other,
                    format!("Failed to daemonize: {stringified_error}"),
                ));
            }
        }
    }

    let tokio_runtime = runtime::Builder::new_current_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        // WAS enable_io
        .enable_all()
        .build()?;

    let daemon = tokio_runtime.block_on(Daemon::new(file_utils))?;
    daemon.start(tokio_runtime);

    Ok(())
}
