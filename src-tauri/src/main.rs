#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if !eefi_browser_lib::run_update_helper() {
        eefi_browser_lib::run();
    }
}
