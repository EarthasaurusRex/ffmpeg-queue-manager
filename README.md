# FFmpeg Queue Manager

A native Windows desktop and CLI application written in pure Rust (`eframe`/`egui` + `clap`) for managing concurrent FFmpeg encoding queues, reusable JSON profiles, and automated media workflows.

## Architecture

- **GUI**: Native immediate-mode Rust interface using `eframe` / `egui` with Windows System Tray (`tray-icon`) and drag-and-drop support.
- **CLI & AI Automation**: Built with `clap` v4 inside the same binary, communicating with a running GUI instance over localhost TCP IPC (`127.0.0.1:45892`) or executing headlessly in-process.
- **Profile & Queue Storage**:
  - Profiles: `Profiles/` adjacent to the executable or `%LOCALAPPDATA%\FFmpeg Queue Manager\Profiles` (`FFMPEG_QUEUE_PROFILES_DIR` overrides both).
  - Settings & Queue Snapshot: `%LOCALAPPDATA%\FFmpeg Queue Manager\`.
- **Media Binaries**: Looks for adjacent `ffmpeg.exe` / `ffprobe.exe` or bundled Git LFS executables in `src-tauri/bin/`.

## Features

- **Concurrent Queue Engine**: Configurable worker concurrency (`1..=16`), job priorities (`High`, `Normal`, `Low`), pause/resume, per-job and batch stop/retry/clear controls.
- **Dynamic Audio Track Parsing (`ffprobe`)**: Detects input audio stream counts prior to encoding. Multi-track inputs automatically expand `{{AUTO_AMERGE_LOUDNORM}}`, `{{AUTO_AMIX}}`, and `{{MASTER_RAW}}`; single-track inputs strip multi-track filter flags cleanly.
- **Real-Time Telemetry**: Parses FFmpeg `stderr` chunks to display live progress percentage, FPS, speed multiplier, bitrate, ETA, estimated output size, and final post-encode size percentage (`% of original`).
- **Triple-Dot Context Menu**: Show input/output in Explorer, open completed outputs in default player, copy resolved FFmpeg command/logs, prioritize, or retry.
- **Windows Integration**: Registry Run-on-Startup toggle, Minimize to System Tray, and Windows Explorer "Send To" `.lnk` shortcut generation.
- **Media Inspector**: Built-in `ffprobe` stream inspector and dry-run command previewer.

## CLI & AI Agent Usage

All CLI commands accept `--json` (`-j`) for structured output and follow deterministic exit codes (`0` success, `1` runtime error, `2` usage error, `3` encoding failure).

```bash
# Read full AI Agent reference manual (schemas, tokens, exit codes)
ffmpeg-queue-manager agent-help

# Discover saved profiles
ffmpeg-queue-manager profiles list --json

# Inspect media metadata and dry-run a profile command
ffmpeg-queue-manager probe "video.mp4" --profile "Encode Clip Audio Merge AV1" --json

# Queue files into the GUI daemon (auto-starts minimized if needed) and wait for completion
ffmpeg-queue-manager queue add -p "Encode Clip Audio Merge AV1" -w --minimized --json "video.mp4"

# Run custom FFmpeg arguments headlessly without saving a profile or starting GUI
ffmpeg-queue-manager run --raw-args "-c:v libx264 -crf 23 -c:a aac" --ext .mp4 --json "video.mp4"

# Monitor and control active queue jobs
ffmpeg-queue-manager queue list --json
ffmpeg-queue-manager queue logs job-1 -n 50 --json
ffmpeg-queue-manager queue stop job-1
ffmpeg-queue-manager queue retry failed
```

Legacy Windows SendTo compatibility (`--profile <NAME> <FILES...>`) launches silently without creating a console window.

## Development & Build

### Prerequisites
- [Rust](https://www.rust-lang.org/) toolchain (`x86_64-pc-windows-msvc`)
- Git LFS (for bundled `ffmpeg` and `ffprobe` binaries in `src-tauri/bin/`)

### Build & Run

```bash
cargo build --release
```

The compiled executable is produced at `target/release/ffmpeg-queue-manager.exe`.

## License

MIT License
