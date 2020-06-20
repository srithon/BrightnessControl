use directories::ProjectDirs;

use std::fs::{self, File, OpenOptions};
use std::io::{Error, ErrorKind, Seek, Write, Read, Result};
use std::fmt::Display;

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

            println!("Read in string: {}", string);

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

        write!(file, "{}", new_value)?;
        Ok(new_value)
    }
}

fn main() -> Result<()> {
    println!("Hello, world!");
    // step 1
    // check if redshift mode or xrandr mode
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

    println!("{}", cache_directory.to_str().unwrap());

    let mut args = std::env::args();

    let increment = {
        let arg = args.nth(1);
        match arg {
            Some(num) => num.parse::<i16>().unwrap_or(0),
            None => 0,
        }
    };

    println!("increment: {}", increment);

    let mode_filepath = cache_directory.join("mode");
    let brightness_filepath = cache_directory.join("brightness");

    let mut file_open_options = OpenOptions::new();
    file_open_options.read(true);
    file_open_options.write(true);
    file_open_options.create(true);

    // 0 for regular
    // 1 for night light
    let mode = {
        let mut mode_file = file_open_options.open(mode_filepath)?;
        mode_file.set_len(1)?;

        get_valid_data_or_write_default(&mut mode_file, &| data_in_file: &String | {
            if let Ok(num) = data_in_file.parse::<u8>() {
                if num == 0 || num == 1 {
                    println!("Successfully read in num: {}", num);
                    return Ok(Valid(num));
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid mode"));

        }, 0)?
    };


    println!("Mode is {}", mode);

    let brightness = {
        let mut brightness_file = file_open_options.open(brightness_filepath)?;
        // brightness_file.set_len(3)?;

        get_valid_data_or_write_default(&mut brightness_file, &| data_in_file: &String | {
            // need to trim this because the newline character breaks the parse
            if let Ok(num) = data_in_file.trim_end().parse::<i16>() {
                println!("Brightness - Successfully read in num: {}", num);

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

    println!("Brightness is {}", brightness);

    Ok(())
}
