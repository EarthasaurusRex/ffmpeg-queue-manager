use crate::engine::QueueEngine;
use crate::ipc::{
    ensure_daemon_and_send, is_daemon_running, try_send, IpcRequest, IpcResponse,
};
use crate::models::{
    AppSettings, EnqueueSpec, Job, JobPriority, JobStatus, OverwriteMode, Profile, APP_VERSION,
};
use crate::paths::{
    format_bytes, get_profiles_dir, is_autostart_enabled, load_settings, save_settings,
    set_autostart_enabled, settings_path,
};
use crate::probe::probe_media_detailed;
use crate::profiles::{
    collect_media_files, delete_profile, generate_sendto_shortcuts, get_profile_by_name,
    import_profile_files, load_profiles, resolve_job_plan, save_profile, tokenize_args,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::Duration;

#[cfg(windows)]
pub fn attach_parent_console() {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    unsafe {
        let h_out: HANDLE = GetStdHandle(STD_OUTPUT_HANDLE);
        let h_err: HANDLE = GetStdHandle(STD_ERROR_HANDLE);
        let out_missing = h_out.is_null() || h_out == INVALID_HANDLE_VALUE;
        let err_missing = h_err.is_null() || h_err == INVALID_HANDLE_VALUE;
        if out_missing && err_missing {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(windows))]
pub fn attach_parent_console() {}

#[derive(Debug, Clone)]
pub struct GuiLaunchPlan {
    pub minimized: bool,
    pub enqueue_profile: Option<String>,
    pub enqueue_files: Vec<String>,
}

/// Determine whether the binary was invoked purely for GUI / Explorer SendTo
/// (where we must NOT touch or flash a Windows console) vs CLI execution.
pub fn detect_gui_only_launch(args: &[String]) -> Option<GuiLaunchPlan> {
    if args.len() <= 1 {
        return Some(GuiLaunchPlan {
            minimized: false,
            enqueue_profile: None,
            enqueue_files: Vec::new(),
        });
    }

    let rest = &args[1..];
    let subcommands = [
        "queue",
        "q",
        "jobs",
        "profiles",
        "profile",
        "p",
        "probe",
        "run",
        "settings",
        "shortcuts",
        "agent-help",
        "ai-help",
        "guide",
        "list-profiles",
        "enqueue",
        "status",
        "help",
    ];
    let cli_flags = [
        "-h",
        "--help",
        "-V",
        "--version",
        "-j",
        "--json",
        "-w",
        "--wait",
        "--headless",
    ];

    for a in rest {
        if cli_flags.contains(&a.as_str()) {
            return None;
        }
        if subcommands.contains(&a.as_str()) {
            // Careful: could someone name a file "status" without extension? Unlikely, subcommand takes precedence.
            return None;
        }
    }

    let mut minimized = false;
    let mut explicit_gui = false;
    let mut profile: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--gui" => explicit_gui = true,
            "--minimized" => {
                explicit_gui = true;
                minimized = true;
            }
            "--profile" | "-p" => {
                if i + 1 < rest.len() {
                    profile = Some(rest[i + 1].clone());
                    i += 1;
                } else {
                    return None;
                }
            }
            arg if arg.starts_with('-') => return None,
            file => files.push(file.to_string()),
        }
        i += 1;
    }

    if explicit_gui && profile.is_none() && files.is_empty() {
        return Some(GuiLaunchPlan {
            minimized,
            enqueue_profile: None,
            enqueue_files: Vec::new(),
        });
    }

    if profile.is_some() && !files.is_empty() {
        return Some(GuiLaunchPlan {
            minimized,
            enqueue_profile: profile,
            enqueue_files: files,
        });
    }

    None
}

const AFTER_HELP_GUIDE: &str = r#"AI AGENT & AUTOMATION QUICK GUIDE:
  1. Discover available encoding profiles:
       ffmpeg-queue-manager profiles list --json
  2. Inspect a media file and preview the resolved FFmpeg command before running:
       ffmpeg-queue-manager probe "video.mp4" --profile "Encode Clip Audio Merge AV1" --json
  3. Add job(s) to the GUI queue (starts background GUI if needed) and wait for completion:
       ffmpeg-queue-manager queue add -p "Encode Clip Audio Merge AV1" -w --json "video.mp4"
  4. Or run a custom one-off FFmpeg encode headlessly without saving a profile:
       ffmpeg-queue-manager run --raw-args "-c:v libx264 -crf 23 -c:a aac" --ext .mp4 --json "video.mp4"
  5. Monitor or control running GUI queue jobs:
       ffmpeg-queue-manager queue list --json
       ffmpeg-queue-manager queue logs job-1 -n 50 --json
       ffmpeg-queue-manager queue stop job-1
  6. Read the complete AI integration manual (schemas, template tokens, exit codes):
       ffmpeg-queue-manager agent-help

EXIT CODES:
  0  Success
  1  Runtime / validation / IPC / file error
  2  Invalid CLI arguments or usage error
  3  Wait/Run completed with one or more failed or cancelled encoding jobs"#;

#[derive(Parser, Debug)]
#[command(
    name = "ffmpeg-queue-manager",
    version = APP_VERSION,
    about = "Fast Pure-Rust FFmpeg Queue Manager with Desktop GUI and AI-first CLI.",
    long_about = "FFmpeg Queue Manager manages concurrent FFmpeg encoding queues, dynamic multi-track audio probing (ffprobe), and reusable JSON profiles.\n\nWhen invoked with no arguments (or --gui / --minimized), it launches the native desktop GUI. When invoked with subcommands or --json/--wait/--headless, it acts as a full automation & AI-agent CLI communicating with the GUI over local TCP IPC or running headlessly.",
    after_long_help = AFTER_HELP_GUIDE
)]
pub struct Cli {
    /// Output structured machine-readable JSON (recommended for AI agents and scripts)
    #[arg(short = 'j', long = "json", global = true)]
    pub json: bool,

    /// Launch the graphical user interface
    #[arg(long = "gui")]
    pub gui: bool,

    /// Start the GUI minimized to the Windows system tray
    #[arg(long = "minimized")]
    pub minimized: bool,

    /// Quick enqueue / SendTo compatibility: profile name to apply to FILES
    #[arg(short = 'p', long = "profile", value_name = "NAME")]
    pub profile: Option<String>,

    /// Quick enqueue / SendTo compatibility: input media file(s)
    #[arg(value_name = "FILES")]
    pub files: Vec<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Enqueue, inspect, monitor, pause, stop, retry, or clear queue jobs
    #[command(aliases = ["q", "jobs"])]
    Queue {
        #[command(subcommand)]
        action: QueueCommands,
    },

    /// List, inspect, create, import, or delete FFmpeg encoding profiles
    #[command(aliases = ["profile", "p"])]
    Profiles {
        #[command(subcommand)]
        action: ProfileCommands,
    },

    /// Inspect video/audio streams using ffprobe and optionally preview the resolved FFmpeg command
    Probe(ProbeArgs),

    /// Synchronously execute FFmpeg on file(s) in-process without starting the GUI daemon
    Run(RunArgs),

    /// View or update persistent queue and output settings
    Settings {
        #[command(subcommand)]
        action: SettingsCommands,
    },

    /// Generate Windows Explorer "Send To" shortcuts for all saved profiles
    Shortcuts,

    /// Print the complete AI Agent Reference Manual (schemas, tokens, workflows, exit codes)
    #[command(name = "agent-help", aliases = ["ai-help", "guide"])]
    AgentHelp,

    /// Convenience alias for `profiles list`
    #[command(name = "list-profiles")]
    ListProfiles,

    /// Convenience alias for `queue add`
    Enqueue(QueueAddArgs),

    /// Convenience alias for `queue list`
    Status(QueueListArgs),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CliPriority {
    Low,
    Normal,
    High,
}

impl From<CliPriority> for JobPriority {
    fn from(v: CliPriority) -> Self {
        match v {
            CliPriority::Low => JobPriority::Low,
            CliPriority::Normal => JobPriority::Normal,
            CliPriority::High => JobPriority::High,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CliStatusFilter {
    Queued,
    Running,
    Paused,
    Completed,
    Error,
    Cancelled,
}

impl CliStatusFilter {
    fn matches(&self, s: JobStatus) -> bool {
        matches!(
            (self, s),
            (Self::Queued, JobStatus::Queued)
                | (Self::Running, JobStatus::Running)
                | (Self::Paused, JobStatus::Paused)
                | (Self::Completed, JobStatus::Completed)
                | (Self::Error, JobStatus::Error)
                | (Self::Cancelled, JobStatus::Cancelled)
        )
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CliOverwriteMode {
    Overwrite,
    Skip,
    Rename,
}

impl From<CliOverwriteMode> for OverwriteMode {
    fn from(v: CliOverwriteMode) -> Self {
        match v {
            CliOverwriteMode::Overwrite => OverwriteMode::Overwrite,
            CliOverwriteMode::Skip => OverwriteMode::Skip,
            CliOverwriteMode::Rename => OverwriteMode::Rename,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum QueueCommands {
    /// Add one or more media files (or directories with -r) to the encoding queue
    #[command(
        aliases = ["enqueue", "push"],
        long_about = "Enqueue media file(s) using either a named saved profile (-p/--profile) or custom arguments (--raw-args). Automatically connects to the running GUI over IPC, or auto-launches the GUI unless --headless is specified."
    )]
    Add(QueueAddArgs),

    /// List current jobs in the queue with status, progress, ETA, estimated size, and final size ratio
    #[command(aliases = ["status", "ls", "ps"])]
    List(QueueListArgs),

    /// Show full metadata, resolved FFmpeg command, sizes, and status for a specific job ID
    #[command(alias = "info")]
    Inspect {
        /// Job ID (e.g. job-1)
        job_id: String,
    },

    /// Fetch or live-tail FFprobe/FFmpeg logs for a job ID
    Logs {
        /// Job ID (e.g. job-1)
        job_id: String,
        /// Show only the last N lines
        #[arg(short = 'n', long = "lines")]
        lines: Option<usize>,
        /// Continuously follow new log lines until the job reaches a terminal state
        #[arg(short = 'f', long = "follow")]
        follow: bool,
    },

    /// Block until specific job ID(s) or all active jobs finish encoding
    Wait {
        /// Specific job ID(s) to wait on (omit and pass --all to wait for the entire queue)
        job_ids: Vec<String>,
        /// Wait until all currently Queued/Running/Paused jobs finish
        #[arg(long = "all")]
        all: bool,
    },

    /// Stop a specific running/queued job ID, or pass "all" to stop all active jobs
    #[command(aliases = ["kill", "cancel"])]
    Stop {
        /// Job ID (e.g. job-1) or "all"
        job_id_or_all: String,
    },

    /// Re-queue a failed/cancelled/completed job ID, or pass "failed" to retry all failed/cancelled jobs
    Retry {
        /// Job ID (e.g. job-1) or "failed"
        job_id_or_failed: String,
    },

    /// Remove non-running jobs from the queue
    Clear {
        /// Clear all Completed jobs
        #[arg(long)]
        completed: bool,
        /// Clear all Error and Cancelled jobs
        #[arg(long)]
        failed: bool,
        /// Clear all non-running jobs
        #[arg(long)]
        all: bool,
        /// Clear a specific job ID
        #[arg(long)]
        id: Option<String>,
    },

    /// Pause scheduling new queued jobs (currently running FFmpeg processes continue)
    Pause,

    /// Resume scheduling queued jobs
    Resume,
}

#[derive(Args, Debug)]
pub struct QueueAddArgs {
    /// Name of saved profile to apply (see `profiles list`). Required unless --raw-args is provided.
    #[arg(short = 'p', long = "profile", value_name = "NAME")]
    pub profile: Option<String>,

    /// Raw FFmpeg output arguments string for ad-hoc encoding without a saved profile (e.g. "-c:v libx264 -crf 23 -c:a aac")
    #[arg(long = "raw-args", value_name = "ARGS", allow_hyphen_values = true)]
    pub raw_args: Option<String>,

    /// Additional FFmpeg arguments appended to the profile's arguments
    #[arg(long = "extra-args", value_name = "ARGS", allow_hyphen_values = true)]
    pub extra_args: Option<String>,

    /// Override output filename prefix (pass "" to force no prefix)
    #[arg(long = "prefix", value_name = "STR", allow_hyphen_values = true)]
    pub prefix: Option<String>,

    /// Override output filename suffix (supports {{MASTER_RAW}}; pass "" to force no suffix)
    #[arg(long = "suffix", value_name = "STR", allow_hyphen_values = true)]
    pub suffix: Option<String>,

    /// Override output file extension (e.g. .mkv or mp4)
    #[arg(long = "ext", value_name = "EXT")]
    pub ext: Option<String>,

    /// Override exact output file path (only valid when queuing a single input file)
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    pub output: Option<String>,

    /// Scheduling priority in the queue
    #[arg(long = "priority", value_enum, default_value_t = CliPriority::Normal)]
    pub priority: CliPriority,

    /// Recursively scan any directory paths in FILES for media files
    #[arg(short = 'r', long = "recursive")]
    pub recursive: bool,

    /// Wait until all newly added jobs finish encoding before returning
    #[arg(short = 'w', long = "wait")]
    pub wait: bool,

    /// Execute in-process in CLI mode (if GUI daemon is not running) without launching a window
    #[arg(long = "headless")]
    pub headless: bool,

    /// If auto-launching the GUI daemon, start it minimized to the system tray
    #[arg(long = "minimized")]
    pub minimized: bool,

    /// Input media file(s) or folder(s)
    #[arg(required = true, value_name = "FILES")]
    pub files: Vec<String>,
}

#[derive(Args, Debug)]
pub struct QueueListArgs {
    /// Filter jobs by status
    #[arg(long = "status", value_enum)]
    pub status: Option<CliStatusFilter>,
}

#[derive(Subcommand, Debug)]
pub enum ProfileCommands {
    /// List all saved encoding profiles
    #[command(alias = "ls")]
    List,

    /// Inspect a single profile and see its resolved JSON definition
    #[command(aliases = ["get", "inspect"])]
    Show {
        /// Profile name (with or without .json)
        name: String,
    },

    /// Create or overwrite a saved JSON encoding profile
    #[command(aliases = ["save", "add", "set"])]
    Create {
        /// Profile name (becomes <NAME>.json in the Profiles directory)
        #[arg(short = 'n', long = "name")]
        name: String,
        /// FFmpeg arguments string (tokens inside quotes are preserved)
        #[arg(short = 'a', long = "args", allow_hyphen_values = true)]
        args: String,
        /// Optional filename prefix
        #[arg(long = "prefix", allow_hyphen_values = true)]
        prefix: Option<String>,
        /// Optional filename suffix (supports {{MASTER_RAW}})
        #[arg(long = "suffix", allow_hyphen_values = true)]
        suffix: Option<String>,
        /// Optional output extension (e.g. .mkv, .mp4)
        #[arg(long = "ext")]
        ext: Option<String>,
    },

    /// Delete a saved profile
    #[command(aliases = ["rm", "remove"])]
    Delete {
        /// Profile name to delete
        name: String,
    },

    /// Import one or more external profile .json files into the Profiles folder
    Import {
        /// Path(s) to profile JSON files
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// Print the active Profiles directory path
    Path,
}

#[derive(Args, Debug)]
pub struct ProbeArgs {
    /// Media file path to inspect via ffprobe
    pub file: String,

    /// Optional profile name to dry-run output path and FFmpeg command expansion
    #[arg(short = 'p', long = "profile", value_name = "NAME")]
    pub profile: Option<String>,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Profile name to apply (or pass --raw-args)
    #[arg(short = 'p', long = "profile", value_name = "NAME")]
    pub profile: Option<String>,

    /// Raw FFmpeg arguments string for ad-hoc execution
    #[arg(long = "raw-args", value_name = "ARGS", allow_hyphen_values = true)]
    pub raw_args: Option<String>,

    /// Extra FFmpeg arguments appended to profile args
    #[arg(long = "extra-args", value_name = "ARGS", allow_hyphen_values = true)]
    pub extra_args: Option<String>,

    /// Override output filename prefix
    #[arg(long = "prefix", allow_hyphen_values = true)]
    pub prefix: Option<String>,

    /// Override output filename suffix
    #[arg(long = "suffix", allow_hyphen_values = true)]
    pub suffix: Option<String>,

    /// Override output extension
    #[arg(long = "ext")]
    pub ext: Option<String>,

    /// Override exact output file path (single file only)
    #[arg(short = 'o', long = "output")]
    pub output: Option<String>,

    /// Recursively scan directories in FILES
    #[arg(short = 'r', long = "recursive")]
    pub recursive: bool,

    /// Input media file(s) or folder(s)
    #[arg(required = true, value_name = "FILES")]
    pub files: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum SettingsCommands {
    /// Show current application settings
    Get,
    /// Update application settings
    Set {
        /// Maximum simultaneous FFmpeg jobs (1..=16)
        #[arg(long = "max-concurrency")]
        max_concurrency: Option<usize>,
        /// Automatically clear Completed jobs after 5 seconds (true/false)
        #[arg(long = "auto-clear")]
        auto_clear: Option<bool>,
        /// Register or unregister Windows Startup autostart (true/false)
        #[arg(long = "autostart")]
        autostart: Option<bool>,
        /// Minimize to Windows System Tray when closing the window (true/false)
        #[arg(long = "minimize-to-tray")]
        minimize_to_tray: Option<bool>,
        /// Use dark theme in GUI (true/false)
        #[arg(long = "dark-mode")]
        dark_mode: Option<bool>,
        /// Output overwrite policy when target file already exists
        #[arg(long = "overwrite-mode", value_enum)]
        overwrite_mode: Option<CliOverwriteMode>,
        /// Custom output directory path (pass "none" to save alongside source files)
        #[arg(long = "output-dir")]
        output_dir: Option<String>,
    },
}

#[derive(Serialize)]
struct SettingsReport {
    #[serde(flatten)]
    settings: AppSettings,
    autostart_enabled: bool,
    settings_file: String,
    profiles_dir: String,
}

#[derive(Serialize)]
struct ProbeReport {
    probe: crate::models::ProbeResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<crate::models::JobPlanPreview>,
}

const AGENT_HELP_MANUAL: &str = r#"================================================================================
FFmpeg Queue Manager v0.2.0 — AI Agent Reference Manual
================================================================================

OVERVIEW
  FFmpeg Queue Manager is a native Windows queue runner and profile manager for
  FFmpeg and FFprobe. All CLI commands support `--json` (`-j`) for deterministic,
  parseable output, and exit cleanly with semantic exit codes.

EXIT CODES
  0 : Operation completed successfully.
  1 : Validation, runtime, file-not-found, or IPC error.
  2 : CLI argument syntax error.
  3 : Encoding finished with one or more Error or Cancelled jobs (`--wait`/`run`).

DYNAMIC PROFILE TEMPLATE TOKENS
  Profiles are stored as JSON in the Profiles folder (`profiles path`):
    {
      "name": "Encode Clip Audio Merge AV1",
      "args": ["-filter_complex", "{{AUTO_AMERGE_LOUDNORM}}", "-map", "0:v", "-map", "[aout]", ...],
      "prefix": null,
      "extension": ".mkv",
      "suffix": " [AV1{{MASTER_RAW}}]"
    }

  Automatic ffprobe expansion happens before FFmpeg runs:
  * {{AUTO_AMERGE_LOUDNORM}}
      - If input has >1 audio streams (N tracks): expands to
        `[0:a:0]...[0:a:N-1]amerge=inputs=N,loudnorm[aout]`
      - If input has <=1 audio stream: removes `-filter_complex` + token AND
        removes `-map [aout]` so single-track files do not fail.
  * {{AUTO_AMIX}}
      - Same as above, expanding to `[0:a:0]...[0:a:N-1]amix=inputs=N:duration=longest[aout]`.
  * {{MASTER_RAW}} (inside `suffix` or `--suffix`)
      - Expands to `" Master+Raw"` when >1 audio stream exists, or `""` otherwise.

RECOMMENDED AI AGENT WORKFLOW
  Step 1: List profiles
    ffmpeg-queue-manager profiles list --json
    -> Returns array of { name, args, prefix, extension, suffix, file_path }

  Step 2: Probe input media & dry-run command
    ffmpeg-queue-manager probe "C:\videos\clip.mp4" -p "Encode Clip Audio Merge AV1" --json
    -> Returns { "probe": { format_name, duration_secs, video_streams, audio_streams },
                 "preview": { input_path, output_path, audio_track_count, args, command_line } }

  Step 3A: Enqueue into the GUI daemon (non-blocking)
    ffmpeg-queue-manager queue add -p "Encode Clip Audio Merge AV1" --minimized --json "C:\videos\clip.mp4"
    -> Returns array of created Job objects containing `"id": "job-1"`, `"status": "Queued"`

  Step 3B: Enqueue & wait synchronously (with GUI or headless)
    ffmpeg-queue-manager queue add -p "Encode Clip Audio Merge AV1" --wait --json "C:\videos\clip.mp4"
    ffmpeg-queue-manager queue add -p "Encode Clip Audio Merge AV1" --headless --json "C:\videos\clip.mp4"
    -> Waits until completion; returns final Job objects with progress, est_size,
       output_path, input_size_bytes, output_size_bytes, compression_ratio, exit_code.

  Step 3C: Run an ad-hoc FFmpeg encode without a saved profile
    ffmpeg-queue-manager run --raw-args "-c:v libx264 -crf 22 -c:a aac" --ext .mp4 --suffix "_h264" --json "C:\videos\clip.mp4"

  Step 4: Poll status or inspect logs
    ffmpeg-queue-manager queue list --json
    ffmpeg-queue-manager queue inspect job-1 --json
    ffmpeg-queue-manager queue logs job-1 -n 100 --json
    ffmpeg-queue-manager queue wait job-1 --json

QUEUE & PROFILE CONTROL COMMANDS
  * queue pause / queue resume
  * queue stop <job_id | all>
  * queue retry <job_id | failed>
  * queue clear [--completed] [--failed] [--all] [--id <job_id>]
  * profiles create --name "<N>" --args "<ARGS>" [--prefix ...] [--suffix ...] [--ext ...]
  * profiles delete "<N>"
  * settings get --json
  * settings set --max-concurrency 4 --overwrite-mode rename --json
================================================================================
"#;

fn print_err_and_exit(json: bool, msg: &str, code: i32) -> ! {
    if json {
        let obj = serde_json::json!({ "error": msg, "code": code });
        println!("{}", serde_json::to_string_pretty(&obj).unwrap());
    } else {
        eprintln!("Error: {}", msg);
    }
    process::exit(code);
}

fn print_json<T: Serialize>(val: &T) {
    match serde_json::to_string_pretty(val) {
        Ok(s) => println!("{}", s),
        Err(e) => {
            eprintln!("JSON serialization error: {}", e);
            process::exit(1);
        }
    }
}

pub fn run_cli(cli: Cli) -> i32 {
    let json = cli.json;

    // Handle top-level --profile compatibility if no subcommand given
    if cli.command.is_none() {
        if let Some(prof) = cli.profile {
            if cli.files.is_empty() {
                print_err_and_exit(json, "No input files provided with --profile", 2);
            }
            let add_args = QueueAddArgs {
                profile: Some(prof),
                raw_args: None,
                extra_args: None,
                prefix: None,
                suffix: None,
                ext: None,
                output: None,
                priority: CliPriority::Normal,
                recursive: false,
                wait: false,
                headless: false,
                minimized: cli.minimized,
                files: cli.files,
            };
            return cmd_queue_add(add_args, json);
        }
        print_err_and_exit(json, "No command provided. Run with --help or --gui.", 2);
    }

    match cli.command.unwrap() {
        Commands::AgentHelp => {
            println!("{}", AGENT_HELP_MANUAL);
            0
        }
        Commands::Shortcuts => match generate_sendto_shortcuts() {
            Ok(msg) => {
                if json {
                    print_json(&serde_json::json!({ "message": msg }));
                } else {
                    println!("{}", msg);
                }
                0
            }
            Err(e) => print_err_and_exit(json, &e, 1),
        },
        Commands::ListProfiles => cmd_profiles(ProfileCommands::List, json),
        Commands::Enqueue(args) => cmd_queue_add(args, json),
        Commands::Status(args) => cmd_queue_list(args, json),
        Commands::Queue { action } => match action {
            QueueCommands::Add(args) => cmd_queue_add(args, json),
            QueueCommands::List(args) => cmd_queue_list(args, json),
            QueueCommands::Inspect { job_id } => cmd_queue_inspect(&job_id, json),
            QueueCommands::Logs {
                job_id,
                lines,
                follow,
            } => cmd_queue_logs(&job_id, lines, follow, json),
            QueueCommands::Wait { job_ids, all } => cmd_queue_wait(job_ids, all, json),
            QueueCommands::Stop { job_id_or_all } => {
                match try_send(&IpcRequest::StopJob {
                    job_id: job_id_or_all,
                }) {
                    Ok(IpcResponse::Ack { message, affected }) => {
                        if json {
                            print_json(
                                &serde_json::json!({ "message": message, "affected": affected }),
                            );
                        } else {
                            println!("{}", message);
                        }
                        0
                    }
                    Ok(IpcResponse::Error { message }) => print_err_and_exit(json, &message, 1),
                    Ok(_) => print_err_and_exit(json, "Unexpected IPC response", 1),
                    Err(e) => print_err_and_exit(
                        json,
                        &format!("Could not reach running GUI daemon: {}", e),
                        1,
                    ),
                }
            }
            QueueCommands::Retry { job_id_or_failed } => {
                match try_send(&IpcRequest::RetryJob {
                    job_id: job_id_or_failed,
                }) {
                    Ok(IpcResponse::Ack { message, affected }) => {
                        if json {
                            print_json(
                                &serde_json::json!({ "message": message, "affected": affected }),
                            );
                        } else {
                            println!("{}", message);
                        }
                        0
                    }
                    Ok(IpcResponse::Error { message }) => print_err_and_exit(json, &message, 1),
                    Ok(_) => print_err_and_exit(json, "Unexpected IPC response", 1),
                    Err(e) => print_err_and_exit(
                        json,
                        &format!("Could not reach running GUI daemon: {}", e),
                        1,
                    ),
                }
            }
            QueueCommands::Clear {
                completed,
                failed,
                all,
                id,
            } => {
                let eff_completed = if !completed && !failed && !all && id.is_none() {
                    true
                } else {
                    completed
                };
                match try_send(&IpcRequest::ClearJobs {
                    completed: eff_completed,
                    failed,
                    all,
                    job_id: id,
                }) {
                    Ok(IpcResponse::Ack { message, affected }) => {
                        if json {
                            print_json(
                                &serde_json::json!({ "message": message, "affected": affected }),
                            );
                        } else {
                            println!("{}", message);
                        }
                        0
                    }
                    Ok(IpcResponse::Error { message }) => print_err_and_exit(json, &message, 1),
                    Ok(_) => print_err_and_exit(json, "Unexpected IPC response", 1),
                    Err(e) => print_err_and_exit(
                        json,
                        &format!("Could not reach running GUI daemon: {}", e),
                        1,
                    ),
                }
            }
            QueueCommands::Pause => cmd_set_paused(true, json),
            QueueCommands::Resume => cmd_set_paused(false, json),
        },
        Commands::Profiles { action } => cmd_profiles(action, json),
        Commands::Probe(args) => cmd_probe(args, json),
        Commands::Run(args) => cmd_run(args, json),
        Commands::Settings { action } => cmd_settings(action, json),
    }
}

fn cmd_set_paused(paused: bool, json: bool) -> i32 {
    match try_send(&IpcRequest::SetPaused { paused }) {
        Ok(IpcResponse::Ack { message, .. }) => {
            if json {
                print_json(&serde_json::json!({ "paused": paused, "message": message }));
            } else {
                println!("{}", message);
            }
            0
        }
        Ok(IpcResponse::Error { message }) => print_err_and_exit(json, &message, 1),
        Ok(_) => print_err_and_exit(json, "Unexpected IPC response", 1),
        Err(e) => print_err_and_exit(json, &format!("GUI daemon not running: {}", e), 1),
    }
}

fn build_enqueue_specs(
    profile: Option<String>,
    raw_args: Option<String>,
    extra_args: Option<String>,
    prefix: Option<String>,
    suffix: Option<String>,
    ext: Option<String>,
    output: Option<String>,
    priority: JobPriority,
    recursive: bool,
    inputs: &[String],
) -> Result<Vec<EnqueueSpec>, String> {
    if profile.is_none() && raw_args.is_none() {
        return Err("Either --profile <NAME> or --raw-args <ARGS> must be specified".to_string());
    }

    let resolved_profile_name = if let Some(ref pname) = profile {
        let found = get_profile_by_name(pname)?;
        found.profile.name
    } else {
        "Custom CLI Args".to_string()
    };

    let parsed_raw = raw_args.as_deref().map(tokenize_args);
    let parsed_extra = extra_args
        .as_deref()
        .map(tokenize_args)
        .unwrap_or_default();

    let files = collect_media_files(inputs, recursive);
    if files.is_empty() {
        return Err("No matching media files found in the provided paths".to_string());
    }

    if output.is_some() && files.len() > 1 {
        return Err("--output cannot be used when queuing multiple input files".to_string());
    }

    let specs = files
        .into_iter()
        .map(|f| EnqueueSpec {
            file_path: f,
            profile_name: resolved_profile_name.clone(),
            raw_args: parsed_raw.clone(),
            extra_args: parsed_extra.clone(),
            output_path_override: output.clone(),
            prefix_override: prefix.clone(),
            suffix_override: suffix.clone(),
            ext_override: ext.clone(),
            priority,
        })
        .collect();
    Ok(specs)
}

fn cmd_queue_add(args: QueueAddArgs, json: bool) -> i32 {
    let specs = match build_enqueue_specs(
        args.profile,
        args.raw_args,
        args.extra_args,
        args.prefix,
        args.suffix,
        args.ext,
        args.output,
        args.priority.into(),
        args.recursive,
        &args.files,
    ) {
        Ok(s) => s,
        Err(e) => print_err_and_exit(json, &e, 1),
    };

    if args.headless && !is_daemon_running() {
        return run_headless_specs(specs, json);
    }

    let created_jobs = match ensure_daemon_and_send(&IpcRequest::Enqueue { specs }, args.minimized) {
        Ok(IpcResponse::Enqueued { jobs }) => jobs,
        Ok(IpcResponse::Error { message }) => print_err_and_exit(json, &message, 1),
        Ok(_) => print_err_and_exit(json, "Unexpected response from GUI daemon", 1),
        Err(e) => print_err_and_exit(json, &e, 1),
    };

    if !args.wait {
        if json {
            print_json(&created_jobs);
        } else {
            for j in &created_jobs {
                println!(
                    "Queued {} ({}) [profile: {}, priority: {}]",
                    j.id, j.file_path, j.profile_name, j.priority
                );
            }
        }
        return 0;
    }

    let target_ids: Vec<String> = created_jobs.into_iter().map(|j| j.id).collect();
    wait_for_daemon_jobs(&target_ids, false, json)
}

fn cmd_run(args: RunArgs, json: bool) -> i32 {
    let specs = match build_enqueue_specs(
        args.profile,
        args.raw_args,
        args.extra_args,
        args.prefix,
        args.suffix,
        args.ext,
        args.output,
        JobPriority::Normal,
        args.recursive,
        &args.files,
    ) {
        Ok(s) => s,
        Err(e) => print_err_and_exit(json, &e, 1),
    };
    run_headless_specs(specs, json)
}

fn run_headless_specs(specs: Vec<EnqueueSpec>, json: bool) -> i32 {
    let engine = QueueEngine::new(false);
    engine.start_scheduler();
    let jobs = engine.enqueue(specs);
    let ids: Vec<String> = jobs.iter().map(|j| j.id.clone()).collect();

    if !json {
        println!(
            "Running {} job(s) headlessly (max concurrency: {})...",
            ids.len(),
            engine.settings().max_concurrency
        );
    }

    let mut last_pct: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
    loop {
        let snap = engine.snapshot();
        let relevant: Vec<Job> = snap
            .jobs
            .into_iter()
            .filter(|j| ids.contains(&j.id))
            .collect();

        if !json {
            for j in &relevant {
                let pct_i = j.progress.round() as i32;
                let prev = last_pct.get(&j.id).copied().unwrap_or(-1);
                if j.status == JobStatus::Running && (pct_i - prev >= 10 || prev < 0) {
                    last_pct.insert(j.id.clone(), pct_i);
                    eprintln!(
                        "[{}] {}% {} {} {}",
                        j.id,
                        pct_i,
                        j.fps.as_deref().map(|f| format!("{}fps", f)).unwrap_or_default(),
                        j.est_size.as_deref().map(|s| format!("est={}", s)).unwrap_or_default(),
                        j.eta.as_deref().map(|e| format!("eta={}", e)).unwrap_or_default()
                    );
                }
            }
        }

        let all_done = relevant.iter().all(|j| j.status.is_terminal());
        if all_done {
            if json {
                print_json(&relevant);
            } else {
                print_jobs_table(&relevant);
            }
            let any_failed = relevant.iter().any(|j| j.status != JobStatus::Completed);
            return if any_failed { 3 } else { 0 };
        }

        thread::sleep(Duration::from_millis(300));
    }
}

fn wait_for_daemon_jobs(job_ids: &[String], wait_all: bool, json: bool) -> i32 {
    let mut last_pct: std::collections::HashMap<String, i32> = std::collections::HashMap::new();

    loop {
        let snap = match try_send(&IpcRequest::GetQueue) {
            Ok(IpcResponse::Queue { snapshot }) => snapshot,
            Ok(IpcResponse::Error { message }) => print_err_and_exit(json, &message, 1),
            Ok(_) => print_err_and_exit(json, "Unexpected IPC response", 1),
            Err(e) => print_err_and_exit(json, &format!("Lost connection to GUI daemon: {}", e), 1),
        };

        let relevant: Vec<Job> = if wait_all {
            snap.jobs
        } else {
            snap.jobs
                .into_iter()
                .filter(|j| job_ids.contains(&j.id))
                .collect()
        };

        if relevant.is_empty() {
            if json {
                print_json(&Vec::<Job>::new());
            } else {
                println!("No matching jobs in queue.");
            }
            return 0;
        }

        if !json {
            for j in &relevant {
                let pct_i = j.progress.round() as i32;
                let prev = last_pct.get(&j.id).copied().unwrap_or(-1);
                if j.status == JobStatus::Running && (pct_i - prev >= 10 || prev < 0) {
                    last_pct.insert(j.id.clone(), pct_i);
                    eprintln!(
                        "[{}] {}% {} {} {}",
                        j.id,
                        pct_i,
                        j.fps.as_deref().map(|f| format!("{}fps", f)).unwrap_or_default(),
                        j.est_size.as_deref().map(|s| format!("est={}", s)).unwrap_or_default(),
                        j.eta.as_deref().map(|e| format!("eta={}", e)).unwrap_or_default()
                    );
                }
            }
        }

        let all_done = relevant.iter().all(|j| j.status.is_terminal());
        if all_done {
            if json {
                print_json(&relevant);
            } else {
                print_jobs_table(&relevant);
            }
            let any_failed = relevant
                .iter()
                .any(|j| matches!(j.status, JobStatus::Error | JobStatus::Cancelled));
            return if any_failed { 3 } else { 0 };
        }

        thread::sleep(Duration::from_millis(300));
    }
}

fn cmd_queue_wait(job_ids: Vec<String>, all: bool, json: bool) -> i32 {
    if job_ids.is_empty() && !all {
        print_err_and_exit(json, "Pass specific job ID(s) or --all to wait", 2);
    }
    wait_for_daemon_jobs(&job_ids, all, json)
}

fn cmd_queue_list(args: QueueListArgs, json: bool) -> i32 {
    let mut snap = match try_send(&IpcRequest::GetQueue) {
        Ok(IpcResponse::Queue { snapshot }) => snapshot,
        Ok(IpcResponse::Error { message }) => print_err_and_exit(json, &message, 1),
        Ok(_) => print_err_and_exit(json, "Unexpected IPC response", 1),
        Err(_) => {
            if json {
                let empty = crate::models::QueueSnapshot {
                    version: APP_VERSION.to_string(),
                    paused: false,
                    active_runners: 0,
                    max_concurrency: load_settings().max_concurrency,
                    jobs: Vec::new(),
                };
                print_json(&empty);
            } else {
                println!("GUI daemon is not running (0 active jobs).");
            }
            return 0;
        }
    };

    if let Some(filter) = args.status {
        snap.jobs.retain(|j| filter.matches(j.status));
    }

    if json {
        print_json(&snap);
    } else {
        println!(
            "Queue (v{} | paused: {} | active: {}/{})",
            snap.version, snap.paused, snap.active_runners, snap.max_concurrency
        );
        print_jobs_table(&snap.jobs);
    }
    0
}

fn print_jobs_table(jobs: &[Job]) {
    if jobs.is_empty() {
        println!("(no jobs)");
        return;
    }
    for j in jobs {
        let stats = match j.status {
            JobStatus::Completed => j
                .compression_ratio
                .clone()
                .unwrap_or_else(|| "Completed".to_string()),
            JobStatus::Running => format!(
                "{:.0}% | {} fps | {} | est: {} | eta: {}",
                j.progress,
                j.fps.as_deref().unwrap_or("-"),
                j.bitrate.as_deref().unwrap_or("-"),
                j.est_size.as_deref().unwrap_or("-"),
                j.eta.as_deref().unwrap_or("--:--")
            ),
            _ => j.status.as_str().to_string(),
        };
        println!(
            "{:<8} [{:<9}] ({:<6}) {} | profile={} | {}",
            j.id, j.status, j.priority, j.file_path, j.profile_name, stats
        );
    }
}

fn cmd_queue_inspect(job_id: &str, json: bool) -> i32 {
    match try_send(&IpcRequest::GetJob {
        job_id: job_id.to_string(),
    }) {
        Ok(IpcResponse::JobDetail { job }) => {
            if json {
                print_json(&job);
            } else {
                println!("Job ID:        {}", job.id);
                println!("Status:        {} ({:.1}%)", job.status, job.progress);
                println!("Priority:      {}", job.priority);
                println!("Profile:       {}", job.profile_name);
                println!("Input File:    {}", job.file_path);
                println!(
                    "Output File:   {}",
                    job.output_path.as_deref().unwrap_or("(pending)")
                );
                if let Some(in_sz) = job.input_size_bytes {
                    println!("Input Size:    {}", format_bytes(in_sz));
                }
                if let Some(out_sz) = job.output_size_bytes {
                    println!("Output Size:   {}", format_bytes(out_sz));
                }
                if let Some(ref ratio) = job.compression_ratio {
                    println!("Final Size:    {}", ratio);
                }
                if let Some(ref cmd) = job.resolved_command {
                    println!("Command:       {}", cmd);
                }
                println!("Log Lines:     {}", job.logs.len());
            }
            0
        }
        Ok(IpcResponse::Error { message }) => print_err_and_exit(json, &message, 1),
        Ok(_) => print_err_and_exit(json, "Unexpected IPC response", 1),
        Err(e) => print_err_and_exit(json, &format!("Could not reach GUI daemon: {}", e), 1),
    }
}

fn cmd_queue_logs(job_id: &str, lines: Option<usize>, follow: bool, json: bool) -> i32 {
    if !follow {
        match try_send(&IpcRequest::GetJobLogs {
            job_id: job_id.to_string(),
            tail: lines,
        }) {
            Ok(resp @ IpcResponse::JobLogs { .. }) => {
                if json {
                    print_json(&resp);
                } else if let IpcResponse::JobLogs { logs, .. } = resp {
                    for line in logs {
                        println!("{}", line);
                    }
                }
                0
            }
            Ok(IpcResponse::Error { message }) => print_err_and_exit(json, &message, 1),
            Ok(_) => print_err_and_exit(json, "Unexpected IPC response", 1),
            Err(e) => print_err_and_exit(json, &format!("Could not reach GUI daemon: {}", e), 1),
        }
    } else {
        let mut printed_count = 0usize;
        let mut last_line = String::new();
        loop {
            match try_send(&IpcRequest::GetJobLogs {
                job_id: job_id.to_string(),
                tail: if printed_count == 0 { lines } else { None },
            }) {
                Ok(IpcResponse::JobLogs { status, logs, .. }) => {
                    if printed_count == 0 {
                        for l in &logs {
                            println!("{}", l);
                            last_line = l.clone();
                        }
                        printed_count = logs.len();
                    } else if logs.len() > printed_count {
                        for l in &logs[printed_count..] {
                            println!("{}", l);
                            last_line = l.clone();
                        }
                        printed_count = logs.len();
                    } else if let Some(last) = logs.last() {
                        if *last != last_line
                            && (last.starts_with("[FFMPEG] frame=")
                                || last.starts_with("[FFMPEG] size="))
                        {
                            println!("{}", last);
                            last_line = last.clone();
                        }
                    }

                    if !status.is_active() {
                        return if status == JobStatus::Completed { 0 } else { 3 };
                    }
                }
                Ok(IpcResponse::Error { message }) => print_err_and_exit(json, &message, 1),
                Ok(_) => print_err_and_exit(json, "Unexpected IPC response", 1),
                Err(e) => {
                    print_err_and_exit(json, &format!("Lost connection to GUI daemon: {}", e), 1)
                }
            }
            thread::sleep(Duration::from_millis(250));
        }
    }
}

fn cmd_profiles(action: ProfileCommands, json: bool) -> i32 {
    match action {
        ProfileCommands::Path => match get_profiles_dir() {
            Ok(dir) => {
                if json {
                    print_json(&serde_json::json!({ "profiles_dir": dir.to_string_lossy() }));
                } else {
                    println!("{}", dir.display());
                }
                0
            }
            Err(e) => print_err_and_exit(json, &e, 1),
        },
        ProfileCommands::List => match load_profiles() {
            Ok(list) => {
                if json {
                    print_json(&list);
                } else if list.is_empty() {
                    println!("No profiles found in {}.", get_profiles_dir().map(|p| p.display().to_string()).unwrap_or_default());
                } else {
                    for p in list {
                        println!("- {}", p.profile.name);
                        println!("  args:      {}", p.profile.args.join(" "));
                        println!(
                            "  prefix:    {} | suffix: {} | ext: {}",
                            p.profile.prefix.as_deref().unwrap_or("(none)"),
                            p.profile.suffix.as_deref().unwrap_or("(none)"),
                            p.profile.extension.as_deref().unwrap_or("(none)")
                        );
                    }
                }
                0
            }
            Err(e) => print_err_and_exit(json, &e, 1),
        },
        ProfileCommands::Show { name } => match get_profile_by_name(&name) {
            Ok(p) => {
                if json {
                    print_json(&p);
                } else {
                    println!("Name:      {}", p.profile.name);
                    println!("File:      {}", p.file_path);
                    println!("Prefix:    {}", p.profile.prefix.as_deref().unwrap_or("(none)"));
                    println!("Suffix:    {}", p.profile.suffix.as_deref().unwrap_or("(none)"));
                    println!("Extension: {}", p.profile.extension.as_deref().unwrap_or("(none)"));
                    println!("Args:      {}", p.profile.args.join(" "));
                }
                0
            }
            Err(e) => print_err_and_exit(json, &e, 1),
        },
        ProfileCommands::Create {
            name,
            args,
            prefix,
            suffix,
            ext,
        } => {
            let parsed_args = tokenize_args(&args);
            let prof = Profile {
                name: name.clone(),
                args: parsed_args,
                prefix,
                extension: ext,
                suffix,
            };
            match save_profile(&prof) {
                Ok(path) => {
                    let _ = try_send(&IpcRequest::ReloadProfiles);
                    if json {
                        print_json(&serde_json::json!({
                            "saved": true,
                            "name": name,
                            "file_path": path.to_string_lossy()
                        }));
                    } else {
                        println!("Saved profile '{}' to {}", name, path.display());
                    }
                    0
                }
                Err(e) => print_err_and_exit(json, &e, 1),
            }
        }
        ProfileCommands::Delete { name } => match delete_profile(&name) {
            Ok(()) => {
                let _ = try_send(&IpcRequest::ReloadProfiles);
                if json {
                    print_json(&serde_json::json!({ "deleted": true, "name": name }));
                } else {
                    println!("Deleted profile '{}'", name);
                }
                0
            }
            Err(e) => print_err_and_exit(json, &e, 1),
        },
        ProfileCommands::Import { files } => match import_profile_files(&files) {
            Ok(count) => {
                let _ = try_send(&IpcRequest::ReloadProfiles);
                if json {
                    print_json(&serde_json::json!({ "imported": count }));
                } else {
                    println!("Imported {} profile(s).", count);
                }
                0
            }
            Err(e) => print_err_and_exit(json, &e, 1),
        },
    }
}

fn cmd_probe(args: ProbeArgs, json: bool) -> i32 {
    let probe = match probe_media_detailed(&args.file) {
        Ok(p) => p,
        Err(e) => print_err_and_exit(json, &e, 1),
    };

    let preview = if let Some(ref prof_name) = args.profile {
        let prof = match get_profile_by_name(prof_name) {
            Ok(p) => p,
            Err(e) => print_err_and_exit(json, &e, 1),
        };
        let settings = load_settings();
        match resolve_job_plan(
            &args.file,
            Some(&prof.profile),
            None,
            &[],
            None,
            None,
            None,
            None,
            &settings,
            probe.audio_streams.len(),
        ) {
            Ok(plan) => Some(plan),
            Err(e) => print_err_and_exit(json, &e, 1),
        }
    } else {
        None
    };

    let report = ProbeReport { probe, preview };
    if json {
        print_json(&report);
    } else {
        println!("File:     {}", report.probe.file_path);
        println!(
            "Format:   {} | Duration: {} ({:.2}s) | Size: {}",
            report.probe.format_name,
            report.probe.duration_formatted,
            report.probe.duration_secs,
            format_bytes(report.probe.size_bytes)
        );
        for v in &report.probe.video_streams {
            println!(
                "  Video #{}: {} {}x{} @ {} fps | pix_fmt={} | colorspace={}",
                v.index,
                v.codec,
                v.width.unwrap_or(0),
                v.height.unwrap_or(0),
                v.fps.as_deref().unwrap_or("?"),
                v.pix_fmt.as_deref().unwrap_or("?"),
                v.color_space.as_deref().unwrap_or("?")
            );
        }
        for a in &report.probe.audio_streams {
            println!(
                "  Audio #{}: {} {}ch ({}) {}Hz {}",
                a.index,
                a.codec,
                a.channels.unwrap_or(0),
                a.channel_layout.as_deref().unwrap_or("?"),
                a.sample_rate.as_deref().unwrap_or("?"),
                a.language.as_deref().unwrap_or("")
            );
        }
        if let Some(prev) = report.preview {
            println!("Dry-Run Preview:");
            println!("  Audio Tracks: {}", prev.audio_track_count);
            println!("  Output Path:  {}", prev.output_path);
            println!("  Command:      {}", prev.command_line);
        }
    }
    0
}

fn cmd_settings(action: SettingsCommands, json: bool) -> i32 {
    let mut settings = load_settings();
    match action {
        SettingsCommands::Get => {
            let report = SettingsReport {
                settings,
                autostart_enabled: is_autostart_enabled(),
                settings_file: settings_path().to_string_lossy().to_string(),
                profiles_dir: get_profiles_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
            };
            if json {
                print_json(&report);
            } else {
                println!("Settings file:     {}", report.settings_file);
                println!("Profiles dir:      {}", report.profiles_dir);
                println!("Max concurrency:   {}", report.settings.max_concurrency);
                println!("Auto-clear:        {}", report.settings.auto_clear);
                println!("Minimize to tray:  {}", report.settings.minimize_to_tray);
                println!("Dark mode:         {}", report.settings.dark_mode);
                println!("Overwrite mode:    {}", report.settings.overwrite_mode.as_str());
                println!(
                    "Custom output dir: {}",
                    report
                        .settings
                        .custom_output_dir
                        .as_deref()
                        .unwrap_or("(alongside input file)")
                );
                println!("Run on Startup:    {}", report.autostart_enabled);
            }
            0
        }
        SettingsCommands::Set {
            max_concurrency,
            auto_clear,
            autostart,
            minimize_to_tray,
            dark_mode,
            overwrite_mode,
            output_dir,
        } => {
            if let Some(c) = max_concurrency {
                settings.max_concurrency = c.clamp(1, 16);
            }
            if let Some(ac) = auto_clear {
                settings.auto_clear = ac;
            }
            if let Some(m) = minimize_to_tray {
                settings.minimize_to_tray = m;
            }
            if let Some(d) = dark_mode {
                settings.dark_mode = d;
            }
            if let Some(om) = overwrite_mode {
                settings.overwrite_mode = om.into();
            }
            if let Some(od) = output_dir {
                if od.trim().is_empty() || od.eq_ignore_ascii_case("none") {
                    settings.custom_output_dir = None;
                } else {
                    settings.custom_output_dir = Some(od);
                }
            }
            if let Err(e) = save_settings(&settings) {
                print_err_and_exit(json, &e, 1);
            }
            if let Some(as_flag) = autostart {
                if let Err(e) = set_autostart_enabled(as_flag) {
                    print_err_and_exit(json, &e, 1);
                }
            }
            let _ = try_send(&IpcRequest::ReloadSettings);

            let report = SettingsReport {
                settings,
                autostart_enabled: is_autostart_enabled(),
                settings_file: settings_path().to_string_lossy().to_string(),
                profiles_dir: get_profiles_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
            };
            if json {
                print_json(&report);
            } else {
                println!("Settings updated.");
            }
            0
        }
    }
}
