use crate::models::{AppSettings, DaemonInfo, QueueSnapshot};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub fn exe_dir() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub fn get_app_data_dir() -> PathBuf {
    let base = dirs::data_local_dir()
        .unwrap_or_else(|| env::temp_dir())
        .join("FFmpeg Queue Manager");
    if !base.exists() {
        let _ = fs::create_dir_all(&base);
    }
    base
}

fn dir_has_json(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("json") {
                return true;
            }
        }
    }
    false
}

pub fn get_profiles_dir() -> Result<PathBuf, String> {
    if let Ok(override_dir) = env::var("FFMPEG_QUEUE_PROFILES_DIR") {
        let p = PathBuf::from(override_dir);
        if !p.exists() {
            fs::create_dir_all(&p).map_err(|e| e.to_string())?;
        }
        return Ok(p);
    }

    let local_profiles = exe_dir().join("Profiles");
    if dir_has_json(&local_profiles) {
        return Ok(local_profiles);
    }

    let appdata_profiles = get_app_data_dir().join("Profiles");
    if appdata_profiles.exists() {
        return Ok(appdata_profiles);
    }

    if !local_profiles.exists() {
        fs::create_dir_all(&local_profiles).map_err(|e| e.to_string())?;
    }
    Ok(local_profiles)
}

fn is_valid_binary(path: &Path) -> bool {
    path.is_file() && fs::metadata(path).map(|m| m.len() > 1024).unwrap_or(false)
}

pub fn resolve_binary(name: &str) -> PathBuf {
    let dir = exe_dir();
    let plain = format!("{}.exe", name);
    let triple = format!("{}-x86_64-pc-windows-msvc.exe", name);

    let mut candidates = vec![
        dir.join(&plain),
        dir.join("bin").join(&plain),
        dir.join(&triple),
        dir.join("bin").join(&triple),
    ];

    let mut cur = dir.as_path();
    for _ in 0..4 {
        candidates.push(cur.join("src-tauri").join("bin").join(&triple));
        candidates.push(cur.join("src-tauri").join("bin").join(&plain));
        candidates.push(cur.join("bin").join(&plain));
        candidates.push(cur.join("bin").join(&triple));
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }

    let app_dir = get_app_data_dir();
    candidates.push(app_dir.join(&plain));
    candidates.push(app_dir.join("bin").join(&plain));

    for c in candidates {
        if is_valid_binary(&c) {
            return c;
        }
    }

    PathBuf::from(plain)
}

pub fn settings_path() -> PathBuf {
    get_app_data_dir().join("settings.json")
}

pub fn queue_state_path() -> PathBuf {
    get_app_data_dir().join("queue_state.json")
}

pub fn daemon_info_path() -> PathBuf {
    get_app_data_dir().join("daemon.json")
}

pub fn load_settings() -> AppSettings {
    let p = settings_path();
    if let Ok(raw) = fs::read_to_string(&p) {
        if let Ok(parsed) = serde_json::from_str::<AppSettings>(&raw) {
            return parsed;
        }
    }
    AppSettings::default()
}

pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let p = settings_path();
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(p, json).map_err(|e| e.to_string())
}

pub fn load_queue_snapshot() -> Option<QueueSnapshot> {
    let raw = fs::read_to_string(queue_state_path()).ok()?;
    serde_json::from_str::<QueueSnapshot>(&raw).ok()
}

pub fn save_queue_snapshot(snapshot: &QueueSnapshot) -> Result<(), String> {
    let json = serde_json::to_string_pretty(snapshot).map_err(|e| e.to_string())?;
    fs::write(queue_state_path(), json).map_err(|e| e.to_string())
}

pub fn load_daemon_info() -> Option<DaemonInfo> {
    let raw = fs::read_to_string(daemon_info_path()).ok()?;
    serde_json::from_str::<DaemonInfo>(&raw).ok()
}

pub fn save_daemon_info(info: &DaemonInfo) {
    if let Ok(json) = serde_json::to_string_pretty(info) {
        let _ = fs::write(daemon_info_path(), json);
    }
}

pub fn remove_daemon_info() {
    let _ = fs::remove_file(daemon_info_path());
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "--:--".to_string();
    }
    let total = secs.floor() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const AUTOSTART_REG_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_VALUE_NAME: &str = "FFmpeg Queue Manager";

pub fn is_autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("reg")
            .args(["query", AUTOSTART_REG_KEY, "/v", AUTOSTART_VALUE_NAME])
            .creation_flags(0x0800_0000)
            .output();
        return matches!(out, Ok(o) if o.status.success());
    }
    #[cfg(not(windows))]
    false
}

pub fn set_autostart_enabled(enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if enabled {
            let exe = env::current_exe().map_err(|e| e.to_string())?;
            let val = format!("\"{}\" --minimized", exe.display());
            let out = std::process::Command::new("reg")
                .args([
                    "add",
                    AUTOSTART_REG_KEY,
                    "/v",
                    AUTOSTART_VALUE_NAME,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &val,
                    "/f",
                ])
                .creation_flags(0x0800_0000)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
        } else {
            let out = std::process::Command::new("reg")
                .args([
                    "delete",
                    AUTOSTART_REG_KEY,
                    "/v",
                    AUTOSTART_VALUE_NAME,
                    "/f",
                ])
                .creation_flags(0x0800_0000)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() && is_autostart_enabled() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
        }
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("Autostart is only supported on Windows".to_string())
    }
}

pub fn show_in_explorer(path: &str) {
    #[cfg(windows)]
    {
        let win_path = path.replace('/', "\\");
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", win_path))
            .spawn();
    }
}

pub fn open_folder_in_explorer(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

pub fn open_with_default_app(target: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", target])
            .creation_flags(0x0800_0000)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = target;
    }
}

#[derive(Clone, Debug)]
pub struct GitHubReleaseInfo {
    pub tag: String,
    pub html_url: String,
    pub setup_download_url: Option<String>,
}

pub fn fetch_latest_github_release() -> Result<GitHubReleaseInfo, String> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("curl.exe");
    cmd.args([
        "-sSL",
        "-H",
        "User-Agent: ffmpeg-queue-manager",
        "https://api.github.com/repos/EarthasaurusRex/ffmpeg-queue-manager/releases/latest",
    ]);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);

    let out = cmd
        .output()
        .map_err(|e| format!("Failed to run curl.exe: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "Failed to query GitHub releases: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("Invalid GitHub JSON: {}", e))?;
    if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
        return Err(format!("GitHub API: {}", msg));
    }
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "Missing tag_name in release response".to_string())?
        .to_string();
    let html_url = v
        .get("html_url")
        .and_then(|u| u.as_str())
        .unwrap_or("https://github.com/EarthasaurusRex/ffmpeg-queue-manager/releases/latest")
        .to_string();

    let setup_download_url = v
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                let name = asset.get("name")?.as_str()?;
                if name.to_ascii_lowercase().ends_with("-setup.exe") {
                    asset
                        .get("browser_download_url")
                        .and_then(|u| u.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
        });

    Ok(GitHubReleaseInfo {
        tag,
        html_url,
        setup_download_url,
    })
}

pub fn download_and_run_installer(download_url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        let dest = env::temp_dir().join("ffmpeg-queue-manager-update-setup.exe");
        let dest_str = dest.to_string_lossy().to_string();

        let mut dl_cmd = std::process::Command::new("curl.exe");
        dl_cmd.args([
            "-sSL",
            "-L",
            "-H",
            "User-Agent: ffmpeg-queue-manager",
            "-o",
            &dest_str,
            download_url,
        ]);
        dl_cmd.creation_flags(0x0800_0000);

        let out = dl_cmd
            .output()
            .map_err(|e| format!("Failed to run curl.exe: {}", e))?;
        if !out.status.success() {
            return Err(format!(
                "Failed to download installer: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }

        let size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        if !dest.is_file() || size <= 1024 * 100 {
            return Err(format!(
                "Downloaded installer file is invalid or too small ({} bytes)",
                size
            ));
        }

        remove_daemon_info();

        let current_exe = env::current_exe()
            .map_err(|e| format!("Failed to resolve current executable: {}", e))?;
        let exe_escaped = current_exe.to_string_lossy().replace('\'', "''");
        let dest_escaped = dest_str.replace('\'', "''");
        let ps_script = format!(
            "Start-Sleep -Milliseconds 600; Start-Process -FilePath '{}' -ArgumentList '/S' -Wait; Start-Process -FilePath '{}'",
            dest_escaped, exe_escaped
        );

        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps_script])
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| format!("Failed to launch updater script: {}", e))?;

        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let _ = download_url;
        Err("In-place NSIS installer update is only supported on Windows".to_string())
    }
}
