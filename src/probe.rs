use crate::models::{AudioStreamInfo, ProbeResult, VideoStreamInfo};
use crate::paths::{format_duration, resolve_binary};
use serde_json::Value;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn probe_audio_track_count(file_path: &str) -> Result<(usize, Vec<String>), String> {
    let ffprobe = resolve_binary("ffprobe");
    let mut cmd = Command::new(&ffprobe);
    cmd.args([
        "-v",
        "error",
        "-select_streams",
        "a",
        "-show_entries",
        "stream=index",
        "-of",
        "csv=p=0",
        file_path,
    ]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to spawn ffprobe ({}): {}", ffprobe.display(), e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(format!("ffprobe failed: {}", stderr.trim()));
    }

    let mut log_lines = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            log_lines.push(trimmed.to_string());
        }
    }
    let count = log_lines.len();
    Ok((count, log_lines))
}

fn parse_fps(val: Option<&str>) -> Option<String> {
    let s = val?;
    if s.is_empty() || s == "0/0" {
        return None;
    }
    if let Some((num_s, den_s)) = s.split_once('/') {
        let num: f64 = num_s.parse().ok()?;
        let den: f64 = den_s.parse().ok()?;
        if den > 0.0 && num > 0.0 {
            return Some(format!("{:.2}", num / den));
        }
    }
    Some(s.to_string())
}

pub fn probe_media_detailed(file_path: &str) -> Result<ProbeResult, String> {
    let ffprobe = resolve_binary("ffprobe");
    let mut cmd = Command::new(&ffprobe);
    cmd.args([
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        file_path,
    ]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to spawn ffprobe ({}): {}", ffprobe.display(), e))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffprobe error: {}", err.trim()));
    }

    let root: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse ffprobe JSON: {}", e))?;

    let fmt = root.get("format");
    let format_name = fmt
        .and_then(|f| f.get("format_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let duration_secs = fmt
        .and_then(|f| f.get("duration"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    let size_bytes = fmt
        .and_then(|f| f.get("size"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| std::fs::metadata(file_path).ok().map(|m| m.len()))
        .unwrap_or(0);

    let bit_rate = fmt
        .and_then(|f| f.get("bit_rate"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok());

    let mut video_streams = Vec::new();
    let mut audio_streams = Vec::new();

    if let Some(streams) = root.get("streams").and_then(|s| s.as_array()) {
        for s in streams {
            let codec_type = s.get("codec_type").and_then(|v| v.as_str()).unwrap_or("");
            let index = s.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let codec = s
                .get("codec_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            if codec_type == "video" {
                let width = s.get("width").and_then(|v| v.as_u64()).map(|n| n as u32);
                let height = s.get("height").and_then(|v| v.as_u64()).map(|n| n as u32);
                let fps = parse_fps(
                    s.get("avg_frame_rate")
                        .and_then(|v| v.as_str())
                        .filter(|&x| x != "0/0")
                        .or_else(|| s.get("r_frame_rate").and_then(|v| v.as_str())),
                );
                let pix_fmt = s
                    .get("pix_fmt")
                    .and_then(|v| v.as_str())
                    .map(|x| x.to_string());
                let color_space = s
                    .get("color_space")
                    .and_then(|v| v.as_str())
                    .map(|x| x.to_string());

                video_streams.push(VideoStreamInfo {
                    index,
                    codec,
                    width,
                    height,
                    fps,
                    pix_fmt,
                    color_space,
                });
            } else if codec_type == "audio" {
                let channels = s.get("channels").and_then(|v| v.as_u64()).map(|n| n as u32);
                let channel_layout = s
                    .get("channel_layout")
                    .and_then(|v| v.as_str())
                    .map(|x| x.to_string());
                let sample_rate = s
                    .get("sample_rate")
                    .and_then(|v| v.as_str())
                    .map(|x| x.to_string());
                let stream_bitrate = s
                    .get("bit_rate")
                    .and_then(|v| v.as_str())
                    .map(|x| x.to_string());
                let tags = s.get("tags");
                let language = tags
                    .and_then(|t| t.get("language"))
                    .and_then(|v| v.as_str())
                    .map(|x| x.to_string());
                let title = tags
                    .and_then(|t| t.get("title"))
                    .and_then(|v| v.as_str())
                    .map(|x| x.to_string());

                audio_streams.push(AudioStreamInfo {
                    index,
                    codec,
                    channels,
                    channel_layout,
                    sample_rate,
                    bit_rate: stream_bitrate,
                    language,
                    title,
                });
            }
        }
    }

    Ok(ProbeResult {
        file_path: file_path.to_string(),
        format_name,
        duration_secs,
        duration_formatted: format_duration(duration_secs),
        size_bytes,
        bit_rate,
        video_streams,
        audio_streams,
    })
}
