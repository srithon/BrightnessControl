use directories::ProjectDirs;

use std::fs::{self, File, OpenOptions};
use std::io::{Error, ErrorKind, Seek, Write, Read};

fn main() -> Result<(), Error> {
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

        let file_contents = {
            // wrapping in a closure allows the inner else and the
            // outer else clauses to share the same code
            // here, we want to return None if the file does not exist
            // or if the file's contents are not readable as a number
            (|| -> std::io::Result<u8> {
                if mode_exists {
                    let mut mode_buffer: [u8; 1] = [0; 1];
                    mode_file.read_exact(&mut mode_buffer)?;

                    let mode_string = unsafe {
                        String::from_utf8_unchecked(mode_buffer.to_vec())
                    };

                    println!("Read in string: {}", mode_string);
                    if let Ok(num) = mode_string.parse::<u8>() {
                        if num == 0 || num == 1 {
                            println!("Successfully read in num: {}", num);
                            return Ok(num);
                        }
                    }
                }

                return Err(Error::new(ErrorKind::InvalidData, "Invalid mode"));
            })()
        };

        match file_contents {
            Ok(mode) => mode,
            Err(_) => {
                // default to "regular" mode
                mode_file.seek(std::io::SeekFrom::Start(0))?;
                write!(mode_file, "0")?;
                0 as u8
            },
        }
    };

    println!("Mode is {}", mode);

    Ok(())
}
