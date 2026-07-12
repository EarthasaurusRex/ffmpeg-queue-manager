# 🎬 FFmpeg Queue Manager

A lightning-fast, native Windows application built with [Tauri](https://tauri.app/), [React](https://reactjs.org/), and [Fluent UI](https://react.fluentui.dev/). It serves as an ultra-optimized graphical interface and background queue manager for executing massive batches of FFmpeg encoding jobs.

## ✨ Features

- **Native Windows Integration**: Generates context menu shortcuts so you can simply right-click any video, select **Send To**, and beam it directly into the queue manager.
- **Silent Background Execution**: Need to encode 50 videos overnight? The app elegantly intercepts close requests and minimizes into the Windows System Tray, churning through your queue in the background completely silently.
- **Dynamic Audio Mixing**: Employs an intelligent `ffprobe` sidecar parser that detects audio tracks before encoding. It dynamically builds and injects `amerge` filters for multi-track inputs, preventing encoding crashes.
- **Profile Management Engine**: Create, edit, and import modular JSON profiles for your FFmpeg arguments (e.g., `-c:v libx264 -crf 23`). 
- **Real-Time Telemetry**: Automatically hooks into FFmpeg's `stderr` output stream to provide a highly performant, auto-scrolling terminal, live **FPS** trackers, **Bitrate** metrics, and dynamically calculated **ETAs**.
- **Run on Startup**: Native Windows registry integration allows the queue manager to boot silently when you log into your PC.

## 🚀 Getting Started

### Prerequisites
- [Node.js](https://nodejs.org/)
- [Rust](https://www.rust-lang.org/)

### Installation

1. Clone the repository:
```bash
git clone https://github.com/EarthasaurusRex/ffmpeg-queue-manager.git
cd ffmpeg-queue-manager
```

2. Install dependencies:
```bash
npm install
```

3. Run the development server:
```bash
npm run tauri dev
```

### Production Build

To compile a hyper-optimized `.exe` and generate the Windows NSIS Installer Setup, run:
```bash
npm run tauri build
```
The installers will be output to `src-tauri/target/release/bundle/`.

## 🛠️ Architecture

- **Frontend**: React 19, Fluent UI 9, Vite.
- **Backend**: Rust (Tauri v2), leveraging native Windows APIs for filesystem operations, single-instance lock protocols, and system tray management.
- **Sidecars**: Bundled statically with `ffmpeg-x86_64-pc-windows-msvc.exe` and `ffprobe-x86_64-pc-windows-msvc.exe`. Tracked via Git LFS.

## 📝 License
MIT License
