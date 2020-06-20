mod brightnessctl;

use std::io::{Result, Read, Write, Seek, SeekFrom};
use std::fs::{OpenOptions};
use std::process::Command;

fn main() -> Result<()> {
    let project_directory = brightnessctl::get_project_directory()?;
    let cache_dir = project_directory.cache_dir();

    let mode_filepath = cache_dir.join("mode");

    let mut file_open_options = OpenOptions::new();
    file_open_options.read(true);
    file_open_options.write(true);
    file_open_options.create(true);

    let mut mode_file = file_open_options.open(mode_filepath)?;

    let mut char_buffer: [u8; 1] = [0; 1];

    mode_file.read_exact(&mut char_buffer)?;

    let new_mode = {
        match char_buffer[0] as char {
            '0' => '1',
            _   => '0'
        }
    };

    mode_file.seek(SeekFrom::Start(0))?;
    write!(mode_file, "{}", new_mode)?;

    let mut command = Command::new("brightnessctl");
    command.spawn()?;

    Ok(())
}
