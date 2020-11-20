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

use std::cell::{ UnsafeCell };

use std::collections::VecDeque;

use std::cmp;

use futures::stream::FuturesOrdered;

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
    brightness: BrightnessState,
    mode: NonReadBlockingRWLock<bool, ()>,
    displays: RwLock<Vec<Monitor>>,
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

        println!("Brightness: {}", daemon.brightness.get());
        println!("Mode: {}", daemon.mode.get());
        println!("Displays: {}", daemon.get_formatted_displays_list().await);

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
                                let mut stream_buffer: [u8; 20] = [0; 20];

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
                    if let Ok(config) = get_configuration_from_file(&mut config_file).await {
                        return Ok(config);
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

        let (brightness, mode, displays) = {
            let (written_brightness, written_mode, connected_displays) = tokio::join!(
                file_utils.get_written_brightness(),
                file_utils.get_written_mode(),
                get_current_connected_displays()
            );

            (
                BrightnessState::new(written_brightness?),
                NonReadBlockingRWLock::new(written_mode?, ()),
                RwLock::new(connected_displays?)
            )
        };

        let config = RwLock::new(config);

        Ok(
            Daemon {
                brightness,
                mode,
                displays,
                config,
                file_utils
            }
        )
    }

    // returns a string with the adapters separated by spaces
    async fn get_formatted_displays_list(&self) -> String {
        self.displays.read().await.iter().map(|monitor| &*monitor.adapter_name).collect::<Vec<&str>>().join(" ")
    }

    async fn save_configuration(&self) -> Result<()> {
        let res = try_join!(
            self.file_utils.write_mode(self.mode.get()),
            self.file_utils.write_brightness(self.brightness.get()),
        );

        match res {
            Ok(_) => Ok( () ),
            // this unpacks std::io::Result<( (), (), () )>
            // and repacks it into a std::io::Result<( () )>
            Err(e) => Err(e)
        }
    }

    // boolean signals whether the function removed any monitors from self.displays
    async fn refresh_brightness(&self) -> Result<bool> {
        let commands = self.create_xrandr_commands().await;
        let auto_remove_displays = self.config.read().await.auto_remove_displays;

        if auto_remove_displays {
            // use UnsafeCell to have 2 separate iterators
            // 1 iterating over futures
            // 1 iterating over indices
            // futures require a mutable reference to the underlying vector
            // therefore, the 2 iterators cannot coexist normally
            let enumerated_futures = {
                let enumerated_futures = commands.into_iter().enumerate().filter_map(|(index, mut command)| {
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
                    let mut displays_write_guard = self.displays.write().await;
                    (*displays_write_guard).remove(index);

                    removed_display = true;
                }
            }

            Ok(removed_display)
        }
        else {
            // wait for it on its own
            for mut command in commands {
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
        let _ = self.refresh_brightness().await?;
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
                else if let Err(e) = self.refresh_brightness().await {
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
            let property_value = match property {
                GetProperty::Brightness => {
                    format!("{}", self.brightness.get())
                },
                GetProperty::Displays => {
                    self.get_formatted_displays_list().await
                },
                GetProperty::Mode => {
                    format!("{}", self.mode.get() as i32)
                },
                GetProperty::Config => {
                    format!("{:?}", *self.config.read().await)
                },
                GetProperty::IsFading => {
                    // return "1" if currently fading
                    // else "0"
                    if self.brightness.is_fading.get() {
                        "1"
                    }
                    else {
                        "0"
                    }.to_owned()
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
        // push to this queue whenever new input comes in
        let mut inputs = VecDeque::<(BrightnessInput, SocketMessageHolder)>::with_capacity(2);
        inputs.push_back( (brightness_change, socket_holder) );

        let mut persistent_guard = None;

        'base_loop: loop {
            let head = inputs.pop_front();

            if head.is_none() {
                break;
            }

            let ( brightness_change, mut socket_holder ) = head.unwrap();

            let current_brightness = self.brightness.get();

            let new_brightness = {
                let integer_representation = match brightness_change.brightness {
                    Some(BrightnessChange::Set(new_brightness)) => new_brightness,
                    Some(BrightnessChange::Adjustment(brightness_shift)) => {
                        cmp::max(cmp::min(brightness_shift as i16 + (current_brightness as i16), 100 as i16), 0) as u8
                    },
                    None => current_brightness as u8
                };

                integer_representation as f64
            };

            let total_brightness_shift = new_brightness - current_brightness;

            // TODO don't clone this
            let fade_options = &self.config.read().await.fade_options.clone();
            let fade = {
                match &brightness_change.override_fade {
                    None => total_brightness_shift.abs() as u8 > fade_options.threshold,
                    Some(x) => *x
                }
            };

            let optional_guard = {
                if brightness_change.terminate_fade {
                    // interrupt fade
                    // if there is actually someone else modifying the brightness
                    // note that this may not necessarily be a fade
                    // if it is not a fade, then we have nothing to worry about
                    // it will terminate on its own
                    if let x @ Some(_) = persistent_guard.take().or_else(|| self.brightness.try_lock_brightness()) {
                        x
                    }
                    else {
                        // someone else has the lock
                        // they may be fading
                        // try sending input over the mpsc channel
                        // respond?
                        let send_channel = self.brightness.get_fade_notifier();

                        let _ = send_channel.send( (brightness_change, socket_holder) );

                        // send all the ones that are queued
                        // theoretically this should always be empty
                        for input in inputs.into_iter() {
                            let _ = send_channel.send( (input.0, input.1) );
                        }

                        return;
                    }
                }
                else {
                    None
                }
            };

            if !fade {
                if let Some(mut guard) = optional_guard {
                    guard.set(new_brightness);
                }
                else {
                    self.brightness.brightness.set_value(new_brightness).await;
                }

                // this returns true if refresh_brightness reconfigured the display automatically
                // dont want to reconfigure AGAIN
                match self.refresh_brightness().await {
                    Ok(_) => {
                        socket_holder.queue_success(format!("Set brightness to {}%", self.brightness.get()));
                    },
                    Err(e) => {
                        socket_holder.queue_error(format!("Failed to refresh brightness: {}", e));
                    }
                };
            }
            else {
                self.brightness.is_fading.set(true);

                // fade
                let total_num_steps = fade_options.total_duration / fade_options.step_duration;

                let brightness_step = total_brightness_shift / (total_num_steps as f64);

                // the last step is dedicated to setting the brightness exactly to
                // new_brightness
                // if we only went by adding brightness_step, we would not end up exactly where
                // we wanted to be
                let iterator_num_steps = total_num_steps - 1;

                let fade_step_delay = std::time::Duration::from_millis(fade_options.step_duration as u64);

                if let Err(e) = self.refresh_brightness().await {
                    socket_holder.queue_error(format!("Error refreshing brightness: {}", e));
                }

                let mut brightness_guard = {
                    if let Some(guard) = optional_guard {
                        guard
                    }
                    else {
                        self.brightness.lock_brightness().await
                    }
                };

                for _ in 0..iterator_num_steps {
                    let brightness = self.brightness.get() + brightness_step;

                    brightness_guard.set(brightness);

                    let brightness_string = format!("{:.5}", brightness / 100.0);

                    let commands = self.create_xrandr_commands_with_brightness(brightness_string).await;

                    for mut command in commands {
                        match command.spawn() {
                            Ok(call_handle) => {
                                tokio::spawn(call_handle);
                            },
                            Err(e) => {
                                socket_holder.queue_error(format!("Failed to set brightness during fade: {}", e));
                            }
                        };
                    }

                    let mut delay_future = tokio::time::delay_for(fade_step_delay);

                    // this has to be mutable to call recv() on it
                    let receiver = &mut *brightness_guard.mutex_guard;

                    loop {
                        select! {
                            _ = &mut delay_future => break,
                            Some( (input, other_socket_holder) ) = receiver.recv() => {
                                let terminate_fade = input.terminate_fade;

                                inputs.push_back( (input, other_socket_holder) );

                                // interrupt current fade by continuing base loop
                                // if terminate_fade is true
                                //
                                // otherwise the queued input will be processed in the next
                                // iteration of the loop
                                if terminate_fade {
                                    socket_holder.consume();
                                    self.brightness.is_fading.set(false);
                                    continue 'base_loop;
                                }
                            }
                        };
                    };
                }

                self.brightness.is_fading.set(false);

                brightness_guard.set(new_brightness);

                persistent_guard = Some(brightness_guard);

                match self.refresh_brightness().await {
                    Ok(_) => {
                        socket_holder.queue_success(format!("Completed fade to brightness: {}", new_brightness));
                    }
                    Err(e) => {
                        socket_holder.queue_error(format!("Failed to complete fade: {}", e));
                    }
                }
            }

            socket_holder.consume();
        }
    }

    async fn reconfigure_displays(&self) -> Result<()> {
        let new_displays = configure_displays().await?;

        // immutable update
        // self.displays.clear();
        // self.displays.clone_from(&new_displays);

        // mutable update
        *self.displays.write().await = new_displays;
        Ok(())
    }

    async fn create_xrandr_commands_with_brightness(&self, brightness_string: String) -> Vec<Command> {
        let config = self.config.read().await;

        let (use_redshift, xrandr_gamma) = ( config.use_redshift, &config.nightlight_options.xrandr_gamma );

        self.displays.read().await.iter().map(|display| {
            let mut xrandr_call = Command::new("xrandr");

            xrandr_call.arg("--output");
            xrandr_call.arg(&display.adapter_name);

            xrandr_call.arg("--brightness")
                .arg(&brightness_string);

            // not using redshift AND nightlight on
            if !use_redshift && self.mode.get() {
                xrandr_call.arg("--gamma")
                    .arg(xrandr_gamma);
            }

            xrandr_call
        }).collect()
    }

    async fn create_xrandr_commands(&self) -> Vec<Command> {
        let brightness_string = format!("{:.2}", self.brightness.get() / 100.0);
        self.create_xrandr_commands_with_brightness(brightness_string).await
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

    let pid_file_path = file_utils.get_cache_dir().join("daemon.pid");

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
