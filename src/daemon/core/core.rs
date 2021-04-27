use daemonize::Daemonize;
use bincode::{Options};

use tokio::{
    prelude::*,
    fs,
    net::UnixListener,
    stream::StreamExt,
    process::{Command},
    sync::{ RwLock, mpsc },
    runtime::{self, Runtime},
    try_join,
    select
};

use std::io::{Error, ErrorKind, Write as SyncWrite, Result};

use std::cell::{ Cell, UnsafeCell };

use std::collections::{ VecDeque, BTreeMap };

use std::cmp;

use futures::stream::{ FuturesUnordered, FuturesOrdered };

use crate::daemon::fs::*;

use super::{
    display::*,
    super::{
        config::{
            runtime::*,
            persistent::*
        },
        util::{
            lock::*,
            io::*
        },
        super::{
            shared::*
        }
    },
};

enum ProcessInputExitCode {
    Normal,
    Shutdown
}

// separate RwLocks so they can be modified concurrently
struct Daemon {
    // these are primitives, so it doesn't matter
    monitor_states: CollectiveMonitorState,
    mode: NonReadBlockingRWLock<bool, ()>,
    config: RwLock<DaemonOptions>,
    file_utils: FileUtils
}

unsafe impl Send for Daemon {}
unsafe impl Sync for Daemon {}

struct DaemonWrapper {
    daemon: UnsafeCell<Daemon>
}

impl DaemonWrapper {
    async fn run(self) -> Result<()> {
        let daemon = unsafe {
            self.daemon.get().as_mut().unwrap()
        };

        daemon.refresh_configuration().await?;

        register_sigterm_handler()?;

        let mut listener = match UnixListener::bind(SOCKET_PATH) {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("Error binding listener: {}", e);
                fs::remove_file(SOCKET_PATH).await?;
                UnixListener::bind(SOCKET_PATH)?
            }
        };

        // this is used as a shutdown signal
        // if any message is sent over the channel, the dameon starts shutting down
        let (tx, mut rx) = mpsc::channel::<()>(30);

        println!("Mode: {}", daemon.mode.get());
        println!("{}", daemon.monitor_states.get_formatted_display_states(Some(&MonitorOverride::All)).await);

        try_join!(
            async move {
                let mut incoming = listener.incoming();
                let daemon_pointer = self.daemon.get();

                while let Some(stream) = incoming.next().await {
                    println!("Stream!");

                    let daemon = unsafe {
                        daemon_pointer.clone().as_mut().unwrap()
                    };

                    let mut shutdown_channel = tx.clone();
                    tokio::spawn(async move {
                        match stream {
                            Ok(mut stream) => {
                                // Rust is amazing
                                // the compiler figured out the type of program_input based on the call to
                                // daemon.process_input 5 lines below

                                // https://github.com/bincode-org/bincode
                                // "The size of the encoded object will be the same or smaller than the size that the object takes up in memory in a running Rust program."
                                const STREAM_BUFFER_SIZE: usize = std::mem::size_of::<ProgramInput>();
                                let mut stream_buffer: [u8; STREAM_BUFFER_SIZE] = [0; STREAM_BUFFER_SIZE];

                                let num_bytes: usize = stream.read(&mut stream_buffer[..]).await?;

                                let program_input = BINCODE_OPTIONS.deserialize_from(&stream_buffer[..num_bytes]);
                                match program_input {
                                    Ok(program_input) => {
                                        println!("Deserialized ProgramInput: {:?}", program_input);
                                        let res = daemon.process_input(program_input, SocketMessageHolder::new(stream)).await;
                                        if let ProcessInputExitCode::Shutdown = res {
                                            // leave the loop
                                            // TODO see if you can just break
                                            match shutdown_channel.send( () ).await {
                                                Ok(_) => println!("Sent shutdown signal!"),
                                                Err(e) => eprintln!("Failed to send shutdown signal! {}", e)
                                            }
                                        }
                                    },
                                    Err(err) => {
                                        eprintln!("Error deserializing: {}", err);
                                    }
                                }
                            }
                            Err(_) => {
                                match shutdown_channel.send( () ).await {
                                    Ok(_) => println!("Sent shutdown signal!"),
                                    Err(e) => eprintln!("Failed to send shutdown signal! {}", e)
                                }
                            }
                        }

                        std::io::Result::Ok( () )
                    });
                }

                Ok( () )
            },
            async move {
                rx.recv().await;
                std::io::Result::<()>::Err( Error::new(ErrorKind::ConnectionAborted, "Shutting down daemon!") )
            }
        )?;

        println!("Successfully exitting run function");

        Ok(())
    }

    fn start(self, mut tokio_runtime: Runtime) {
        println!("{:?}", tokio_runtime.block_on(self.run()));
        tokio_runtime.shutdown_timeout(std::time::Duration::from_millis(1000));
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
                        Err(e) => eprintln!("Error getting configuration from file (initial): {}", e),
                        // rewrapping Result with different Err type
                        Ok(c) => return Ok(c)
                    }
                }
                else {
                    overwrite_file_with_content(&mut config_file, CONFIG_TEMPLATE).await?;
                }

                let config = DaemonOptions::default();

                // saves creating another instance of DaemonOptions::default()
                return std::io::Result::<DaemonOptions>::Ok(config);
            }.await?;

            configuration
        };

        println!("Loaded configuration: {:?}", config);

        let (mode, monitor_states) = {
            let cached_state = file_utils.get_cached_state().await?;

            (
                NonReadBlockingRWLock::new(cached_state.nightlight, ()),
                CollectiveMonitorState::new(cached_state.active_monitor, cached_state.brightness_states).await
            )
        };

        let config = RwLock::new(config);

        Ok(
            Daemon {
                monitor_states,
                mode,
                config,
                file_utils
            }
        )
    }

    async fn save_configuration(&self) -> Result<()> {
        let monitor_states = self.monitor_states.read().await;

        let iterator = monitor_states.iter_all_monitor_states();

        let mut map = fnv::FnvHashMap::default();
        for monitor_state in iterator {
            map.insert(monitor_state.get_monitor_name().to_owned(), monitor_state.get_brightness_state().get());
        }

        let cached_state = CachedState {
            brightness_states: map,
            nightlight: self.mode.get(),
            active_monitor: *monitor_states.get_active_monitor_index()
        };

        let res = try_join!(
            self.file_utils.write_cached_state(&cached_state)
        );

        match res {
            Ok(_) => Ok( () ),
            // this unpacks std::io::Result<( (), (), () )>
            // and repacks it into a std::io::Result<( () )>
            Err(e) => Err(e)
        }
    }

    async fn refresh_brightness_all(&self) -> Result<bool> {
        self.refresh_brightness(
            self.monitor_states.read().await.get_monitor_override_indices(&MonitorOverride::Enabled).into_iter(), true
        ).await
    }

    // boolean signals whether the function removed any monitors from self.displays
    async fn refresh_brightness(&self, monitors: impl Iterator<Item=usize>, auto_remove_displays: bool) -> Result<bool> {
        let commands = self.create_xrandr_commands(monitors).await;

        if auto_remove_displays {
            // use UnsafeCell to have 2 separate iterators
            // 1 iterating over futures
            // 1 iterating over indices
            // futures require a mutable reference to the underlying vector
            // therefore, the 2 iterators cannot coexist normally
            let enumerated_futures = {
                let enumerated_futures = commands.into_iter().filter_map(|(index, mut command)| {
                    if let Ok(call_handle) = command.spawn() {
                        Some( (index, call_handle) )
                    }
                    else {
                        None
                    }
                }).collect::<Vec<( usize, _ )>>();

                UnsafeCell::new(enumerated_futures)
            };

            let mut active_monitor_indices_iterator = {
                let enumerated_futures = unsafe {
                    enumerated_futures.get().as_mut().unwrap()
                };

                enumerated_futures.iter().map(| ( index, _ ) | *index).rev()
            };

            // we use FuturesOrdered because we want the output to line up with
            // active_monitor_indices_iterator
            // so we can identify the indices of the displays which threw an error
            let mut ordered_futures = {
                let enumerated_futures = unsafe {
                    enumerated_futures.get().as_mut().unwrap()
                };

                enumerated_futures.iter_mut().map(| ( _, future ) | future).rev().collect::<FuturesOrdered<_>>()
            };

            let mut removed_display = false;

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
        }
        else {
            // wait for it on its own
            for (_, mut command) in commands {
                let call_handle = command.spawn()?;
                tokio::spawn(call_handle);
            }

            Ok(false)
        }
    }

    fn clear_redshift(&mut self) -> Result<()> {
        // turn off redshift
        let mut redshift_disable = Command::new("redshift");
        redshift_disable.arg("-x");
        let call_handle = redshift_disable.spawn()?;
        tokio::spawn(call_handle);
        Ok(())
    }

    async fn enable_redshift(&mut self) -> Result<()> {
        // turn on redshift
        let mut redshift_enable = Command::new("redshift");
        redshift_enable.arg("-O");
        redshift_enable.arg(format!("{}", self.config.read().await.nightlight_options.redshift_temperature));
        let call_handle = redshift_enable.spawn()?;
        tokio::spawn(call_handle);
        Ok(())
    }

    async fn refresh_redshift(&mut self) -> Result<()> {
        if self.mode.get() {
            self.enable_redshift().await?;
        }
        else {
            self.clear_redshift()?;
        }

        Ok(())
    }

    async fn refresh_configuration(&mut self) -> Result<()> {
        // don't need the early return flag here
        let _ = self.refresh_brightness_all().await?;

        if self.config.read().await.use_redshift {
            self.refresh_redshift().await?;
        }

        Ok(())
    }

    async fn process_input(&mut self, program_input: ProgramInput, mut socket_holder: SocketMessageHolder) -> ProcessInputExitCode {
        // avoided using destructuring because destructuring relies entirely on the order of the
        // struct elements
        let brightness_change = program_input.brightness;
        let get_property = program_input.get_property;
        let toggle_nightlight = program_input.toggle_nightlight;
        let configure_display = program_input.configure_display;
        let reload_configuration = program_input.reload_configuration;
        let shutdown = program_input.shutdown;

        if toggle_nightlight {
            self.mode.set_value(!self.mode.get()).await;

            // janky alternative to an async closure
            // this allows us to early-return from this block
            // can't use a regular closure because then we wouldnt be able to use async/await
            // inside of it
            // can't use an async block inside of a regular one because then we would need to move all the
            // captured variables into the block
            // this is ugly but it works
            #[allow(clippy::never_loop)]
            loop {
                if self.config.read().await.use_redshift {
                    if let Err(e) = self.refresh_redshift().await {
                        socket_holder.queue_error(format!("Failed to refresh redshift: {}", e));
                        break;
                    }
                }
                // not using redshift, so refresh brightness to activate xrandr nightlight active
                else if let Err(e) = self.refresh_brightness_all().await {
                    socket_holder.queue_error(format!("Failed to refresh xrandr: {}", e));
                    break;
                }

                // could have used format! to make this a one-liner, but this allows the strings to be
                // stored in static memory instead of having to be generated at runtime
                if self.mode.get() {
                    socket_holder.queue_success("Enabled nightlight");
                }
                else {
                    socket_holder.queue_success("Disabled nightlight");
                }

                break;
            };
        }

        if let Some(property) = get_property {
            // TODO create macro that yields an iterator over MonitorState's
            let property_value = match property {
                GetProperty::Brightness(optional_monitor_override) => {
                    self.monitor_states.get_formatted_display_states(optional_monitor_override.as_ref()).await
                },
                GetProperty::Displays => {
                    // TODO take a MonitorOverride for this one too
                    self.monitor_states.get_formatted_display_names(Some(&MonitorOverride::All)).await
                },
                GetProperty::Mode => {
                    format!("{}", self.mode.get() as i32)
                },
                GetProperty::Config => {
                    format!("{:?}", *self.config.read().await)
                },
                GetProperty::IsFading(optional_monitor_override) => {
                    // return "1" if currently fading
                    // else "0"
                    self.monitor_states.get_formatted_display_states_with_format(optional_monitor_override.as_ref(), |monitor_state| {
                        let is_fading = monitor_state.brightness_state.is_fading.get();

                        if is_fading {
                            "1"
                        }
                        else {
                            "0"
                        }
                    }).await
                },
                GetProperty::ActiveMonitor => {
                    let monitors = self.monitor_states.read().await;
                    let active_index = *monitors.get_active_monitor_index();
                    let active_monitor_state = monitors.get_monitor_state_by_index(active_index).unwrap();
                    format!("{}", active_monitor_state.get_monitor_name())
                }
            };

            socket_holder.queue_success(property_value);
        };

        if configure_display {
            if let Err(e) = self.reconfigure_displays().await {
                socket_holder.queue_error(format!("Failed to reconfigure displays: {}", e));
            }
            else {
                socket_holder.queue_success("Successfully reconfigured displays!");
            }
        }

        if reload_configuration {
            match self.file_utils.open_configuration_file().await {
                Ok( (mut configuration_file, _) ) => {
                    let config_result = get_configuration_from_file(&mut configuration_file);

                    match config_result.await {
                        std::result::Result::<DaemonOptions, toml::de::Error>::Ok(config) => {
                            *self.config.write().await = config;

                            socket_holder.queue_success("Successfully reloaded configuration!");
                        }
                        Err(error) => {
                            socket_holder.queue_error(format!("Failed to parse configuration file: {}", error));
                        }
                    }
                },
                Err(e) => {
                    socket_holder.queue_error(format!("Failed to open configuration file for reloading: {}", e));
                }
            }
        }

        if shutdown {
            if let Err(e) = self.save_configuration().await {
                socket_holder.queue_error(format!("Failed to save configuration: {}", e));
            }
            else {
                socket_holder.queue_success("Successfully saved configuration!");
            }

            return ProcessInputExitCode::Shutdown;
        }

        if brightness_change.is_active() {
            // let process_brightness_input handle the socket holder
            self.process_brightness_input(brightness_change, socket_holder).await;
        }
        else {
            // otherwise, consume it here
            socket_holder.consume();
        }

        ProcessInputExitCode::Normal
    }

    async fn process_brightness_input(&self, brightness_change: BrightnessInput, socket_holder: SocketMessageHolder) {
        println!("In processbrigtnesinput!");

        // push to this queue whenever new input comes in
        let mut inputs = VecDeque::<ForwardedBrightnessInput>::with_capacity(2);
        inputs.push_back( ForwardedBrightnessInput::new_unprocessed(brightness_change, socket_holder) );

        struct BrightnessChangeInfo {
            brightness_step: f64,
            end_brightness: f64,
            fade: bool
        }

        struct MonitorInfo<'a> {
            current_brightness: BrightnessGuard<'a>,
            is_enabled: bool,
            is_fading: &'a Cell<bool>,
            brightness_change_info: BrightnessChangeInfo
        }

        // ???
        unsafe impl<'a> Send for MonitorInfo<'a> {}
        unsafe impl<'a> Sync for MonitorInfo<'a> {}

        // map between monitor index and BrightnessChangeInfo
        let mut intermediate_brightness_states: BTreeMap<usize, MonitorInfo> = BTreeMap::new();

        // use the read() and write() functions
        // this used to be on the inside of the loop so we would have to retake the lock every time
        // should we do it like that again?
        let monitor_states_guard = self.monitor_states.read().await;

        // process inputs queue
        'base_loop: loop {
            let head = inputs.pop_front();

            if head.is_none() {
                break;
            }

            let ForwardedBrightnessInput {
                brightness_input,
                mut socket_message_holder,
                mut info
            } = head.unwrap();

            if info.is_unprocessed() {
                let monitor_indices = {
                    let config = self.config.read().await;

                    let override_monitor = {
                        if let Some(ref override_monitor) = &brightness_input.override_monitor {
                            override_monitor
                        }
                        else {
                            &config.monitor_default_behavior
                        }
                    };

                    monitor_states_guard.get_monitor_override_indices(override_monitor)
                };

                println!("Monitor indices: {:#?}", monitor_indices);

                info.transform_unprocessed(monitor_indices);
            }

            let relevant_monitor_indices = match info {
                BrightnessInputInfo::Processed { relevant_monitor_indices } => relevant_monitor_indices,
                _ => unreachable!("Info should have been transformed!")
            };

            // Printing out the starting brightness for all affected monitors
            for &i in relevant_monitor_indices.iter() {
                let f = monitor_states_guard.get_monitor_state_by_index(i).unwrap();
                println!("Original Brightness: {}", f.get_brightness_state().brightness.get());
            }

            // remove all irrelevant intermediate_brightness_states
            // this releases the mutices associated with them
            // TODO can we assume that relevant_monitor_indices is sorted?
            // TODO improve complexity
            // TODO consider using a BTreeSet for relevant_monitor_indices?
            {
                let keys_to_remove: Vec<_> = intermediate_brightness_states.keys().map(|&index| index).filter(|index| !relevant_monitor_indices.contains(&index)).collect();

                for key in keys_to_remove {
                    intermediate_brightness_states.remove(&key);
                }
            }

            let optional_guards: Vec<Option<BrightnessGuard>> = relevant_monitor_indices
                .iter()
                .map(|&index| monitor_states_guard
                    .get_monitor_state_by_index(index)
                    .expect("monitor state not in vector")
                    .get_brightness_state()
                    .try_lock_brightness()
                ).collect();

            // TODO make this random so there is no preference towards those in the beginning of
            // the list
            let first_none_guard_index = optional_guards.iter().position(|x| x.is_none());

            if let Some(index) = first_none_guard_index {
                // pass everything along to the owner of the receiver
                // someone else has the lock
                // they may be fading
                // try sending input over the mpsc channel
                // respond?
                let brightness_state = monitor_states_guard.get_monitor_state_by_index(index).expect("Monitor state found from find() not in the list?").get_brightness_state();

                let send_channel = brightness_state.get_fade_notifier();

                let forwarded_brightness_input = ForwardedBrightnessInput::new_processed(brightness_input, socket_message_holder, relevant_monitor_indices);

                let _ = send_channel.send( forwarded_brightness_input );

                // send all the ones that are queued
                // theoretically this should always be empty
                for input in inputs.into_iter() {
                    let _ = send_channel.send(input);
                }

                return;
            }

            // TODO don't clone this
            let fade_options = &self.config.read().await.fade_options.clone();

            // fade
            let total_num_steps = fade_options.total_duration / fade_options.step_duration;

            // populate intermediate_brightness_states with current brightnesses
            optional_guards.into_iter().enumerate().for_each(|(i, guard)| {
                let monitor_index = relevant_monitor_indices[i];
                // intermediate_brightness_states.insert(monitor_index, guard.unwrap().get());

                let brightness_state = monitor_states_guard.get_monitor_state_by_index(monitor_index).expect("monitor state index not found").get_brightness_state();

                let current_brightness = guard.as_ref().unwrap().get();

                let new_brightness = {
                    let integer_representation = match brightness_input.brightness {
                        Some(BrightnessChange::Set(new_brightness)) => new_brightness,
                        Some(BrightnessChange::Adjustment(brightness_shift)) => {
                            cmp::max(cmp::min(brightness_shift as i16 + (current_brightness as i16), 100 as i16), 0) as u8
                        },
                        None => current_brightness as u8
                    };

                    integer_representation as f64
                };

                let total_brightness_shift = new_brightness - current_brightness;

                let is_enabled = monitor_states_guard.is_monitor_index_enabled(monitor_index);

                // don't waste time fading if the monitor isn't on
                let fade = if is_enabled {
                    match &brightness_input.override_fade {
                        None => total_brightness_shift.abs() as u8 > fade_options.threshold,
                        Some(x) => *x
                    }
                } else {
                    false
                };

                let brightness_step = total_brightness_shift / (total_num_steps as f64);

                let brightness_input_info = MonitorInfo {
                    // TODO verify that this works
                    current_brightness: guard.unwrap(),
                    is_enabled,
                    is_fading: &brightness_state.is_fading,
                    brightness_change_info: BrightnessChangeInfo {
                        end_brightness: new_brightness,
                        brightness_step,
                        fade
                    }
                };

                intermediate_brightness_states.insert(monitor_index, brightness_input_info);
            });

            // print out intermediate brightness states
            for i in &intermediate_brightness_states {
                println!("{}: {}", i.0, i.1.current_brightness.get());
            }

            // TODO drain filter all the ones that will not be faded
            // alternative: partition into fade and non-fade
            // used relevant_monitor_indices instead of intermediate_brightness_states for
            // partition because you cannot map after partition
            let (to_fade_unsafe_cell, mut to_not_fade): (UnsafeCell<Vec<_>>, Vec<_>) = {
                let (to_fade, to_not_fade): (Vec<_>, Vec<_>) = intermediate_brightness_states
                    .iter_mut()
                    .partition(|(_, monitor_info)|
                        monitor_info
                        .brightness_change_info
                        .fade
                    );

                // UnsafeCell usage explained below; we need a mutable reference at one point
                (UnsafeCell::new(to_fade), to_not_fade)
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
            match self.refresh_brightness(to_not_fade.iter().filter(|(_, monitor_info)| monitor_info.is_enabled).map(|(&i, _)| i), auto_remove_displays).await {
                Ok(_) => {
                    for (brightness, adapter_name) in to_not_fade.into_iter().map(|(&i, monitor_info)| (monitor_info.brightness_change_info.end_brightness, monitor_states_guard.get_monitor_state_by_index(i).unwrap().get_monitor_name())) {
                        socket_message_holder.queue_success(format!("Set {} brightness to {}%\n", adapter_name, brightness));
                    }
                    // socket_message_holder.queue_success(format!("Successfully modified brightness"));
                },
                Err(e) => {
                    socket_message_holder.queue_error(format!("Failed to refresh brightness: {}", e));
                }
            };

            let to_fade = unsafe {
                to_fade_unsafe_cell.get().as_mut().unwrap()
            };

            macro_rules! set_fading_status {
                ($status:expr) => {
                    for (_, monitor_info) in to_fade.iter() {
                        monitor_info.is_fading.set($status)
                    }
                }
            }

            set_fading_status!(true);

            // the last step is dedicated to setting the brightness exactly to
            // new_brightness
            // if we only went by adding brightness_step, we would not end up exactly where
            // we wanted to be
            let iterator_num_steps = total_num_steps - 1;

            let fade_step_delay = std::time::Duration::from_millis(fade_options.step_duration as u64);

            macro_rules! fade_iterator {
                () => {
                    to_fade.iter().map(|(&i, _)| i)
                }
            }

            if let Err(e) = self.refresh_brightness(fade_iterator!(), auto_remove_displays).await {
                socket_message_holder.queue_error(format!("Error refreshing brightness: {}", e));
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
            let mut receiver_futures = unsafe {
                let to_fade_alias = to_fade_unsafe_cell.get().as_mut().unwrap();
                to_fade_alias
                    .iter_mut()
                    .map(|(_, monitor_info)| {
                        monitor_info.current_brightness.mutex_guard.recv()
                    }).collect::<FuturesUnordered<_>>()
            };

            for _ in 0..iterator_num_steps {
                for (i, (_, monitor_info)) in to_fade.iter_mut().enumerate() {
                    println!("Starting {} brightness: {}", i, monitor_info.current_brightness.get());
                    let brightness = monitor_info.current_brightness.get() + monitor_info.brightness_change_info.brightness_step;
                    println!("{} brightness: {}", i, brightness);
                    monitor_info.current_brightness.set(brightness);
                }

                // do not autoremove displays because we do not want to slow it down
                // autoremove means that we will have to wait for the exit codes from each xrandr
                // process to return before moving on
                // we optimize the loop by only looking at the exit codes in the very beginning
                if let Err(e) = self.refresh_brightness(fade_iterator!(), false).await {
                    socket_message_holder.queue_error(format!("Failed to set brightness during fade: {}", e));
                }

                let mut delay_future = tokio::time::delay_for(fade_step_delay);

                // monitors 2 futures
                // delay_future: checks to see if the fade delay is up
                // receiver_futures.next(): checks to see if any ForwardedBrightnessInput's have
                // been sent over the mutex's channel
                // if it receives a ForwardedBrightnessInput, it will add it to the queue or
                // process it immediately if its terminate_fade flag is set to true
                loop {
                    select! {
                        _ = &mut delay_future => break,
                        Some( Some( forwarded_brightness_input ) ) = receiver_futures.next() => {
                            let terminate_fade = forwarded_brightness_input.brightness_input.terminate_fade;

                            inputs.push_back(forwarded_brightness_input);

                            // interrupt current fade by continuing base loop
                            // if terminate_fade is true
                            //
                            // otherwise the queued input will be processed in the next
                            // iteration of the loop
                            if terminate_fade {
                                socket_message_holder.consume();
                                set_fading_status!(false);
                                continue 'base_loop;
                            }
                        }
                    };
                };
            }

            // send messages to client
            socket_message_holder.consume();
        }
    }

    async fn reconfigure_displays(&self) -> Result<()> {
        self.monitor_states.write().await.refresh_displays().await?;

        Ok(())
    }

    async fn create_xrandr_commands(&self, monitors: impl Iterator<Item=usize>) -> Vec<(usize, Command)> {
        let config = self.config.read().await;

        let monitor_states = self.monitor_states.read().await;

        let nightlight_on = self.mode.get();

        monitors.map(move |monitor_state_index| {
            // TODO safety
            // filter_map instead?
            let monitor_state = monitor_states.get_monitor_state_by_index(monitor_state_index).unwrap();

            let mut xrandr_call = Command::new("xrandr");

            xrandr_call.arg("--output");
            xrandr_call.arg(monitor_state.get_monitor_name());

            // TODO don't waste memory on another copy of the brightness
            // maybe pass it in from the calling method?
            let brightness_string = format!("{:.5}", monitor_state.brightness_state.get() / 100.0);

            xrandr_call.arg("--brightness")
                .arg(brightness_string);

            // not using redshift AND nightlight on
            if !config.use_redshift && nightlight_on {
                xrandr_call.arg("--gamma")
                    .arg(&config.nightlight_options.xrandr_gamma);
            }

            (monitor_state_index, xrandr_call)
        })
        .collect()
    }
}

fn register_sigterm_handler() -> Result<()> {
    unsafe {
        signal_hook::register(signal_hook::SIGTERM, move || {
            // signal_hook 
            std::thread::spawn(|| {
                // SEND INPUT TO DAEMON
                match std::os::unix::net::UnixStream::connect(SOCKET_PATH) {
                    Ok(mut sock) => {
                        let mock_save_daemon_input = ProgramInput {
                            brightness: BrightnessInput {
                                brightness: None,
                                override_fade: None,
                                override_monitor: None,
                                terminate_fade: false
                            },
                            get_property: None,
                            toggle_nightlight: false,
                            configure_display: false,
                            reload_configuration: false,
                            shutdown: true,
                        };

                        if let Ok(binary_encoded_input) = BINCODE_OPTIONS.serialize(&mock_save_daemon_input) {
                            let write_result = sock.write_all(&binary_encoded_input);
                            match write_result {
                                Ok(_) => {
                                    println!("Successfully wrote save command to socket");
                                },
                                Err(e) => {
                                    eprintln!("Failed to write save command to socket: {}", e);
                                }
                            }
                        }

                        // wait 1 second for it to finish
                        let one_second = std::time::Duration::from_millis(1000);
                        std::thread::sleep(one_second);
                    },
                    Err(e) => {
                        eprintln!("Couldn't connect: {:?}", e);
                    }
                };

                let _ = std::fs::remove_file(SOCKET_PATH);
            });
        })
    }?;

    Ok(())
}

pub fn daemon(fork: bool) -> Result<()> {
    let file_utils = FileUtils::new()?;

    let pid_file_path = file_utils.get_daemon_pid_file();

    let cache_dir = file_utils.project_directory.cache_dir();

    if fork {
        let (stdout, stderr) = {
            let mut open_options = std::fs::OpenOptions::new();
            open_options
                .append(true)
                .create(true);

            let stdout = open_options.open(cache_dir.join("daemon_stdout.out"))?;
            let stderr = open_options.open(cache_dir.join("daemon_stderr.err"))?;

            (stdout, stderr)
        };

        let daemonize = Daemonize::new()
            .pid_file(pid_file_path)
            .working_directory(&cache_dir)
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

                return Err(Error::new(ErrorKind::Other, format!("Failed to daemonize: {}", stringified_error)));
            }
        }
    }

    let mut tokio_runtime = runtime::Builder::new()
        .core_threads(2)
        .max_threads(4)
        .threaded_scheduler()
        // WAS enable_io
        .enable_all()
        .build()?;

    let daemon = tokio_runtime.block_on(Daemon::new(file_utils))?;
    let daemon_wrapper = DaemonWrapper {
        daemon: UnsafeCell::new(daemon)
    };

    daemon_wrapper.start(tokio_runtime);

    Ok( () )
}
