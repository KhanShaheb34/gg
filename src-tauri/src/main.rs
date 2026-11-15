// Prevents an additional console window on Windows in release. Do not remove.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    gg_desktop_lib::run()
}
