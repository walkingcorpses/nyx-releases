// main.rs - binary entry point. All the logic lives in the library (lib.rs) so tests can use it.
// `windows_subsystem = "windows"` keeps a console window from opening in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nyx_lib::run()
}
