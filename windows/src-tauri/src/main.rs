// Macha runs without a console window: Macha is the whole UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    macha_lib::run()
}
