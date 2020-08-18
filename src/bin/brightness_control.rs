use directories::ProjectDirs;

use std::fs::{self, File, OpenOptions};
use std::io::{Error, ErrorKind, Seek, SeekFrom, Write, Read, Result};
use std::fmt::Display;
use std::process::Command;

use std::cmp;

enum DataValidatorResult<T> {
    Valid(T),
    Changed(T),
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

fn main() -> Result<()> {
    // step 1
    // check if redshift mode or xrandr mode
    let project_directory = get_project_directory()?;
    let cache_directory = project_directory.cache_dir();

    let mut args = std::env::args();

    let arg = args.nth(1);
    let arg_unwrapped = arg.unwrap_or("".to_string());

    let increment = arg_unwrapped.parse::<i16>().unwrap_or(0);

    let mode_filepath = cache_directory.join("mode");
    let brightness_filepath = cache_directory.join("brightness");

    let mut file_open_options = OpenOptions::new();
    file_open_options.read(true);
    file_open_options.write(true);
    file_open_options.create(true);

    // 0 for regular
    // 1 for night light
    let mut mode_file = file_open_options.open(mode_filepath)?;
    mode_file.set_len(1)?;

    let toggled_mode: Option<u8> = {
        if increment == 0 && arg_unwrapped.eq("--toggle") {
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
        match mode {
            0 => {
                // turn off redshift
                let mut redshift_disable = Command::new("redshift");
                redshift_disable.arg("-x");
                redshift_disable.spawn()?;
            },
            1 => {
                // turn on redshift
                let mut redshift_enable = Command::new("redshift");
                redshift_enable.arg("-r");
                redshift_enable.arg("-o");
                redshift_enable.arg("4700");
                redshift_enable.spawn()?;
            },
            _ => panic!("Mode is {}!?", mode)
        };

        mode
    }
    else {
        get_valid_data_or_write_default(&mut mode_file, &| data_in_file: &String | {
            if let Ok(num) = data_in_file.parse::<u8>() {
                if num == 0 || num == 1 {
                    return Ok(Valid(num));
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid mode"));

        }, 0)?
    };

    let brightness = {
        let mut brightness_file = file_open_options.open(brightness_filepath)?;
        // brightness_file.set_len(3)?;

        get_valid_data_or_write_default(&mut brightness_file, &| data_in_file: &String | {
            // need to trim this because the newline character breaks the parse
            if let Ok(num) = data_in_file.trim_end().parse::<i16>() {

                if num >= 0 {
                    // ensure range of [0, 100]
                    // <<NOTE>> weird behavior incase of overflow
                    let new_num = cmp::max(cmp::min(increment + num, 100), 0);

                    if num == new_num {
                        return Ok(Valid(num));
                    }
                    else {
                        return Ok(Changed(new_num));
                    }
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid brightness"));
        }, 100)?
    };

    let brightness_string = format!("{:.2}", brightness as f32 / 100.0);
    println!("Brightness: {}", brightness_string);

    let mut xrandr_call = {
        let mut xrandr_call = Command::new("xrandr");
        xrandr_call.arg("--output")
            .arg("eDP-1")
            .arg("--brightness")
            .arg(brightness_string);

        xrandr_call
    };

    xrandr_call.spawn()?;

    Ok(())
}
