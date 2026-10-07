// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Must run before the Tauri/GTK runtime starts; a no-op off Linux.
    riga_shell::prepare_linux_display();
    riga_demo::run()
}
