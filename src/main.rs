#![cfg_attr(not(test), windows_subsystem = "windows")]

mod clock;
mod config;
mod fullscreen;
mod host;
mod idle;
mod overlay;
mod scheduler;
mod session;
mod state;
mod tray;
mod util;

fn main() {
    host::run();
}
