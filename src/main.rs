use directories::ProjectDirs;

use std::fs::{self, File, OpenOptions};
use std::io::{Error, ErrorKind, Seek, Write, Read, Result};
use std::fmt::Display;

// where ....
fn get_valid_data_or_write_default<T>(file: &mut File, data_validator: &dyn Fn(&String) -> Result<T>, default_value: T) -> Result<T>
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

            println!("Read in string: {}", string);

            data_validator(&string)
        })()
    };

    match file_contents {
        Ok(contents) => Ok(contents),
        Err(_) => {
            // default to "regular" mode
            file.seek(std::io::SeekFrom::Start(0))?;
            write!(file, "{}", &default_value)?;
            Ok(default_value)
        },
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

    let mode_filepath = cache_directory.join("mode");

    let mut file_open_options = OpenOptions::new();
    file_open_options.read(true);
    file_open_options.write(true);
    file_open_options.create(true);

    // 0 for regular
    // 1 for night light
    let mode = {
        let mode_exists = mode_filepath.exists();
        println!("Mode exists? {}", mode_exists);
        let mut mode_file = file_open_options.open(mode_filepath)?;
        mode_file.set_len(1)?;

        get_valid_data_or_write_default(&mut mode_file, &| data_in_file: &String | {
            if let Ok(num) = data_in_file.parse::<u8>() {
                if num == 0 || num == 1 {
                    println!("Successfully read in num: {}", num);
                    return Ok(num);
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid mode"));

        }, 0)?
    };


    println!("Mode is {}", mode);

    Ok(())
}
