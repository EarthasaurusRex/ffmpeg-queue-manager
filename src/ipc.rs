use crate::engine::QueueEngine;
use crate::models::{DaemonInfo, EnqueueSpec, Job, JobStatus, QueueSnapshot, APP_VERSION};
use crate::paths::{load_daemon_info, save_daemon_info};
use crate::probe::CREATE_NO_WINDOW;
use serde::{Deserialize, Serialize};
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub const DEFAULT_IPC_PORT: u16 = 45892;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IpcRequest {
    Ping,
    ShowWindow,
    Enqueue {
        specs: Vec<EnqueueSpec>,
    },
    GetQueue,
    GetJob {
        job_id: String,
    },
    GetJobLogs {
        job_id: String,
        tail: Option<usize>,
    },
    StopJob {
        job_id: String,
    },
    RetryJob {
        job_id: String,
    },
    ClearJobs {
        completed: bool,
        failed: bool,
        all: bool,
        job_id: Option<String>,
    },
    SetPaused {
        paused: bool,
    },
    ReloadProfiles,
    ReloadSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    Pong {
        version: String,
        pid: u32,
        total_jobs: usize,
        running: usize,
        queued: usize,
        paused: bool,
    },
    Ack {
        message: String,
        affected: usize,
    },
    Enqueued {
        jobs: Vec<Job>,
    },
    Queue {
        snapshot: QueueSnapshot,
    },
    JobDetail {
        job: Job,
    },
    JobLogs {
        job_id: String,
        status: JobStatus,
        logs: Vec<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Clone)]
pub struct GuiWakeSignals {
    pub show_window: Arc<AtomicBool>,
    pub reload_profiles: Arc<AtomicBool>,
    pub hwnd: Arc<AtomicIsize>,
}

impl GuiWakeSignals {
    pub fn new() -> Self {
        Self {
            show_window: Arc::new(AtomicBool::new(false)),
            reload_profiles: Arc::new(AtomicBool::new(false)),
            hwnd: Arc::new(AtomicIsize::new(0)),
        }
    }

    pub fn wake_window(&self) {
        self.show_window.store(true, Ordering::SeqCst);
        #[cfg(windows)]
        {
            let h = self.hwnd.load(Ordering::SeqCst);
            if h != 0 {
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
                    };
                    ShowWindow(h as _, SW_SHOW);
                    ShowWindow(h as _, SW_RESTORE);
                    SetForegroundWindow(h as _);
                }
            }
        }
    }
}

pub fn start_ipc_server(
    engine: QueueEngine,
    signals: GuiWakeSignals,
    repaint_ctx: Option<egui::Context>,
) -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", DEFAULT_IPC_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .map_err(|e| format!("Failed to bind IPC listener: {}", e))?;

    let port = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();

    save_daemon_info(&DaemonInfo {
        port,
        pid: std::process::id(),
    });

    thread::spawn(move || {
        for stream_res in listener.incoming() {
            if let Ok(stream) = stream_res {
                let eng = engine.clone();
                let sig = signals.clone();
                let ctx = repaint_ctx.clone();
                thread::spawn(move || {
                    handle_client(stream, &eng, &sig, ctx.as_ref());
                });
            }
        }
    });

    Ok(port)
}

fn handle_client(
    mut stream: TcpStream,
    engine: &QueueEngine,
    signals: &GuiWakeSignals,
    repaint_ctx: Option<&egui::Context>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut line = String::new();
    {
        let mut reader = BufReader::new(&stream);
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            return;
        }
    }

    let response = match serde_json::from_str::<IpcRequest>(line.trim()) {
        Ok(req) => process_request(req, engine, signals, repaint_ctx),
        Err(e) => IpcResponse::Error {
            message: format!("Invalid IPC JSON request: {}", e),
        },
    };

    if let Ok(mut json) = serde_json::to_string(&response) {
        json.push('\n');
        let _ = stream.write_all(json.as_bytes());
        let _ = stream.flush();
    }
}

fn process_request(
    req: IpcRequest,
    engine: &QueueEngine,
    signals: &GuiWakeSignals,
    repaint_ctx: Option<&egui::Context>,
) -> IpcResponse {
    match req {
        IpcRequest::Ping => {
            let snap = engine.snapshot();
            let running = snap
                .jobs
                .iter()
                .filter(|j| j.status == JobStatus::Running)
                .count();
            let queued = snap
                .jobs
                .iter()
                .filter(|j| j.status == JobStatus::Queued)
                .count();
            IpcResponse::Pong {
                version: APP_VERSION.to_string(),
                pid: std::process::id(),
                total_jobs: snap.jobs.len(),
                running,
                queued,
                paused: snap.paused,
            }
        }
        IpcRequest::ShowWindow => {
            signals.wake_window();
            if let Some(ctx) = repaint_ctx {
                ctx.request_repaint();
            }
            IpcResponse::Ack {
                message: "Window shown".to_string(),
                affected: 1,
            }
        }
        IpcRequest::Enqueue { specs } => {
            let jobs = engine.enqueue(specs);
            if let Some(ctx) = repaint_ctx {
                ctx.request_repaint();
            }
            IpcResponse::Enqueued { jobs }
        }
        IpcRequest::GetQueue => IpcResponse::Queue {
            snapshot: engine.snapshot(),
        },
        IpcRequest::GetJob { job_id } => match engine.get_job(&job_id) {
            Some(job) => IpcResponse::JobDetail { job },
            None => IpcResponse::Error {
                message: format!("Job '{}' not found", job_id),
            },
        },
        IpcRequest::GetJobLogs { job_id, tail } => match engine.get_job(&job_id) {
            Some(job) => {
                let logs = match tail {
                    Some(n) if n < job.logs.len() => job.logs[job.logs.len() - n..].to_vec(),
                    _ => job.logs,
                };
                IpcResponse::JobLogs {
                    job_id,
                    status: job.status,
                    logs,
                }
            }
            None => IpcResponse::Error {
                message: format!("Job '{}' not found", job_id),
            },
        },
        IpcRequest::StopJob { job_id } => {
            let affected = engine.stop_job(&job_id);
            IpcResponse::Ack {
                message: format!("Stopped {} job(s)", affected),
                affected,
            }
        }
        IpcRequest::RetryJob { job_id } => {
            let affected = engine.retry_job(&job_id);
            IpcResponse::Ack {
                message: format!("Re-queued {} job(s)", affected),
                affected,
            }
        }
        IpcRequest::ClearJobs {
            completed,
            failed,
            all,
            job_id,
        } => {
            let affected = engine.clear_jobs(completed, failed, all, job_id.as_deref());
            IpcResponse::Ack {
                message: format!("Cleared {} job(s)", affected),
                affected,
            }
        }
        IpcRequest::SetPaused { paused } => {
            engine.set_paused(paused);
            IpcResponse::Ack {
                message: if paused {
                    "Queue paused".to_string()
                } else {
                    "Queue resumed".to_string()
                },
                affected: 1,
            }
        }
        IpcRequest::ReloadProfiles => {
            signals.reload_profiles.store(true, Ordering::SeqCst);
            if let Some(ctx) = repaint_ctx {
                ctx.request_repaint();
            }
            IpcResponse::Ack {
                message: "Profiles reload signaled".to_string(),
                affected: 1,
            }
        }
        IpcRequest::ReloadSettings => {
            engine.reload_settings_from_disk();
            if let Some(ctx) = repaint_ctx {
                ctx.request_repaint();
            }
            IpcResponse::Ack {
                message: "Settings reloaded".to_string(),
                affected: 1,
            }
        }
    }
}

fn candidate_ports() -> Vec<u16> {
    let mut ports = Vec::new();
    if let Some(info) = load_daemon_info() {
        ports.push(info.port);
    }
    if !ports.contains(&DEFAULT_IPC_PORT) {
        ports.push(DEFAULT_IPC_PORT);
    }
    ports
}

pub fn try_send(req: &IpcRequest) -> Result<IpcResponse, String> {
    let mut last_err = "Daemon not reachable".to_string();
    for port in candidate_ports() {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        match TcpStream::connect_timeout(&addr, Duration::from_millis(250)) {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

                let mut payload = serde_json::to_string(req).map_err(|e| e.to_string())?;
                payload.push('\n');
                if let Err(e) = stream.write_all(payload.as_bytes()) {
                    last_err = e.to_string();
                    continue;
                }
                let _ = stream.flush();

                let mut resp_line = String::new();
                let mut reader = BufReader::new(stream);
                match reader.read_line(&mut resp_line) {
                    Ok(n) if n > 0 => {
                        return serde_json::from_str::<IpcResponse>(resp_line.trim())
                            .map_err(|e| format!("Invalid IPC response: {}", e));
                    }
                    Ok(_) => last_err = "Empty response from daemon".to_string(),
                    Err(e) => last_err = e.to_string(),
                }
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(last_err)
}

pub fn is_daemon_running() -> bool {
    matches!(try_send(&IpcRequest::Ping), Ok(IpcResponse::Pong { .. }))
}

pub fn spawn_gui_daemon(minimized: bool) -> Result<(), String> {
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    if minimized {
        cmd.arg("--minimized");
    } else {
        cmd.arg("--gui");
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd.spawn()
        .map_err(|e| format!("Failed to start GUI daemon: {}", e))?;
    Ok(())
}

pub fn ensure_daemon_and_send(
    req: &IpcRequest,
    start_minimized: bool,
) -> Result<IpcResponse, String> {
    if is_daemon_running() {
        return try_send(req);
    }

    spawn_gui_daemon(start_minimized)?;

    for _ in 0..40 {
        thread::sleep(Duration::from_millis(100));
        if is_daemon_running() {
            return try_send(req);
        }
    }

    Err("Timed out waiting for FFmpeg Queue Manager GUI daemon to start".to_string())
}
