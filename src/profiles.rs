use crate::models::{
    AppSettings, JobPlanPreview, OverwriteMode, Profile, ProfileWithPath,
};
use crate::paths::get_profiles_dir;
use mslnk::ShellLink;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub fn normalize_opt(s: Option<String>) -> Option<String> {
    s.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(v)
        }
    })
}

pub fn normalize_ext(ext: Option<String>) -> Option<String> {
    ext.and_then(|v| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else if t.starts_with('.') {
            Some(t.to_string())
        } else {
            Some(format!(".{}", t))
        }
    })
}

pub fn load_profiles() -> Result<Vec<ProfileWithPath>, String> {
    let profiles_dir = get_profiles_dir()?;
    let mut profiles = Vec::new();

    let entries = fs::read_dir(&profiles_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(profile_data) = serde_json::from_str::<Profile>(&content) {
                    let stem = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    profiles.push(ProfileWithPath {
                        profile: Profile {
                            name: stem,
                            args: profile_data.args,
                            prefix: profile_data.prefix,
                            extension: profile_data.extension,
                            suffix: profile_data.suffix,
                        },
                        file_path: path.to_string_lossy().to_string(),
                    });
                }
            }
        }
    }

    profiles.sort_by(|a, b| a.profile.name.to_lowercase().cmp(&b.profile.name.to_lowercase()));
    Ok(profiles)
}

pub fn get_profile_by_name(name: &str) -> Result<ProfileWithPath, String> {
    let clean = name.trim().trim_end_matches(".json");
    let profiles = load_profiles()?;
    if let Some(found) = profiles.iter().find(|p| p.profile.name == clean) {
        return Ok(found.clone());
    }
    let clean_lower = clean.to_lowercase();
    if let Some(found) = profiles
        .iter()
        .find(|p| p.profile.name.to_lowercase() == clean_lower)
    {
        return Ok(found.clone());
    }
    Err(format!("Profile '{}' not found", clean))
}

pub fn save_profile(profile: &Profile) -> Result<PathBuf, String> {
    let name = profile.name.trim();
    if name.is_empty() {
        return Err("Profile name cannot be empty".to_string());
    }
    let profiles_dir = get_profiles_dir()?;
    let file_path = profiles_dir.join(format!("{}.json", name));
    let sanitized = Profile {
        name: name.to_string(),
        args: profile.args.clone(),
        prefix: normalize_opt(profile.prefix.clone()),
        extension: normalize_ext(profile.extension.clone()),
        suffix: profile.suffix.clone().filter(|s| !s.is_empty()),
    };
    let content = serde_json::to_string_pretty(&sanitized).map_err(|e| e.to_string())?;
    fs::write(&file_path, content).map_err(|e| e.to_string())?;
    Ok(file_path)
}

pub fn delete_profile(name: &str) -> Result<(), String> {
    let clean = name.trim().trim_end_matches(".json");
    let profiles_dir = get_profiles_dir()?;
    let exact = profiles_dir.join(format!("{}.json", clean));
    if exact.exists() {
        fs::remove_file(exact).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if let Ok(found) = get_profile_by_name(clean) {
        fs::remove_file(found.file_path).map_err(|e| e.to_string())?;
        return Ok(());
    }
    Err(format!("Profile '{}' not found", clean))
}

pub fn import_profile_files(paths: &[PathBuf]) -> Result<usize, String> {
    let mut imported = 0;
    for path in paths {
        let contents = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let parsed: Profile = serde_json::from_str(&contents)
            .map_err(|e| format!("Invalid profile JSON in {}: {}", path.display(), e))?;
        if parsed.name.trim().is_empty() {
            return Err(format!("Profile in {} is missing a name", path.display()));
        }
        save_profile(&parsed)?;
        imported += 1;
    }
    Ok(imported)
}

pub fn tokenize_args(raw: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_double = false;
    let mut in_single = false;
    let mut has_token = false;

    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if !in_single => {
                in_double = !in_double;
                has_token = true;
            }
            '\'' if !in_double => {
                in_single = !in_single;
                has_token = true;
            }
            c if c.is_whitespace() && !in_double && !in_single => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            other => {
                current.push(other);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(current);
    }
    args
}

pub fn format_command_line(program: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(quote_arg(program));
    for arg in args {
        parts.push(quote_arg(arg));
    }
    parts.join(" ")
}

fn quote_arg(s: &str) -> String {
    if s.is_empty() || s.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

pub fn builtin_templates() -> Vec<Profile> {
    vec![
        Profile {
            name: "Encode Clip Audio Merge AV1".to_string(),
            args: vec![
                "-filter_complex",
                "{{AUTO_AMERGE_LOUDNORM}}",
                "-map",
                "0:v",
                "-map",
                "[aout]",
                "-map",
                "0:a",
                "-c:v",
                "libsvtav1",
                "-color_trc",
                "bt709",
                "-color_primaries",
                "bt709",
                "-colorspace",
                "bt709",
                "-preset",
                "4",
                "-crf",
                "35",
                "-c:a",
                "libopus",
                "-b:a",
                "128k",
                "-ac:a:0",
                "2",
                "-disposition:a",
                "0",
                "-disposition:a:0",
                "default",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            prefix: None,
            extension: Some(".mkv".to_string()),
            suffix: Some(" [AV1{{MASTER_RAW}}]".to_string()),
        },
        Profile {
            name: "H.264 Fast (CRF 23)".to_string(),
            args: vec![
                "-c:v", "libx264", "-preset", "fast", "-crf", "23", "-c:a", "aac", "-b:a", "192k",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            prefix: None,
            extension: Some(".mp4".to_string()),
            suffix: Some(" [H264]".to_string()),
        },
        Profile {
            name: "HEVC High Efficiency (CRF 28)".to_string(),
            args: vec![
                "-c:v", "libx265", "-preset", "medium", "-crf", "28", "-tag:v", "hvc1", "-c:a",
                "aac", "-b:a", "160k",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            prefix: None,
            extension: Some(".mp4".to_string()),
            suffix: Some(" [HEVC]".to_string()),
        },
        Profile {
            name: "Remux to MP4 (Copy Streams)".to_string(),
            args: vec!["-map", "0", "-c", "copy", "-movflags", "+faststart"]
                .into_iter()
                .map(String::from)
                .collect(),
            prefix: None,
            extension: Some(".mp4".to_string()),
            suffix: Some(" [Remux]".to_string()),
        },
        Profile {
            name: "Extract Audio (Opus 160k)".to_string(),
            args: vec!["-vn", "-c:a", "libopus", "-b:a", "160k"]
                .into_iter()
                .map(String::from)
                .collect(),
            prefix: None,
            extension: Some(".opus".to_string()),
            suffix: None,
        },
    ]
}

pub fn resolve_job_plan(
    input_path: &str,
    profile: Option<&Profile>,
    raw_args: Option<&[String]>,
    extra_args: &[String],
    output_path_override: Option<&str>,
    prefix_override: Option<&str>,
    suffix_override: Option<&str>,
    ext_override: Option<&str>,
    settings: &AppSettings,
    audio_track_count: usize,
) -> Result<JobPlanPreview, String> {
    let base_args: Vec<String> = if let Some(raw) = raw_args {
        raw.to_vec()
    } else if let Some(p) = profile {
        p.args.clone()
    } else {
        return Err("Neither profile nor raw_args provided".to_string());
    };

    let in_path = Path::new(input_path);
    let default_dir = in_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let out_dir = match normalize_opt(settings.custom_output_dir.clone()) {
        Some(custom) => PathBuf::from(custom),
        None => default_dir,
    };

    let stem = in_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "output".to_string());

    let original_ext = in_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_else(|| ".mp4".to_string());

    let ext = ext_override
        .and_then(|s| normalize_ext(Some(s.to_string())))
        .or_else(|| profile.and_then(|p| normalize_ext(p.extension.clone())))
        .unwrap_or(original_ext);

    let prefix = prefix_override
        .map(|s| s.to_string())
        .or_else(|| profile.and_then(|p| p.prefix.clone()))
        .unwrap_or_default();

    let raw_suffix = suffix_override
        .map(|s| s.to_string())
        .or_else(|| profile.and_then(|p| p.suffix.clone()))
        .unwrap_or_default();

    let resolved_suffix = raw_suffix.replace(
        "{{MASTER_RAW}}",
        if audio_track_count > 1 {
            " Master+Raw"
        } else {
            ""
        },
    );

    let output_path = if let Some(override_path) = output_path_override.filter(|s| !s.trim().is_empty()) {
        PathBuf::from(override_path)
    } else {
        let mut candidate = out_dir.join(format!("{}{}{}{}", prefix, stem, resolved_suffix, ext));
        if settings.overwrite_mode == OverwriteMode::Rename && candidate.exists() {
            let mut idx = 1usize;
            loop {
                let next = out_dir.join(format!(
                    "{}{}{} ({}){}",
                    prefix, stem, resolved_suffix, idx, ext
                ));
                if !next.exists() {
                    candidate = next;
                    break;
                }
                idx += 1;
            }
        }
        candidate
    };

    let mut final_args: Vec<String> = Vec::new();
    for arg in &base_args {
        if arg.contains("{{AUTO_AMIX}}") || arg.contains("{{AUTO_AMERGE_LOUDNORM}}") {
            if audio_track_count > 1 {
                let mut inputs = String::new();
                for k in 0..audio_track_count {
                    inputs.push_str(&format!("[0:a:{}]", k));
                }
                if arg.contains("{{AUTO_AMIX}}") {
                    let expanded = arg.replace(
                        "{{AUTO_AMIX}}",
                        &format!("{}amix=inputs={}:duration=longest[aout]", inputs, audio_track_count),
                    );
                    final_args.push(expanded);
                } else {
                    let expanded = arg.replace(
                        "{{AUTO_AMERGE_LOUDNORM}}",
                        &format!("{}amerge=inputs={},loudnorm[aout]", inputs, audio_track_count),
                    );
                    final_args.push(expanded);
                }
            } else if final_args.last().map(|s| s.as_str()) == Some("-filter_complex") {
                final_args.pop();
            }
        } else if arg == "[aout]" {
            if audio_track_count > 1 {
                final_args.push(arg.clone());
            } else if final_args.last().map(|s| s.as_str()) == Some("-map") {
                final_args.pop();
            }
        } else {
            final_args.push(arg.clone());
        }
    }

    for extra in extra_args {
        final_args.push(extra.clone());
    }

    let mut cmd_args = Vec::new();
    let has_overwrite_flag = final_args.iter().any(|a| a == "-y" || a == "-n");
    if !has_overwrite_flag {
        match settings.overwrite_mode {
            OverwriteMode::Overwrite | OverwriteMode::Rename => cmd_args.push("-y".to_string()),
            OverwriteMode::Skip => cmd_args.push("-n".to_string()),
        }
    }
    cmd_args.push("-i".to_string());
    cmd_args.push(input_path.to_string());
    cmd_args.extend(final_args);
    let output_str = output_path.to_string_lossy().to_string();
    cmd_args.push(output_str.clone());

    let command_line = format_command_line("ffmpeg", &cmd_args);

    Ok(JobPlanPreview {
        input_path: input_path.to_string(),
        output_path: output_str,
        audio_track_count,
        args: cmd_args,
        command_line,
    })
}

pub fn generate_sendto_shortcuts() -> Result<String, String> {
    let exe_path = env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe_path
        .to_str()
        .ok_or_else(|| "Invalid executable path".to_string())?;
    let profiles_dir = get_profiles_dir()?;

    let appdata = env::var("APPDATA").map_err(|e| e.to_string())?;
    let sendto_dir = Path::new(&appdata)
        .join("Microsoft")
        .join("Windows")
        .join("SendTo");
    if !sendto_dir.exists() {
        return Err("SendTo directory not found".to_string());
    }

    let mut generated_count = 0usize;
    for entry in fs::read_dir(&profiles_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                let profile_name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let shortcut_name = format!("{}.lnk", profile_name);
                let shortcut_path = sendto_dir.join(shortcut_name);

                let mut sl = ShellLink::new(exe_str).map_err(|e| e.to_string())?;
                let args = format!("--profile \"{}\"", file_name);
                sl.set_arguments(Some(args));
                sl.set_icon_location(Some(exe_str.to_string()));
                sl.create_lnk(&shortcut_path).map_err(|e| e.to_string())?;
                generated_count += 1;
            }
        }
    }

    Ok(format!(
        "Generated {} shortcuts in SendTo folder ({}).",
        generated_count,
        sendto_dir.display()
    ))
}

pub fn is_media_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "ts" | "flv" | "m4v" | "wmv" | "mpg" | "mpeg"
            | "m2ts" | "mp3" | "wav" | "flac" | "aac" | "opus" | "m4a"
    )
}

pub fn collect_media_files(inputs: &[String], recursive: bool) -> Vec<String> {
    let mut results = Vec::new();
    for raw in inputs {
        let p = Path::new(raw);
        if p.is_dir() {
            collect_dir(p, recursive, &mut results);
        } else {
            results.push(raw.clone());
        }
    }
    results
}

fn collect_dir(dir: &Path, recursive: bool, out: &mut Vec<String>) {
    if let Ok(entries) = fs::read_dir(dir) {
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.is_dir() && recursive {
                collect_dir(&p, true, out);
            } else if p.is_file() && is_media_file(&p) {
                out.push(p.to_string_lossy().to_string());
            }
        }
    }
}
