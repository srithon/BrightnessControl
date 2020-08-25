use daemonize::Daemonize;
use bincode::{Options, DefaultOptions};
use toml::Value;
use directories::ProjectDirs;

use serde::{Serialize, Deserialize};

use std::os::unix::net::{UnixStream, UnixListener};

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Error, ErrorKind, Seek, Write, Read, Result};
use std::fmt::Display;
use std::process::Command;

use std::cmp;

pub const SOCKET_PATH: &str = "/tmp/brightness_control_socket.sock";

pub fn get_bincode_options() -> DefaultOptions {
    let options = bincode::DefaultOptions::default();
    options.with_fixint_encoding();
    options
}

enum DataValidatorResult<T> {
    Valid(T),
    Changed(T),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum BrightnessChange {
    Adjustment(i8),
    Set(u8)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProgramInput {
    brightness: Option<BrightnessChange>,
    configure_display: bool,
    toggle_nightlight: bool,
    reload_configuration: bool,
    save_configuration: bool
}

impl ProgramInput {
    pub fn new(brightness: Option<BrightnessChange>, configure_display: bool, toggle_nightlight: bool, reload_configuration: bool, save_configuration: bool) -> ProgramInput {
        ProgramInput {
            brightness,
            configure_display,
            toggle_nightlight,
            reload_configuration,
            save_configuration
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
    // returns the file and whether or not it existed prior to opening it
    fn open_configuration_file(&self) -> Result<(File, bool)> {
        let config_dir = self.project_directory.config_dir();
        let filepath = config_dir.join("config.toml");

        if !config_dir.exists() {
            std::fs::create_dir(self.project_directory.config_dir())?;
        }

        let file_exists = std::fs::metadata(&filepath).is_ok();
        Ok( (self.file_open_options.open(filepath)?, file_exists) )
    }

    fn open_cache_file_with_options(&self, file_name: &str, open_options: &OpenOptions) -> Result<File> {
        let filepath = self.project_directory.cache_dir().join(file_name);
        open_options.open(filepath)
    }

    fn open_cache_file(&self, file_name: &str) -> Result<File> {
        self.open_cache_file_with_options(file_name, &self.file_open_options)
    }

    fn get_mode_file(&self) -> Result<File> {
        self.open_cache_file("mode")
    }

    fn get_brightness_file(&self) -> Result<File> {
        self.open_cache_file("brightness")
    }

    fn get_displays_file(&self) -> Result<File> {
        self.open_cache_file("displays")
    }

    // 0 for regular
    // 1 for night light
    // gets the mode written to disk; if invalid, writes a default and returns it
    fn get_written_mode(&self) -> Result<bool> {
        let mut mode_file = self.get_mode_file()?;
        mode_file.set_len(1)?;

        get_valid_data_or_write_default(&mut mode_file, &| data_in_file: &String | {
            if let Ok(num) = data_in_file.parse::<u8>() {
                if num == 0 || num == 1 {
                    // cannot cast "num as bool" normally
                    return Ok(Valid(num != 0));
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid mode"));

        }, false)
    }

    // loads displays in `displays` or writes down the real values
    fn get_written_displays(&self) -> Result<Vec<String>> {
        let mut displays_file = self.get_displays_file()?;

        let buffered_display_file_reader = BufReader::new(&mut displays_file);
        // filter out all invalid lines and then collect them into a Vec<String>
        let read_displays = buffered_display_file_reader.lines().filter_map(| line | line.ok()).collect::<Vec<String>>();

        if read_displays.len() > 0 {
            Ok(read_displays)
        }
        else {
            configure_displays(&mut displays_file)
        }
    }

    fn get_written_brightness(&self) -> Result<u8> {
        let mut brightness_file = self.get_brightness_file()?;

        get_valid_data_or_write_default(&mut brightness_file, &| data_in_file: &String | {
            // need to trim this because the newline character breaks the parse
            if let Ok(num) = data_in_file.trim_end().parse::<u8>() {
                // check bounds
                if num <= 100 {
                    return Ok(Valid(num));
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid brightness"));
        }, 100)
    }

    fn write_brightness(&self, brightness: u8) -> Result<()> {
        let mut brightness_file = self.get_brightness_file()?;
        overwrite_file_with_content(&mut brightness_file, brightness)?;
        Ok(())
    }

    fn write_mode(&self, mode: bool) -> Result<()> {
        let mut mode_file = self.get_mode_file()?;
        overwrite_file_with_content(&mut mode_file, mode)?;
        Ok(())
    }

    fn write_displays(&self, displays: &Vec<String>) -> Result<()> {
        let mut displays_file = self.get_displays_file()?;
        write_specified_displays_to_file(&mut displays_file, displays)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct DaemonOptions {
    placeholder_option: u8
}

impl DaemonOptions {
    fn default() -> DaemonOptions {
        DaemonOptions {
            placeholder_option: 10
        }
    }
}

struct Daemon {
    brightness: u8,
    mode: bool,
    displays: Vec<String>,
    config: DaemonOptions,
    file_utils: FileUtils
}

impl Daemon {
    fn new(project_directory: ProjectDirs) -> Result<Daemon> {
        let file_open_options = {
            let mut file_open_options = OpenOptions::new();
            file_open_options.read(true);
            file_open_options.write(true);
            file_open_options.create(true);
            file_open_options
        };

        let file_utils = FileUtils {
            project_directory,
            file_open_options
        };

        let config: DaemonOptions = {
            let configuration: DaemonOptions = (|| {
                let (mut config_file, file_existed) = file_utils.open_configuration_file()?;

                // file exists
                if file_existed {
                    if let Ok(config) = get_configuration_from_file(&mut config_file) {
                        return Ok(config);
                    }
                }
                else {
                    // write defaults to config file
                    let defaults = DaemonOptions::default();
                    let serialized_defaults = match toml::to_string_pretty(&defaults) {
                        Ok(serialized_defaults) => serialized_defaults,
                        Err(error) => {
                            return Err(Error::new(ErrorKind::InvalidData, format!("{}", error)));
                        }
                    };

                    overwrite_file_with_content(&mut config_file, serialized_defaults)?;

                    // saves creating another instance of DaemonOptions::default()
                    return Ok(defaults);
                }

                Ok(DaemonOptions::default())
            })()?;

            configuration
        };

        println!("Loaded configuration: {:?}", config);

        Ok(
            Daemon {
                brightness: file_utils.get_written_brightness()?,
                mode: file_utils.get_written_mode()?,
                displays: file_utils.get_written_displays()?,
                config,
                file_utils
            }
        )
    }

    fn run(&mut self) -> Result<()> {
        let pid_file_path = self.file_utils.project_directory.cache_dir().join("daemon.pid");
        let (stdout, stderr) = {
            let mut log_file_open_options = OpenOptions::new();
            log_file_open_options.create(true);
            log_file_open_options.append(true);

            let stdout = self.file_utils.open_cache_file_with_options("daemon_stdout.out", &log_file_open_options)?;
            let stderr = self.file_utils.open_cache_file_with_options("daemon_stderr.err", &log_file_open_options)?;

            (stdout, stderr)
        };

        let daemonize = Daemonize::new()
            .pid_file(pid_file_path)
            .working_directory(&self.file_utils.project_directory.cache_dir())
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

        let listener = match UnixListener::bind(SOCKET_PATH) {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("Error binding listener: {}", e);
                std::fs::remove_file(SOCKET_PATH)?;
                UnixListener::bind(SOCKET_PATH)?
            }
        };

        let bincode_options = get_bincode_options();

        println!("Brightness: {}", self.brightness);
        println!("Mode: {}", self.mode);
        println!("Displays: {:?}", self.displays);

        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let stream_reader = BufReader::new(&mut stream);
                    // Rust is amazing
                    // the compiler figured out the type of program_input based on the call to
                    // self.process_input 5 lines below
                    let program_input = bincode_options.deserialize_from(stream_reader);
                    match program_input {
                        Ok(program_input) => {
                            println!("Deserialized ProgramInput: {:?}", program_input);
                            self.process_input(program_input, Some(&mut stream))?;
                        },
                        Err(err) => {
                            eprintln!("Error deserializing: {}", err);
                        }
                    }

                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
                Err(_) => {
                    break;
                }
            }
        }
        Ok(())
    }

    fn save_configuration(&self) -> Result<()> {
        self.file_utils.write_mode(self.mode)?;
        self.file_utils.write_brightness(self.brightness)?;
        self.file_utils.write_displays(&self.displays)?;
        Ok(())
    }

    // pass in blank input to process_input to refresh
    fn refresh_configuration(&mut self) -> Result<()> {
        let blank_input = ProgramInput {
            brightness: None,
            toggle_nightlight: false,
            configure_display: false,
            reload_configuration: false,
            save_configuration: false
        };

        self.process_input(blank_input, None)
    }

    fn process_input(&mut self, program_input: ProgramInput, socket: Option<&mut UnixStream>) -> Result<()> {
        // avoided using destructuring because destructuring relies entirely on the order of the
        // struct elements
        let brightness = program_input.brightness;
        let toggle_nightlight = program_input.toggle_nightlight;
        let configure_display = program_input.configure_display;
        let reload_configuration = program_input.reload_configuration;
        let save_configuration = program_input.save_configuration;

        if toggle_nightlight {
            self.mode = !self.mode;

            #[cfg(feature = "redshift")]
            {
                if self.mode {
                    // turn on redshift
                    let mut redshift_enable = Command::new("redshift");
                    redshift_enable.arg("-O");
                    redshift_enable.arg("1400");
                    redshift_enable.spawn()?;
                }
                else {
                    // turn off redshift
                    let mut redshift_disable = Command::new("redshift");
                    redshift_disable.arg("-x");
                    redshift_disable.spawn()?;
                }
            }
        }

        match brightness {
            Some(brightness_change) => {
                self.brightness = match brightness_change {
                    BrightnessChange::Set(new_brightness) => new_brightness,
                    BrightnessChange::Adjustment(brightness_shift) => {
                        cmp::max(cmp::min(brightness_shift + (self.brightness as i8), 100), 0) as u8
                    }
                };

                let mut _call_handle = self.create_xrandr_command().spawn()?;

                #[cfg(feature = "auto-reconfigure")]
                {
                    let exit_status = _call_handle.wait()?;

                    // if the call fails, then the configuration is no longer valid
                    // reconfigures the display and then tries again
                    if !exit_status.success() {
                        println!("Reconfiguring!");
                        // force reconfigure
                        self.reconfigure_displays()?;
                        self.create_xrandr_command().spawn()?;

                        // if configure_display is true, dont want to reconfigure again
                        // instead of branching, just return early regardless
                        return Ok(());
                    }
                }
            },
            None => ()
        };

        if configure_display {
            self.reconfigure_displays()?;
        }

        if reload_configuration {
            if let Ok( (mut configuration_file, _) ) = self.file_utils.open_configuration_file() {
                let config_result = get_configuration_from_file(&mut configuration_file);

                // dont want to print success to stderr when there is no socket
                // since 'socket' is being moved into the closure below, the logic cannot check 'socket.is_some()'.
                // this boolean is needed to allow the logging to be skipped
                let socket_available = socket.is_some();

                let write_message = move |message: String| {
                    if let Some(socket) = socket {
                        if let Err(e) = socket.write_all(message.as_bytes()) {
                            eprintln!("Failed to write \"{}\" to socket: {}", message, e);
                        }
                    }
                    else {
                        eprintln!("{}", message);
                    }
                };

                match config_result {
                    Ok(config) => {
                        self.config = config;

                        // only log success if there is a socket
                        if socket_available {
                            write_message("Successfully reloaded configuration!".to_string());
                        }
                    }
                    Err(error) => {
                        write_message(format!("Failed to parse configuration file: {}", error));
                    }
                }
            }
            else {
                eprintln!("Failed to open configuration file for reloading!");
            }
        }

        if save_configuration {
            self.save_configuration()?;
        }

        Ok(())
    }

    fn reconfigure_displays(&mut self) -> Result<()> {
        let mut displays_file = self.file_utils.get_displays_file()?;
        let new_displays = configure_displays(&mut displays_file)?;

        // immutable update
        // self.displays.clear();
        // self.displays.clone_from(&new_displays);

        // mutable update
        self.displays = new_displays;
        Ok(())
    }

    fn create_xrandr_command(&self) -> Command {
        let mut xrandr_call = Command::new("xrandr");

        for display in &self.displays {
            xrandr_call.arg("--output");
            xrandr_call.arg(display);
        }

        let brightness_string = format!("{:.2}", *(&self.brightness) as f32 / 100.0);

        xrandr_call.arg("--brightness")
            .arg(brightness_string);

        #[cfg(not(feature = "redshift"))]
        {
            if self.mode {
                xrandr_call.arg("--gamma")
                    .arg("1.0:0.7:0.45");
            }
        }

        xrandr_call
    }
}

use DataValidatorResult::*;

fn overwrite_file_with_content<T>(file: &mut File, new_content: T) -> Result<()>
where T: Display {
    file.seek(std::io::SeekFrom::Start(0))?;

    let formatted_new_content = format!("{}", new_content);

    // <<NOTE>> this can overflow? len() returns a usize
    file.set_len(formatted_new_content.len() as u64)?;

    write!(file, "{}", formatted_new_content)?;

    Ok(())
}

// where ....
fn get_valid_data_or_write_default<T>(file: &mut File, data_validator: &dyn Fn(&String) -> Result<DataValidatorResult<T>>, default_value: T) -> Result<T>
where T: Display {
    let file_contents = {
        // wrapping in a closure allows the inner else and the
        // outer else clauses to share the same code
        // here, we want to return None if the file does not exist
        // or if the file's contents are not readable as a number
        (|| -> std::io::Result<DataValidatorResult<T>> {
            let mut buffer: Vec<u8> = Vec::new();
            file.read_to_end(&mut buffer)?;

            let string = unsafe {
                String::from_utf8_unchecked(buffer)
            };

            data_validator(&string)
        })()
    };

    if let Ok(Valid(contents)) = file_contents {
        Ok(contents)
    }
    else {
        let new_value = {
            if let Ok(Changed(new_value)) = file_contents {
                new_value
            }
            else {
                default_value
            }
        };

        overwrite_file_with_content(file, &new_value)?;

        Ok(new_value)
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

fn get_current_connected_displays() -> Result<Vec<String>> {
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

fn write_specified_displays_to_file(displays_file: &mut std::fs::File, connected_displays: &Vec<String>) -> Result<()> {
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
    displays_file.set_len(displays_file_length as u64)?;

    Ok(())
}

fn get_configuration_from_file(configuration_file: &mut File) -> std::result::Result<DaemonOptions, toml::de::Error> {
    let buffered_reader = BufReader::new(configuration_file);
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
    overwrite_values!(placeholder_option);

    return Ok(config);
}

fn configure_displays(displays_file: &mut std::fs::File) -> Result<Vec<String>> {
    let connected_displays = get_current_connected_displays()?;

    write_specified_displays_to_file(displays_file, &connected_displays)?;

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
    let project_directory = get_project_directory()?;

    let mut daemon = Daemon::new(project_directory)?;
    daemon.refresh_configuration()?;

    register_sigterm_handler()?;

    // enters the daemon event loop
    // (blocking)
    daemon.run()?;

    Ok(())
}
