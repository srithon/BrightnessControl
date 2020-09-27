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
    try_join
};

use std::io::{Error, ErrorKind, Write as SyncWrite, Result};
use std::fmt::Display;

use std::collections::VecDeque;
use std::cell::{ Cell, UnsafeCell };

use std::cmp;

pub const SOCKET_PATH: &str = "/tmp/brightness_control_socket.sock";

pub const CONFIG_TEMPLATE: &str = include_str!("../config_template.toml");

lazy_static! {
    static ref DEFAULT_CONFIG: DaemonOptions = {
        let parsed_toml: DaemonOptions = toml::from_slice(CONFIG_TEMPLATE.as_bytes()).unwrap();
        parsed_toml
    };
}

pub fn get_bincode_options() -> DefaultOptions {
    let options = bincode::DefaultOptions::default();
    options.with_fixint_encoding();
    options
}

#[derive(Serialize, Deserialize, Debug)]
pub enum BrightnessChange {
    Adjustment(i8),
    Set(u8)
}

#[derive(Serialize, Deserialize, Debug)]
pub enum GetProperty {
    Brightness,
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
    brightness: Option<BrightnessChange>,
    get_property: Option<GetProperty>,
    override_fade: Option<bool>,
    configure_display: bool,
    toggle_nightlight: bool,
    reload_configuration: bool,
    shutdown: bool,
    interrupt_fade: bool
}

impl ProgramInput {
    pub fn new(brightness: Option<BrightnessChange>, get_property: Option<GetProperty>, override_fade: Option<bool>, configure_display: bool, toggle_nightlight: bool, reload_configuration: bool, shutdown: bool, interrupt_fade: bool) -> ProgramInput {
        ProgramInput {
            brightness,
            get_property,
            override_fade,
            configure_display,
            toggle_nightlight,
            reload_configuration,
            shutdown,
            interrupt_fade
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

    async fn get_displays_file(&self) -> Result<File> {
        self.open_cache_file("displays").await
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

    // loads displays in `displays` or writes down the real values
    async fn get_written_displays(&self) -> Result<Vec<String>> {
        let mut displays_file = self.get_displays_file().await?;

        let buffered_display_file_reader = BufReader::new(&mut displays_file);
        // filter out all invalid lines and then collect them into a Vec<String>
        let read_displays = buffered_display_file_reader.lines().filter_map(| line | line.ok()).collect::<Vec<String>>().await;

        if read_displays.len() > 0 {
            Ok(read_displays)
        }
        else {
            configure_displays(&mut displays_file).await
        }
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

    async fn write_displays(&self, displays: &Vec<String>) -> Result<()> {
        let mut displays_file = self.get_displays_file().await?;
        write_specified_displays_to_file(&mut displays_file, displays).await?;
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
    auto_reconfigure: bool,
    fade_options: FadeOptions,
    nightlight_options: NightlightOptions
}

impl DaemonOptions {
    fn default() -> DaemonOptions {
        (*DEFAULT_CONFIG).clone()
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

    fn get_mutex_guard_contents(&'a mut self) -> &'a mut K {
        &mut *self.mutex_guard
    }
}

// a version of RWLock that does not block readers from reading while a writer writes
// by providing them with a copy of the internal value
// this is mainly intended for primitives which have cheap Copy implementations
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

struct Daemon {
    // these are primitives, so it doesn't matter
    brightness: NonReadBlockingRWLock<f64, ()>,
    mode: NonReadBlockingRWLock<bool, ()>,
    displays: RwLock<Vec<String>>,
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
        println!("Displays: {:?}", *daemon.displays.read().await);

        try_join!(
            async move {
                let mut incoming = listener.incoming();
                while let Some(stream) = incoming.next().await {
                    println!("Stream!");

                    let daemon = unsafe {
                        self.daemon.get().as_mut().unwrap()
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
                                        let res = daemon.process_input(program_input, stream).await;
                                        if let ProcessInputExitCode::Shutdown = res {
                                            // leave the loop
                                            // TODO see if you can just break
                                            match shutdown_channel.send( () ).await {
                                                Ok(_) => println!("SENT SHUTDOWN SIGNAL!"),
                                                Err(e) => eprintln!("FAILED TO SEND SHUTDOWN SIGNAL! {}", e)
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
                                    Ok(_) => println!("SENT SHUTDOWN SIGNAL!"),
                                    Err(e) => eprintln!("FAILED TO SEND SHUTDOWN SIGNAL! {}", e)
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
        );

        println!("EXITING RUN");

        Ok(())
    }

    fn start(self, mut tokio_runtime: Runtime) {
        println!("{:?}", tokio_runtime.block_on(self.run()));
        tokio_runtime.shutdown_timeout(std::time::Duration::from_millis(1000));
        println!("Shutdown tokio runtime!");
    }
}


        let config: DaemonOptions = {
            let configuration: DaemonOptions = (|| -> Result<DaemonOptions> {
                let (mut config_file, file_existed) = file_utils.open_configuration_file()?;

                // file exists
                if file_existed {
                    if let Ok(config) = get_configuration_from_file(&mut config_file) {
                        return Ok(config);
                    }
                }
                else {
                    overwrite_file_with_content(&mut config_file, CONFIG_TEMPLATE)?;
                }

                let config = DaemonOptions::default();

                // saves creating another instance of DaemonOptions::default()
                return Ok(config);
            })()?;

            configuration
        };

        println!("Loaded configuration: {:?}", config);

        Ok(
            Daemon {
                brightness: file_utils.get_written_brightness()?,
                mode: file_utils.get_written_mode()?,
                displays: file_utils.get_written_displays()?,
                child_processes: VecDeque::new(),
                config,
                file_utils
            }
        )
    }

    async fn save_configuration(&self) -> Result<()> {
        let res = try_join!(
            self.file_utils.write_mode(self.mode.get()),
            self.file_utils.write_brightness(self.brightness.get()),
            async {
                let displays = self.displays.read().await;
                self.file_utils.write_displays(&displays).await
            }
        );

        match res {
            Ok(_) => Ok( () ),
            // this unpacks std::io::Result<( (), (), () )>
            // and repacks it into a std::io::Result<( () )>
            Err(e) => Err(e)
        }
    }

    // boolean signals whether to skip display reconfiguration in process_input
    async fn refresh_brightness(&mut self) -> Result<bool> {
        let call_handle = self.create_xrandr_command().await.spawn()?;

        if self.config.auto_reconfigure {
            let exit_status = call_handle.await?;

            // if the call fails, then the configuration is no longer valid
            // reconfigures the display and then tries again
            if !exit_status.success() {
                // force reconfigure
                self.reconfigure_displays().await?;
                self.create_xrandr_command().await.spawn()?.await?;
                return Ok(true);
            }
        }
        else {
            self.child_processes.push_back(call_handle);
        }

        Ok(false)
    }

    fn clear_redshift(&mut self) -> Result<()> {
        // turn off redshift
        let mut redshift_disable = Command::new("redshift");
        redshift_disable.arg("-x");
        redshift_disable.spawn()?;
        Ok(())
    }

    async fn enable_redshift(&mut self) -> Result<()> {
        // turn on redshift
        let mut redshift_enable = Command::new("redshift");
        redshift_enable.arg("-O");
        redshift_enable.arg(format!("{}", self.config.nightlight_options.redshift_temperature));
        redshift_enable.spawn()?;
        Ok(())
    }

        if self.mode {
    async fn refresh_redshift(&mut self) -> Result<()> {
            self.enable_redshift().await?;
        }
        else {
            self.clear_redshift()?;
        }

        Ok(())
    }

    async fn refresh_configuration(&mut self) -> Result<()> {
        // don't need the early return flag here
        if self.config.use_redshift {
            self.refresh_redshift()?;
        let _ = self.refresh_brightness().await?;
        }

        Ok(())
    }

    fn process_input(&mut self, program_input: ProgramInput, socket: &mut UnixStream) {
        // avoided using destructuring because destructuring relies entirely on the order of the
        // struct elements
        let brightness = program_input.brightness;
        let get_property = program_input.get_property;
        let fade = program_input.override_fade;
        let toggle_nightlight = program_input.toggle_nightlight;
        let mut configure_display = program_input.configure_display;
        let reload_configuration = program_input.reload_configuration;
        let save_configuration = program_input.save_configuration;

        let mut write_message = move |message: &str, log_socket_error: bool| {
            if let Err(e) = socket.write_all(message.as_bytes()) {
                if log_socket_error {
                    eprintln!("Failed to write \"{}\" to socket: {}", message, e);
                }
            }
        };

        // have to use macros for these two because if they were closures they would need to share
        // a mutable reference to the write_message closure, which the borrow checker
        // would not allow
        macro_rules! write_success {
            ($message:expr) => {
                write_message($message, false);
            }
        }

        macro_rules! write_error {
            ($message:expr) => {
                write_message($message, true);
            }
        }

        if toggle_nightlight {
            self.mode = !self.mode;

            (|| {
                if self.config.use_redshift {
                    if let Err(e) = self.refresh_redshift() {
                        write_error!(&format!("Failed to refresh redshift: {}", e));
                        return;
                    }
                }
                else {
                    if let Err(e) = self.refresh_brightness() {
                        write_error!(&format!("Failed to refresh xrandr: {}", e));
                        return;
                    }
                }

                // could have used format! to make this a one-liner, but this allows the strings to be
                // stored in static memory instead of having to be generated at runtime
                if self.mode {
                    write_error!("Enabled nightlight");
                }
                else {
                    write_error!("Disabled nightlight");
                }
            })()
        }

        match brightness {
            Some(brightness_change) => {
                let new_brightness = {
                    let integer_representation = match brightness_change {
                        BrightnessChange::Set(new_brightness) => new_brightness,
                        BrightnessChange::Adjustment(brightness_shift) => {
                            cmp::max(cmp::min(brightness_shift + (self.brightness as i8), 100), 0) as u8
                        }
                    };

                    integer_representation as f64
                };

                let total_brightness_shift = new_brightness - self.brightness;

                let fade_options = &self.config.fade_options;

                let fade = {
                    // if it contains a value, use it
                    if let Some(fade) = fade {
                        fade
                    }
                    else {
                        // if there is no override, then do the auto check
                        (total_brightness_shift.abs()) as u8 > fade_options.threshold 
                    }
                };

                if !fade {
                    self.brightness = new_brightness;

                    // this returns true if refresh_brightness reconfigured the display automatically
                    // dont want to reconfigure AGAIN
                    match self.refresh_brightness().await {
                        Ok(skip_configure_display) => {
                            write_error!(&format!("Set brightness to {}%", self.brightness));

                            if skip_configure_display {
                                configure_display = false;
                                write_error!("Automatically reconfigured displays!");
                            }
                        },
                        Err(e) => {
                            write_error!(&format!("Failed to refresh brightness: {}", e));
                        }
                    };
                }
                else {
                    // fade
                    let total_num_steps = fade_options.total_duration / fade_options.step_duration;

                    let brightness_step = total_brightness_shift / (total_num_steps as f64);

                    // the last step is dedicated to setting the brightness exactly to
                    // new_brightness
                    // if we only went by adding brightness_step, we would not end up exactly where
                    // we wanted to be
                    let iterator_num_steps = total_num_steps - 1;

                    let fade_step_delay = std::time::Duration::from_millis(fade_options.step_duration as u64);

                    if let Ok(true) = self.refresh_brightness() {
                        configure_display = false;
                    }

                    for _ in 0..iterator_num_steps {
                        self.brightness += brightness_step;

                        let brightness_string = format!("{:.5}", self.brightness / 100.0);

                        let mut command = self.create_xrandr_command_with_brightness(brightness_string);

                        match command.spawn() {
                            Ok(call_handle) => {
                                self.child_processes.push_back(call_handle);
                            },
                            Err(e) => {
                                write_error!(&format!("Failed to set brightness during fade: {}", e));
                            }
                        };

                        self.check_child_processes();

                        // spin_sleep is more accurate than regular std::thread::sleep
                        // we want our fades to be smooth, so the delay for each one must be as
                        // consistent as possible
                        spin_sleep::sleep(fade_step_delay);
                    }

                    self.brightness = new_brightness;

                    match self.refresh_brightness() {
                        Ok(_) => {
                            write_success!(&format!("Completed fade to brightness: {}", new_brightness));
                        }
                        Err(e) => {
                            write_error!(&format!("Failed to complete fade: {}", e));
                        }
                    }
                }
            },
            None => ()
        };

        if let Some(property) = get_property {
            let property_value = match property {
                GetProperty::Brightness => {
                    format!("{}", &self.brightness)
                },
                GetProperty::Displays => {
                    self.displays.join(" ")
                },
                GetProperty::Mode => {
                    format!("{}", self.mode as i32)
                },
                GetProperty::Config => {
                    format!("{:?}", &self.config)
                }
            };

            write_success!(&property_value);
        };

        if configure_display {
                write_error!(&format!("Failed to reconfigure displays: {}", e));
            if let Err(e) = self.reconfigure_displays().await {
            }
            else {
                write_success!("Successfully reconfigured displays!");
            }
        }

        if reload_configuration {
            match self.file_utils.open_configuration_file().await {
                Ok( (mut configuration_file, _) ) => {
                    let config_result = get_configuration_from_file(&mut configuration_file);

                    match config_result.await {
                        Ok(config) => {
                            self.config = config;

                            write_success!("Successfully reloaded configuration!");
                        }
                        Err(error) => {
                            write_error!(&format!("Failed to parse configuration file: {}", error));
                        }
                    }
                },
                Err(e) => {
                    write_error!(&format!("Failed to open configuration file for reloading: {}", e));
                }
            }
        }

        if save_configuration {
            if let Err(e) = self.save_configuration() {
                write_error!(&format!("Failed to save configuration: {}", e));
            }
            else {
                write_success!("Successfully saved configuration!");
            }
        }
    }

    async fn reconfigure_displays(&mut self) -> Result<()> {
        let mut displays_file = self.file_utils.get_displays_file().await?;
        let new_displays = configure_displays(&mut displays_file).await?;

        // immutable update
        // self.displays.clear();
        // self.displays.clone_from(&new_displays);

        // mutable update
        self.displays = new_displays;
        Ok(())
    }

    async fn create_xrandr_command_with_brightness(&self, brightness_string: String) -> Command {
        let mut xrandr_call = Command::new("xrandr");

        for display in &self.displays {
            xrandr_call.arg("--output");
            xrandr_call.arg(display);
        }

        xrandr_call.arg("--brightness")
            .arg(brightness_string);

        if !self.config.use_redshift
        {
            if self.mode {
                xrandr_call.arg("--gamma")
                    .arg(&self.config.nightlight_options.xrandr_gamma);
            }
        }

        xrandr_call

    }

    fn create_xrandr_command(&self) -> Command {
        let brightness_string = format!("{:.2}", *(&self.brightness) as f32 / 100.0);
        self.create_xrandr_command_with_brightness(brightness_string)
    async fn create_xrandr_command(&self) -> Command {
    }
}

async fn overwrite_file_with_content<T>(file: &mut File, new_content: T) -> Result<()>
where T: Display {
    file.seek(std::io::SeekFrom::Start(0))?;

    let formatted_new_content = format!("{}", new_content);

    // <<NOTE>> this can overflow? len() returns a usize
    file.set_len(formatted_new_content.len() as u64)?;

    write!(file, "{}", formatted_new_content)?;

    Ok(())
}

// where ....
async fn get_valid_data_or_write_default<T>(file: &mut File, data_validator: &dyn Fn(&String) -> Result<T>, default_value: T) -> Result<T>
where T: Display {
    let file_contents = {
        // wrapping in a closure allows the inner else and the
        // outer else clauses to share the same code
        // here, we want to return None if the file does not exist
        // or if the file's contents are not readable as a number
        (|| -> std::io::Result<T> {
            let mut buffer: Vec<u8> = Vec::new();
            file.read_to_end(&mut buffer)?;

            let string = unsafe {
                String::from_utf8_unchecked(buffer)
            };

            data_validator(&string)
        })()
    };

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
        fs::create_dir_all(cache_directory)?;
    }

    Ok(project_directory)
}

async fn get_current_connected_displays() -> Result<Vec<String>> {
    let mut xrandr_current = Command::new("xrandr");
    xrandr_current.arg("--current");
    let command_output = xrandr_current.output()?;
    // the '&' operator dereferences ascii_code so that it can be compared with a regular u8
    // its original type is &u8
    let output_lines = command_output.stdout.split(| &ascii_code | ascii_code == '\n' as u8);
    let connected_displays: Vec<String> = output_lines.filter(| line | {
        // panic if the output is not UTF 8
        let line_as_string = std::str::from_utf8(line).unwrap();
        /*
           this predicate filters out everything but the first line
           example output
           eDP-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 344mm x 193mm
           HDMI-1 disconnected (normal left inverted right x axis y axis)
           DP-1 disconnected (normal left inverted right x axis y axis)
           HDMI-2 disconnected (normal left inverted right x axis y axis)*
           */
        line_as_string.contains(" connected")
    }).map(| line | {
        // if it passed through the filter, it has to be valid UTF8
        // therefore, this unsafe call can be made safely
        let line_as_string = unsafe { std::str::from_utf8_unchecked(line) };
        // panic if the output does not contain a space
        // it has to contain a space because the filter predicate specifically has a space
        // in it
        // create a slice of the first Word
        line_as_string[0..line_as_string.find(' ').unwrap()].to_owned()
    }).collect();

    Ok(connected_displays)
}

async fn write_specified_displays_to_file(displays_file: &mut File, connected_displays: &Vec<String>) -> Result<()> {
    // sum the lengths of each display name, and then add (number of names - 1) to account
    // for newline separators between each name
    // subtract 1 because there is no newline at the end
    let displays_file_length = (connected_displays.len() - 1) +
        connected_displays.iter().map(| display_name | display_name.len()).sum::<usize>();

    // .iter() so that connected_displays is not moved
    for display in connected_displays.iter() {
        writeln!(displays_file, "{}", display)?;
    }

    // the above loop appends a newline to each display, including the last one
    // however, this call to set_len() cuts out this final newline
    displays_file.set_len(displays_file_length as u64).await?;

    Ok(())
}

fn get_configuration_from_file(configuration_file: &mut File) -> std::result::Result<DaemonOptions, toml::de::Error> {
    let mut buffered_reader = BufReader::new(configuration_file);

    // fill buffer
    if let Err(e) = configuration_file.read_to_end(&mut configuration_buffer).await {
        eprintln!("Failed to read from configuration file! {}", e);
    }

    let parsed_toml: toml::Value = toml::from_slice(buffered_reader.buffer())?;

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
    overwrite_values!(use_redshift, auto_reconfigure, fade_options, nightlight_options);

    return Ok(config);
}

async fn configure_displays(displays_file: &mut File) -> Result<Vec<String>> {
    let connected_displays = get_current_connected_displays().await?;

    write_specified_displays_to_file(displays_file, &connected_displays).await?;

    Ok(connected_displays)
}

fn register_sigterm_handler() -> Result<()> {
    unsafe {
        signal_hook::register(signal_hook::SIGTERM, move || {
            // signal_hook 
            std::thread::spawn(|| {
                // SEND INPUT TO DAEMON
                match UnixStream::connect(SOCKET_PATH) {
                    Ok(mut sock) => {
                        let mock_save_daemon_input = ProgramInput {
                            brightness: None,
                            get_property: None,
                            override_fade: None,
                            toggle_nightlight: false,
                            configure_display: false,
                            reload_configuration: false,
                            save_configuration: true
                        };

                        let bincode_options = get_bincode_options();
                        if let Ok(binary_encoded_input) = bincode_options.serialize(&mock_save_daemon_input) {
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

                        let _ = sock.shutdown(std::net::Shutdown::Both);

                        // wait 1 second for it to finish
                        let one_second = std::time::Duration::from_millis(1000);
                        std::thread::sleep(one_second);
                    },
                    Err(e) => {
                        eprintln!("Couldn't connect: {:?}", e);
                    }
                };

                let _ = std::fs::remove_file(SOCKET_PATH);

                std::process::abort()
            });
        })
    }?;

    Ok(())
}

pub fn daemon() -> Result<()> {
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
