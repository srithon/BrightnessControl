pub mod core;
pub mod fs;
pub mod config;
pub mod util;

pub fn start_daemon(fork: bool) -> std::io::Result<()> {
    core::core::daemon(fork)
}
