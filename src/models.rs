use serde::{Deserialize, Serialize};
use std::fmt;

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum JobStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Error,
    Cancelled,
}

impl JobStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Error | Self::Cancelled)
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Paused)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Completed => "Completed",
            Self::Error => "Error",
            Self::Cancelled => "Cancelled",
        }
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum JobPriority {
    Low,
    #[default]
    Normal,
    High,
}

impl JobPriority {
    pub fn rank(&self) -> u8 {
        match self {
            Self::High => 2,
            Self::Normal => 1,
            Self::Low => 0,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Normal => "Normal",
            Self::High => "High",
        }
    }
}

impl fmt::Display for JobPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub file_path: String,
    #[serde(default)]
    pub output_path: Option<String>,
    pub profile_name: String,
    #[serde(default)]
    pub raw_args: Option<Vec<String>>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub prefix_override: Option<String>,
    #[serde(default)]
    pub suffix_override: Option<String>,
    #[serde(default)]
    pub ext_override: Option<String>,
    #[serde(default)]
    pub priority: JobPriority,
    pub status: JobStatus,
    pub progress: f32,
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub expanded_terminal: bool,
    #[serde(default)]
    pub fps: Option<String>,
    #[serde(default)]
    pub bitrate: Option<String>,
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub eta: Option<String>,
    #[serde(default)]
    pub est_size: Option<String>,
    #[serde(default)]
    pub compression_ratio: Option<String>,
    #[serde(default)]
    pub input_size_bytes: Option<u64>,
    #[serde(default)]
    pub output_size_bytes: Option<u64>,
    #[serde(default)]
    pub resolved_command: Option<String>,
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub finished_at: Option<u64>,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub extension: Option<String>,
    #[serde(default)]
    pub suffix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileWithPath {
    #[serde(flatten)]
    pub profile: Profile,
    pub file_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum OverwriteMode {
    #[default]
    Overwrite,
    Skip,
    Rename,
}

impl OverwriteMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Overwrite => "Overwrite (-y)",
            Self::Skip => "Skip (-n)",
            Self::Rename => "Auto-Rename",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub max_concurrency: usize,
    pub auto_clear: bool,
    pub minimize_to_tray: bool,
    pub dark_mode: bool,
    pub overwrite_mode: OverwriteMode,
    pub custom_output_dir: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            max_concurrency: 2,
            auto_clear: false,
            minimize_to_tray: true,
            dark_mode: true,
            overwrite_mode: OverwriteMode::Overwrite,
            custom_output_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoStreamInfo {
    pub index: usize,
    pub codec: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<String>,
    pub pix_fmt: Option<String>,
    pub color_space: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub index: usize,
    pub codec: String,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    pub sample_rate: Option<String>,
    pub bit_rate: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub file_path: String,
    pub format_name: String,
    pub duration_secs: f64,
    pub duration_formatted: String,
    pub size_bytes: u64,
    pub bit_rate: Option<u64>,
    pub video_streams: Vec<VideoStreamInfo>,
    pub audio_streams: Vec<AudioStreamInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobPlanPreview {
    pub input_path: String,
    pub output_path: String,
    pub audio_track_count: usize,
    pub args: Vec<String>,
    pub command_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueueSpec {
    pub file_path: String,
    pub profile_name: String,
    #[serde(default)]
    pub raw_args: Option<Vec<String>>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub output_path_override: Option<String>,
    #[serde(default)]
    pub prefix_override: Option<String>,
    #[serde(default)]
    pub suffix_override: Option<String>,
    #[serde(default)]
    pub ext_override: Option<String>,
    #[serde(default)]
    pub priority: JobPriority,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueSnapshot {
    pub version: String,
    pub paused: bool,
    pub active_runners: usize,
    pub max_concurrency: usize,
    pub jobs: Vec<Job>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub port: u16,
    pub pid: u32,
}
