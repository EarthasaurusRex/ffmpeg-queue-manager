use mslnk::ShellLink;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize, Deserialize, Clone)]
pub struct Profile {
    name: String,
    args: Vec<String>,
    #[serde(default)]
    extension: Option<String>,
    #[serde(default)]
    suffix: Option<String>,
}

fn get_profiles_dir() -> Result<PathBuf, String> {
    let exe_path = env::current_exe().map_err(|e| e.to_string())?;
    let exe_dir = exe_path.parent().unwrap();
    let profiles_dir = exe_dir.join("Profiles");
    if !profiles_dir.exists() {
        fs::create_dir_all(&profiles_dir).map_err(|e| e.to_string())?;
    }
    Ok(profiles_dir)
}

#[tauri::command]
fn get_profiles() -> Result<Vec<Profile>, String> {
    let profiles_dir = get_profiles_dir()?;
    let mut profiles = Vec::new();

    if let Ok(entries) = fs::read_dir(profiles_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(profile_data) = serde_json::from_str::<Profile>(&content) {
                        profiles.push(Profile {
                            name: path.file_stem().unwrap().to_string_lossy().to_string(),
                            args: profile_data.args,
                            extension: profile_data.extension,
                            suffix: profile_data.suffix,
                        });
                    }
                }
            }
        }
    }
    Ok(profiles)
}

#[tauri::command]
fn save_profile(
    name: String,
    args: Vec<String>,
    extension: Option<String>,
    suffix: Option<String>,
) -> Result<(), String> {
    let profiles_dir = get_profiles_dir()?;
    let file_path = profiles_dir.join(format!("{}.json", name));
    let profile = Profile {
        name: name.clone(),
        args,
        extension,
        suffix,
    };
    let content = serde_json::to_string_pretty(&profile).map_err(|e| e.to_string())?;
    fs::write(file_path, content).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn delete_profile(name: String) -> Result<(), String> {
    let profiles_dir = get_profiles_dir()?;
    let file_path = profiles_dir.join(format!("{}.json", name));
    if file_path.exists() {
        fs::remove_file(file_path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn generate_shortcuts(_app: AppHandle) -> Result<String, String> {
    let exe_path = env::current_exe().map_err(|e| e.to_string())?;
    let profiles_dir = get_profiles_dir()?;

    let appdata = env::var("APPDATA").map_err(|e| e.to_string())?;
    let sendto_dir = Path::new(&appdata)
        .join("Microsoft")
        .join("Windows")
        .join("SendTo");
    if !sendto_dir.exists() {
        return Err("SendTo directory not found".into());
    }

    let mut generated_count = 0;

    for entry in fs::read_dir(&profiles_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                let profile_name = path.file_stem().unwrap().to_str().unwrap();
                let shortcut_name = format!("{}.lnk", profile_name);
                let shortcut_path = sendto_dir.join(&shortcut_name);

                let mut sl =
                    ShellLink::new(exe_path.to_str().unwrap()).map_err(|e| e.to_string())?;
                let args = format!("--profile \"{}\"", file_name);
                sl.set_arguments(Some(args));
                sl.set_icon_location(Some(exe_path.to_str().unwrap().to_string()));
                sl.create_lnk(&shortcut_path).map_err(|e| e.to_string())?;
                generated_count += 1;
            }
        }
    }

    Ok(format!(
        "Generated {} shortcuts in SendTo folder.",
        generated_count
    ))
}

use tauri::menu::{Menu, MenuItem};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // Forward args to primary instance via event
            let _ = app.emit("queue-items", argv);
            // Focus main window
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            generate_shortcuts,
            get_profiles,
            save_profile,
            delete_profile
        ])
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                let _ = window.hide();
                api.prevent_close();
            }
            _ => {}
        })
        .setup(|app| {
            let _ = get_profiles_dir();

            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            let args: Vec<String> = env::args().collect();
            if args.len() > 1 {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let _ = app_handle.emit("queue-items", args);
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
