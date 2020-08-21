use getopts::Options;
use directories::ProjectDirs;

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Error, ErrorKind, Seek, SeekFrom, Write, Read, Result};
use std::fmt::Display;
use std::process::Command;

use std::cmp;

enum DataValidatorResult<T> {
    Valid(T),
    Changed(T),
}

pub enum BrightnessChange {
    Adjustment(i8),
    Set(u8)
}

pub struct ProgramInput {
    brightness: Option<BrightnessChange>,
    configure_display: bool,
    toggle_nightlight: bool
}

struct FileUtils<'a> {
    cache_directory: &'a std::path::Path,
    file_open_options: OpenOptions,
}

struct Daemon<'a> {
    brightness: u8,
    mode: u8,
    displays: Vec<String>,
    file_utils: FileUtils<'a>
}

impl<'a> Daemon<'a> {
    fn new() -> Daemon<'a> {
        let blank_input = ProgramInput {
            brightness: None,
            configure_display: false,
            toggle_nightlight: false
        };

        Daemon {
            brightness: 0,
            mode: 0,
            displays: Vec::new()
        }
    }

    fn open_cache_file(&self, file_name: &str) -> Result<File> {
        let filepath = self.file_utils.cache_directory.join(file_name);
        self.file_utils.file_open_options.open(filepath)
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
    fn get_written_mode(&self) -> Result<u8> {
        let mut mode_file = self.get_mode_file()?;
        mode_file.set_len(1)?;

        get_valid_data_or_write_default(&mut mode_file, &| data_in_file: &String | {
            if let Ok(num) = data_in_file.parse::<u8>() {
                if num == 0 || num == 1 {
                    return Ok(Valid(num));
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid mode"));

        }, 0)
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
                if num <= 100 && num >= 0 {
                    return Ok(Valid(num));
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid brightness"));
        }, 100)
    }

    fn create_xrandr_command(&self) -> Command {
        let mut xrandr_call = Command::new("xrandr");

        for display in &self.displays {
            xrandr_call.arg("--output");
            xrandr_call.arg(display);
        }

        xrandr_call.arg("--brightness")
            .arg(&self.brightness.to_string());

        #[cfg(not(feature = "redshift"))]
        {
            if *(&self.mode) == 1 {
                xrandr_call.arg("--gamma")
                    .arg("1.0:0.7:0.45");
            }
        }

        xrandr_call
    }
}

use DataValidatorResult::*;

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
        file.seek(std::io::SeekFrom::Start(0))?;
        let new_value = {
            if let Ok(Changed(new_value)) = file_contents {
                new_value
            }
            else {
                default_value
            }
        };

        let formatted_new_value = format!("{}", new_value);
        // <<NOTE>> this can overflow? len() returns a usize
        file.set_len(formatted_new_value.len() as u64)?;

        write!(file, "{}", formatted_new_value)?;
        Ok(new_value)
    }
}

pub fn get_project_directory() -> Result<directories::ProjectDirs> {
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

fn configure_displays(displays_file: &mut std::fs::File) -> Result<Vec<String>> {
    let connected_displays = get_current_connected_displays()?;

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

    Ok(connected_displays)
}


pub fn daemon() -> Result<()> {
    let project_directory = get_project_directory()?;
    let cache_directory = project_directory.cache_dir();

    let file_open_options = {
        let mut file_open_options = OpenOptions::new();
        file_open_options.read(true);
        file_open_options.write(true);
        file_open_options.create(true);
        file_open_options
    };

    let mode = get_mode(&program_state)?;
    let displays = get_displays(&program_state, false)?;
    let brightness = get_brightness(&program_state)?;

    let brightness_string = format!("{:.2}", brightness as f32 / 100.0);
    println!("Brightness: {}", brightness_string);

    let mut xrandr_call = create_xrandr_command(displays, &brightness_string, mode);

    // this variable is only used if the "auto-reconfigure" feature is enabled
    let mut _call_handle = xrandr_call.spawn()?;

    #[cfg(feature = "auto-reconfigure")]
    {
        let exit_status = _call_handle.wait()?;

        // if the call fails, then the configuration is no longer valid
        // reconfigures the display and then tries again
        if !exit_status.success() {
            println!("Reconfiguring!");
            // force reconfigure
            let displays = get_displays(&program_state, true)?;
            create_xrandr_command(displays, &brightness_string, mode).spawn()?;
        }
    }

    Ok(())
}
