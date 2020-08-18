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

struct ProgramState<'a> {
    cache_directory: &'a std::path::Path,
    increment: i16,
    argument: String,
}

static file_open_options: OpenOptions = {
    let local_file_open_options = OpenOptions::new();
    local_file_open_options.read(true);
    local_file_open_options.write(true);
    local_file_open_options.create(true);
    local_file_open_options
};

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

// 0 for regular
// 1 for night light
fn get_mode(program_state: &ProgramState) -> Result<u8> {
    let mode_filepath = program_state.cache_directory.join("mode");

    let mut mode_file = file_open_options.open(mode_filepath)?;
    mode_file.set_len(1)?;

    let toggled_mode: Option<u8> = {
        if program_state.increment == 0 && program_state.argument.eq("--toggle") {
            Some((|| -> Result<u8> {
                // toggle code
                let mut char_buffer: [u8; 1] = [0; 1];

                mode_file.read_exact(&mut char_buffer)?;

                let new_mode = {
                    match char_buffer[0] as char {
                        '0' => 1,
                        _   => 0
                    }
                };

                mode_file.seek(SeekFrom::Start(0))?;
                write!(mode_file, "{}", new_mode)?;
                return Ok(new_mode);
            })()?)
        }
        else {
            None
        }
    };

    if let Some(mode) = toggled_mode {
        Ok(mode)
    }
    else {
        get_valid_data_or_write_default(&mut mode_file, &| data_in_file: &String | {
            if let Ok(num) = data_in_file.parse::<u8>() {
                if num == 0 || num == 1 {
                    return Ok(Valid(num));
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid mode"));

        }, 0)
    }
}

fn configure_displays(displays_file: &mut std::fs::File) -> Result<Vec<String>> {
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

fn get_displays(program_state: &ProgramState) -> Result<Vec<String>> {
    let displays_filepath = program_state.cache_directory.join("displays");

    let mut displays_file = file_open_options.open(displays_filepath)?;

    if program_state.argument.eq("--configure-display") {
        configure_displays(&mut displays_file)
    }
    else {
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
}

fn get_brightness(program_state: &ProgramState) -> Result<i16> {
    let brightness_filepath = program_state.cache_directory.join("brightness");

    let mut brightness_file = file_open_options.open(brightness_filepath)?;

    get_valid_data_or_write_default(&mut brightness_file, &| data_in_file: &String | {
        // need to trim this because the newline character breaks the parse
        if let Ok(num) = data_in_file.trim_end().parse::<i16>() {

            if num >= 0 {
                // ensure range of [0, 100]
                // <<NOTE>> weird behavior incase of overflow
                let new_num = cmp::max(cmp::min(program_state.increment + num, 100), 0);

                if num == new_num {
                    return Ok(Valid(num));
                }
                else {
                    return Ok(Changed(new_num));
                }
            }
        }

        return Err(Error::new(ErrorKind::InvalidData, "Invalid brightness"));
    }, 100)
}

fn main() -> Result<()> {
    // step 1
    // check if redshift mode or xrandr mode
    let project_directory = get_project_directory()?;
    let cache_directory = project_directory.cache_dir();

    let mut args = std::env::args();

    let arg = args.nth(1);
    let arg_unwrapped = arg.unwrap_or("".to_string());

    let increment = arg_unwrapped.parse::<i16>().unwrap_or(0);

    let brightness_filepath = cache_directory.join("brightness");

    let program_state = ProgramState {
        cache_directory,
        increment,
        argument: arg_unwrapped
    };

    let mode = get_mode(&program_state)?;
    let displays = get_displays(&program_state)?;
    let brightness = get_brightness(&program_state)?;

    let brightness_string = format!("{:.2}", brightness as f32 / 100.0);
    println!("Brightness: {}", brightness_string);

    let mut xrandr_call = {
        let mut xrandr_call = Command::new("xrandr");

        for display in displays {
            xrandr_call.arg("--output");
            xrandr_call.arg(display);
        }

        xrandr_call.arg("--brightness")
                   .arg(brightness_string);

        if mode == 1 {
            xrandr_call.arg("--gamma")
                       .arg("1.0:0.7:0.45");
        }

        xrandr_call
    };

    xrandr_call.spawn()?;

    Ok(())
}