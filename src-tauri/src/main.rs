// Prevents additional console window on Windows
#![windows_subsystem = "windows"]

fn main() {
    ffmpeg_queue_manager_lib::run()
}
