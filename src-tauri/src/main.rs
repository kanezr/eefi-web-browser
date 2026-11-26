#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    eefi_browser_lib::run();
}
