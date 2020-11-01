use daemonize::Daemonize;
use bincode::{Options, DefaultOptions};
use directories::ProjectDirs;

use lazy_static::lazy_static;

use serde::{Serialize, Deserialize};

use tokio::{
    prelude::*,
    io::{ BufReader },
    fs::{self, File, OpenOptions},
    net::{ UnixStream, UnixListener },
    stream::StreamExt,
    process::{Command},
    sync::{ RwLock, Mutex, MutexGuard, mpsc },
    runtime::{self, Runtime},
    try_join,
    select
};

use std::io::{Error, ErrorKind, Write as SyncWrite, Result};
use std::fmt::Display;

use std::cell::{ Cell, UnsafeCell };

use std::collections::VecDeque;

use std::cmp;

use regex::Regex;

use futures::stream::FuturesOrdered;

pub const SOCKET_PATH: &str = "/tmp/brightness_control_socket.sock";

pub const CONFIG_TEMPLATE: &str = include_str!("../config_template.toml");

lazy_static! {
    static ref DEFAULT_CONFIG: DaemonOptions = {
        let parsed_toml: DaemonOptions = toml::from_slice(CONFIG_TEMPLATE.as_bytes()).unwrap();
        parsed_toml
    };

    pub static ref BINCODE_OPTIONS: DefaultOptions = {
        let options = bincode::DefaultOptions::default();
        options.with_fixint_encoding();
        options
    };

    static ref XRANDR_DISPLAY_INFORMATION_REGEX: Regex = {
        Regex::new(r"(?x) # ignore whitespace
        # [[:alpha:]] represents ascii letters
        ^([[:alpha:]]+-[[:digit:]]+) # 0 : the adapter name
        \ # space
        connected
        \ # space
        .*? # optional other words
        ([[:digit:]]+) # 1 : width
        x
        ([[:digit:]]+) # 2 : height
        \+
        ([[:digit:]]+) # 3 : xOffset
        \+
        ([[:digit:]]+) # 4 : yOffset
        ").unwrap()
    };
}

// encapsulates information from xrandr --current
struct Monitor {
    adapter_name: String,
    width: u32,
    height: u32,
    xOffset: u32,
    yOffset: u32
}

impl Monitor {
    fn new(xrandr_line: &str) -> Option<Monitor> {
        // eDP-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 344mm x 193mm
        // HDMI-1 connected 1280x1024+1920+28 (normal left inverted right x axis y axis) 338mm x 270mm
        // <adapter> connected [primary] <width>x<height>+<x offset>+<y offset> (<flags>) <something>mm x <something else>mm
        let captures = (*XRANDR_DISPLAY_INFORMATION_REGEX).captures(xrandr_line);

        if let Some(captures) = captures {
            // 0 points to the entire match, so skip
            let adapter_name = captures.get(1).unwrap().as_str().to_owned();

            let mut parse_int = | num: regex::Match | {
                num.as_str().parse::<u32>()
            };

            (|| {
                let width = parse_int(captures.get(2).unwrap())?;
                let height = parse_int(captures.get(3).unwrap())?;
                let xOffset = parse_int(captures.get(4).unwrap())?;
                let yOffset = parse_int(captures.get(5).unwrap())?;

                std::result::Result::<Option<Monitor>, std::num::ParseIntError>::Ok(
                    Some(
                        Monitor {
                            adapter_name,
                            width,
                            height,
                            xOffset,
                            yOffset
                        }
                    )
                )
            })().unwrap_or(None)
        }
        else {
            None
        }
    }
}

struct SocketMessageHolder {
    // queue messages into this and then push them all to the socket at the end
    messages: Vec<SocketMessage>,
    socket: UnixStream
}

impl SocketMessageHolder {
    fn new(socket: UnixStream) -> SocketMessageHolder {
        SocketMessageHolder {
            messages: Vec::with_capacity(5),
            socket
        }
    }

    fn queue_message<T>(&mut self, message: T, log_socket_error: bool)
    where T: Into<String> {
        self.messages.push(
            SocketMessage {
                message: message.into(),
                log_socket_error
            }
        )
    }

    fn queue_success<T>(&mut self, message: T)
    where T: Into<String> {
        self.queue_message(message, true)
    }

    fn queue_error<T>(&mut self, message: T)
    where T: Into<String> {
        self.queue_message(message, false)
    }

    // NOTE remember to consume before it goes out of scope
    fn consume(mut self) {
        // write all messages to the socket
        tokio::spawn(
            async move {
                for message_struct in self.messages.into_iter() {
                    let message = message_struct.message;
                    if let Err(e) = self.socket.write_all(&message.as_bytes()).await {
                        if message_struct.log_socket_error {
                            eprintln!("Failed to write \"{}\" to socket: {}", message, e);
                        }
                    }
                }

                // cleanup; close connection
                let _ = self.socket.shutdown(std::net::Shutdown::Both);
            }
        );
    }
}

struct SocketMessage {
    message: String,
    log_socket_error: bool
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BrightnessInput {
    pub brightness: Option<BrightnessChange>,
    pub override_fade: Option<bool>,
    pub terminate_fade: bool
}

impl BrightnessInput {
    fn is_active(&self) -> bool {
        self.brightness.is_some() || self.terminate_fade
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum BrightnessChange {
    Adjustment(i8),
    Set(u8)
}

#[derive(Serialize, Deserialize, Debug)]
pub enum GetProperty {
    Brightness,
    IsFading,
    Mode,
    Displays,
    Config
}

enum ProcessInputExitCode {
    Normal,
    Shutdown
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProgramInput {
    brightness: BrightnessInput,
    get_property: Option<GetProperty>,
    configure_display: bool,
    toggle_nightlight: bool,
    reload_configuration: bool,
    shutdown: bool
}

impl ProgramInput {
    pub fn new(brightness: BrightnessInput, get_property: Option<GetProperty>, configure_display: bool, toggle_nightlight: bool, reload_configuration: bool, shutdown: bool) -> ProgramInput {
        ProgramInput {
            brightness,
            get_property,
            configure_display,
            toggle_nightlight,
            reload_configuration,
            shutdown,
        }
    }

    pub fn returns_feedback(&self) -> bool {
        // create a vector of all options that send back feedback to the client
        let feedback_returning_options = vec![
            self.reload_configuration
        ];

        // returns true if any of them are true
        feedback_returning_options.iter().any(|&b| b)
    }
}

struct FileUtils {
    project_directory: ProjectDirs,
    file_open_options: OpenOptions,
}

impl FileUtils {
    fn new() -> Result<FileUtils> {
        let project_directory = get_project_directory()?;

        let file_open_options = {
            let mut file_open_options = OpenOptions::new();
            file_open_options.read(true);
            file_open_options.write(true);
            file_open_options.create(true);
            file_open_options
        };

        Ok(
            FileUtils {
                project_directory,
                file_open_options
            }
        )
    }

    // returns the file and whether or not it existed prior to opening it
    async fn open_configuration_file(&self) -> Result<(File, bool)> {
        let config_dir = self.project_directory.config_dir();
        let filepath = config_dir.join("config.toml");

        if !config_dir.exists() {
            fs::create_dir(self.project_directory.config_dir()).await?;
        }

        let file_exists = fs::metadata(&filepath).await.is_ok();
        Ok( (self.file_open_options.open(filepath).await?, file_exists) )
    }

    async fn open_cache_file_with_options(&self, file_name: &str, open_options: &OpenOptions) -> Result<File> {
        let filepath = self.project_directory.cache_dir().join(file_name);
        open_options.open(filepath).await
    }

    async fn open_cache_file(&self, file_name: &str) -> Result<File> {
        self.open_cache_file_with_options(file_name, &self.file_open_options).await
    }

    async fn get_mode_file(&self) -> Result<File> {
        self.open_cache_file("mode").await
    }

    async fn get_brightness_file(&self) -> Result<File> {
        self.open_cache_file("brightness").await
    }

    // 0 for regular
    // 1 for night light
    // gets the mode written to disk; if invalid, writes a default and returns it
    async fn get_written_mode(&self) -> Result<bool> {
        let mut mode_file = self.get_mode_file().await?;
        mode_file.set_len(1).await?;

        get_valid_data_or_write_default(&mut mode_file, &| data_in_file: &String | {
            if let Ok(num) = data_in_file.parse::<u8>() {
                if num == 0 || num == 1 {
                    // cannot cast "num as bool" normally
                    return Ok(num != 0);
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid mode"));

        }, false).await
    }

    async fn get_written_brightness(&self) -> Result<f64> {
        let mut brightness_file = self.get_brightness_file().await?;

        get_valid_data_or_write_default(&mut brightness_file, &| data_in_file: &String | {
            // need to trim this because the newline character breaks the parse
            if let Ok(num) = data_in_file.trim_end().parse::<f64>() {
                // check bounds; don't allow 0.0 brightness
                if num <= 100.0 && num > 0.0 {
                    return Ok(num);
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid brightness"));
        }, 100.0).await
    }

    async fn write_brightness(&self, brightness: f64) -> Result<()> {
        let mut brightness_file = self.get_brightness_file().await?;
        overwrite_file_with_content(&mut brightness_file, brightness).await?;
        Ok(())
    }

    async fn write_mode(&self, mode: bool) -> Result<()> {
        let mut mode_file = self.get_mode_file().await?;
        overwrite_file_with_content(&mut mode_file, mode).await?;
        Ok(())
    }

    // checks if the header in the config template reads a different version
    // than the current version of BrightnessControl
    // if they do not match OR anything goes wrong during the check, overwrites
    // the template with CONFIG_TEMPLATE
    async fn update_config_template(&self) -> Result<()> {
        // read header of current template
        let data_dir = self.project_directory.data_dir();

        if !data_dir.exists() {
            fs::create_dir_all(data_dir).await?;
        }

        let template_filepath = data_dir.join("config_template.toml");
        let template_exists = template_filepath.exists();

        let mut template_file = self.file_open_options.open(&template_filepath).await?;
        let mut template_file_clone = template_file.try_clone().await?;

        // this allows us to handle any errors in this section the same way
        let overwrite_template_result = (|| async move {
            if template_exists {
                // we only have to read until the first newline
                // # a.b.c
                // 12345678
                // 2 bytes extra incase 'a' becomes double(/triple)-digits
                const BYTES_TO_READ: usize = 20;

                let buffered_reader = BufReader::with_capacity(BYTES_TO_READ, &mut template_file);

                // buffered_reader.fill_buf().await?;

                if let Ok(Some(first_line)) = buffered_reader.lines().next_line().await {
                    const NUM_CHARS_TO_IGNORE: usize = "# v".len();
                    // Strings from BufReader::lines do not include newlines at the
                    // end
                    let version_string = &first_line[NUM_CHARS_TO_IGNORE..];

                    let current_version_string = {
                        let beginning_trimmed = &CONFIG_TEMPLATE[NUM_CHARS_TO_IGNORE..];
                        let newline_index = beginning_trimmed.find("\n").unwrap();
                        &beginning_trimmed[..newline_index]
                    };

                    // compare to actual version string
                    if version_string.eq(current_version_string) {
                        return Ok(());
                    }
                    else {
                        println!("Config template updated! \"{}\" to \"{}\"", version_string, current_version_string);
                    }
                }
            }
            else {
                println!("Config template saved to {}", &template_filepath.display());
            }

            // the cause of this error is irrelevant, so it doesnt need a
            // message
            Err(Error::new(ErrorKind::Other, ""))
        })().await;

        // dont care about the cause
        if let Err(_) = overwrite_template_result {
            // overwrite
            overwrite_file_with_content(&mut template_file_clone, CONFIG_TEMPLATE).await?;
        }

        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct NightlightOptions {
    xrandr_gamma: String,
    redshift_temperature: u32
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct FadeOptions {
    threshold: u8,
    // milliseconds
    total_duration: u32,
    // milliseconds
    step_duration: u32
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct DaemonOptions {
    use_redshift: bool,
    auto_remove_displays: bool,
    fade_options: FadeOptions,
    nightlight_options: NightlightOptions
}

impl DaemonOptions {
    fn default() -> DaemonOptions {
        (*DEFAULT_CONFIG).clone()
    }
}

struct BrightnessState {
    // receiver end of channel in mutex
    brightness: NonReadBlockingRWLock<f64, mpsc::UnboundedReceiver<(BrightnessInput, SocketMessageHolder)>>,
    fade_notifier: mpsc::UnboundedSender<(BrightnessInput, SocketMessageHolder)>,
    is_fading: Cell<bool>
}

unsafe impl Send for BrightnessState {}
unsafe impl Sync for BrightnessState {}

impl BrightnessState {
    fn new(initial_brightness: f64) -> BrightnessState {
        let (tx, rx) = mpsc::unbounded_channel::<(BrightnessInput, SocketMessageHolder)>();

        BrightnessState {
            brightness: NonReadBlockingRWLock::new(initial_brightness, rx),
            fade_notifier: tx,
            is_fading: Cell::new(false)
        }
    }

    fn get(&self) -> f64 {
        self.brightness.get()
    }

    fn get_fade_notifier(&self) -> mpsc::UnboundedSender<(BrightnessInput, SocketMessageHolder)> {
        self.fade_notifier.clone()
    }

    fn try_lock_brightness<'a>(&'a self) -> Option<MutexGuardRefWrapper<'a, f64, mpsc::UnboundedReceiver<(BrightnessInput, SocketMessageHolder)>>> {
        self.brightness.try_lock_mut()
    }

    async fn lock_brightness<'a>(&'a self) -> MutexGuardRefWrapper<'a, f64, mpsc::UnboundedReceiver<(BrightnessInput, SocketMessageHolder)>> {
        self.brightness.lock_mut().await
    }
}

struct MutexGuardRefWrapper<'a, T: Copy, K> {
    internal: &'a mut T,
    mutex_guard: MutexGuard<'a, K>
}

impl<'a, T, K> MutexGuardRefWrapper<'a, T, K>
where T: Copy {
    fn set(&mut self, new_value: T) {
        *self.internal = new_value;
    }
}

// a version of RWLock that does not block readers from reading while a writer writes
// by providing them with a copy of the internal value
// this is meant to be used with primitives which have cheap Copy implementations
struct NonReadBlockingRWLock<T: Copy, K> {
    internal: Cell<T>,
    write_mutex: Mutex<K>
}

unsafe impl<T, K> Send for NonReadBlockingRWLock<T, K>
where T: Copy {}

unsafe impl<T, K> Sync for NonReadBlockingRWLock<T, K>
where T: Copy {}

impl<T, K> NonReadBlockingRWLock<T, K>
where T: Copy {
    fn new(initial_value: T, mutex_value: K) -> NonReadBlockingRWLock<T, K> {
        NonReadBlockingRWLock {
            internal: Cell::new(initial_value),
            write_mutex: Mutex::new(mutex_value)
        }
    }

    fn get(&self) -> T {
        self.internal.get()
    }

    async fn set_value(&self, new_value: T) {
        self.write_mutex.lock().await;
        self.internal.set(new_value);
    }

    fn try_lock_mut<'a>(&'a self) -> Option<MutexGuardRefWrapper<'a, T, K>> {
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
    

    async fn lock_mut<'a>(&'a self) -> MutexGuardRefWrapper<'a, T, K> {
        let guard = self.write_mutex.lock().await;

        MutexGuardRefWrapper {
            // need to get a mutable reference to internal without making the
            // function take a mutable reference
            internal: unsafe { self.internal.as_ptr().as_mut().unwrap() },
            mutex_guard: guard
        }
    }
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

        Ok(
            Daemon {
                brightness: BrightnessState::new(file_utils.get_written_brightness().await?),
                mode: NonReadBlockingRWLock::new(file_utils.get_written_mode().await?, ()),
                displays: RwLock::new(get_current_connected_displays().await?),
                config: RwLock::new(config),
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
                let index = active_monitor_indices_iterator.nth(0).unwrap();

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
            loop {
                if self.config.read().await.use_redshift {
                    if let Err(e) = self.refresh_redshift().await {
                        socket_holder.queue_error(format!("Failed to refresh redshift: {}", e));
                        break;
                    }
                }
                else {
                    if let Err(e) = self.refresh_brightness().await {
                        socket_holder.queue_error(format!("Failed to refresh xrandr: {}", e));
                        break;
                    }
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
                        Ok(config) => {
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

                self.refresh_brightness().await;

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

            if !use_redshift
            {
                if self.mode.get() {
                    xrandr_call.arg("--gamma")
                        .arg(xrandr_gamma);
                }
            }

            xrandr_call
        }).collect()
    }

    async fn create_xrandr_commands(&self) -> Vec<Command> {
        let brightness_string = format!("{:.2}", self.brightness.get() / 100.0);
        self.create_xrandr_commands_with_brightness(brightness_string).await
    }
}

async fn overwrite_file_with_content<T>(file: &mut File, new_content: T) -> Result<()>
where T: Display {
    file.seek(std::io::SeekFrom::Start(0)).await?;

    let formatted_new_content = format!("{}", new_content);

    // <<NOTE>> this can overflow? len() returns a usize
    file.set_len(formatted_new_content.len() as u64).await?;

    file.write_all(&formatted_new_content.as_bytes()).await?;

    Ok(())
}

// where ....
async fn get_valid_data_or_write_default<T>(file: &mut File, data_validator: &dyn Fn(&String) -> Result<T>, default_value: T) -> Result<T>
where T: Display {
    let mut file_clone = file.try_clone().await?;

    let file_contents = {
        // wrapping in a closure allows the inner else and the
        // outer else clauses to share the same code
        // here, we want to return None if the file does not exist
        // or if the file's contents are not readable as a number
        (|| async move {
            let mut buffer: Vec<u8> = Vec::new();
            file.read_to_end(&mut buffer).await?;

            let string = unsafe {
                String::from_utf8_unchecked(buffer)
            };

            data_validator(&string)
        })()
    }.await;

    if let Ok(contents) = file_contents {
        Ok(contents)
    }
    else {
        overwrite_file_with_content(&mut file_clone, &default_value).await?;
        Ok(default_value)
    }
}

fn get_project_directory() -> Result<directories::ProjectDirs> {
    let project_directory = ProjectDirs::from("", "Sridaran Thoniyil", "BrightnessControl");
    // did not use if let because it would require the entire function to be indented

    if let None = project_directory {
        panic!("Cannot find base directory");
    }

    let project_directory = project_directory.unwrap();
    // cache the mode
    let cache_directory = project_directory.cache_dir();

    if !cache_directory.exists() {
        std::fs::create_dir_all(cache_directory)?;
    }

    Ok(project_directory)
}

async fn get_current_connected_displays() -> Result<Vec<Monitor>> {
    let mut xrandr_current = Command::new("xrandr");
    xrandr_current.arg("--current");
    let command_output = xrandr_current.output().await?;
    // the '&' operator dereferences ascii_code so that it can be compared with a regular u8
    // its original type is &u8
    let output_lines = command_output.stdout.split(| &ascii_code | ascii_code == '\n' as u8);

    let connected_displays: Vec<Monitor> = output_lines.filter_map(|line| {
        // if valid UTF-8, pass to Monitor
        if let Ok(line) = std::str::from_utf8(line) {
            Monitor::new(line)
        }
        else {
            None
        }
    }).collect();

    Ok(connected_displays)
}

async fn get_configuration_from_file(configuration_file: &mut File) -> std::result::Result<DaemonOptions, toml::de::Error> {
    // 8 KB
    const INITIAL_BUFFER_SIZE: usize = 8 * 1024;

    let mut configuration_buffer = Vec::with_capacity(INITIAL_BUFFER_SIZE);

    // fill buffer
    if let Err(e) = configuration_file.read_to_end(&mut configuration_buffer).await {
        eprintln!("Failed to read from configuration file! {}", e);
    }

    let parsed_toml: toml::Value = toml::from_slice(&configuration_buffer[..configuration_buffer.len()])?;

    let mut config = DaemonOptions::default();

    macro_rules! overwrite_values {
        ( $( $x:ident ),* ) => {
            {
                $(
                    if let Some(option) = parsed_toml.get(stringify!($x)) {
                        config.$x = option.clone().try_into()?;
                    }
                )*
            }
        };
    }

    // TODO figure out how to use derive macro for this
    overwrite_values!(use_redshift, auto_remove_displays, fade_options, nightlight_options);

    return Ok(config);
}

async fn configure_displays() -> Result<Vec<Monitor>> {
    let connected_displays = get_current_connected_displays().await?;

    Ok(connected_displays)
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

    let pid_file_path = &file_utils.project_directory.cache_dir().join("daemon.pid");

    let cache_dir = file_utils.project_directory.cache_dir();

    let (stdout, stderr) = {
        let mut open_options = std::fs::OpenOptions::new();
        open_options
            .append(true)
            .create(true);

        let stdout = open_options.open(cache_dir.join("daemon_stdout.out"))?;
        let stderr = open_options.open(cache_dir.join("daemon_stderr.err"))?;

        (stdout, stderr)
    };

    if fork {
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
