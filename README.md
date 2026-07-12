# FFmpeg Queue Manager

A native Windows desktop application built with Tauri, React, and Fluent UI for managing and executing batch FFmpeg encoding jobs.

## Architecture

- **Frontend**: React 19, Fluent UI 9, Vite
- **Backend**: Rust (Tauri v2)
- **Sidecars**: Bundled statically with `ffmpeg-x86_64-pc-windows-msvc.exe` and `ffprobe-x86_64-pc-windows-msvc.exe`. Tracked via Git LFS.

## Features

- **Background Queue Processing**: Manages active FFmpeg instances (configurable concurrency) completely in the background via the Windows System Tray.
- **Dynamic Audio Parsing**: Utilizes `ffprobe` to parse audio channels from input files before encoding, automatically generating and injecting `amerge` filters if multi-track audio is detected.
- **Profile System**: Allows creation of JSON-based FFmpeg profiles with configurable arguments, prefixes, suffixes, and extensions.
- **Telemetry Hook**: Hooks into FFmpeg `stderr` pipes using Regex to track frame output, calculating live FPS, Bitrate, and ETA metrics.
- **Windows Integration**: Hooks into the Windows Registry for Run on Startup, and generates contextual "Send To" shortcuts mapped directly to saved encoding profiles.
- **In-App Updater**: Checks GitHub releases and seamlessly installs new updates.

## Development

### Prerequisites
- [Node.js](https://nodejs.org/) (v20+)
- [Rust](https://www.rust-lang.org/)

### Setup

```bash
git clone https://github.com/EarthasaurusRex/ffmpeg-queue-manager.git
cd ffmpeg-queue-manager
npm install
npm run tauri dev
```

### Production Build

```bash
npm run tauri build
```

## CI/CD

The `.github/workflows/release.yml` pipeline automatically builds and publishes `.exe` installers to GitHub Releases whenever a new `v*` tag is pushed. The application uses these releases to deliver Over-The-Air (OTA) updates via the Tauri updater plugin.

## License
MIT License
