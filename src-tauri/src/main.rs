#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    trapclean_lib::run();
}
