use crate::models::{
    AppSettings, EnqueueSpec, Job, JobPriority, JobStatus, Profile, QueueSnapshot, APP_VERSION,
};
use crate::paths::{
    format_bytes, format_duration, load_settings, now_unix_secs, resolve_binary, save_queue_snapshot,
    save_settings, load_queue_snapshot,
};
use crate::probe::{probe_audio_track_count, CREATE_NO_WINDOW};
use crate::profiles::{get_profile_by_name, resolve_job_plan};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

struct ActiveProcess {
    pid: u32,
    cancelled: Arc<AtomicBool>,
}

struct EngineInner {
    jobs: Vec<Job>,
    next_job_seq: u64,
    settings: AppSettings,
    paused: bool,
    active_runners: usize,
    active_processes: HashMap<String, ActiveProcess>,
    persist: bool,
    repaint_ctx: Option<egui::Context>,
}

#[derive(Clone)]
pub struct QueueEngine {
    inner: Arc<Mutex<EngineInner>>,
}

impl QueueEngine {
    pub fn new(persist: bool) -> Self {
        let settings = load_settings();
        let mut jobs = Vec::new();
        let mut max_seq = 0u64;

        if persist {
            if let Some(snapshot) = load_queue_snapshot() {
                for mut job in snapshot.jobs {
                    if let Some(num) = job.id.strip_prefix("job-").and_then(|s| s.parse::<u64>().ok()) {
                        if num > max_seq {
                            max_seq = num;
                        }
                    }
                    if job.status == JobStatus::Running {
                        job.status = JobStatus::Cancelled;
                        job.logs
                            .push("[SYSTEM] Previous session terminated while job was running.".to_string());
                    }
                    jobs.push(job);
                }
            }
        }

        Self {
            inner: Arc::new(Mutex::new(EngineInner {
                jobs,
                next_job_seq: max_seq + 1,
                settings,
                paused: false,
                active_runners: 0,
                active_processes: HashMap::new(),
                persist,
                repaint_ctx: None,
            })),
        }
    }

    pub fn set_repaint_ctx(&self, ctx: egui::Context) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.repaint_ctx = Some(ctx);
        }
    }

    fn notify_ui(&self) {
        if let Ok(guard) = self.inner.lock() {
            if let Some(ctx) = &guard.repaint_ctx {
                ctx.request_repaint();
            }
        }
    }

    fn persist_locked(guard: &EngineInner) {
        if !guard.persist {
            return;
        }
        let snapshot = QueueSnapshot {
            version: APP_VERSION.to_string(),
            paused: guard.paused,
            active_runners: guard.active_runners,
            max_concurrency: guard.settings.max_concurrency,
            jobs: guard.jobs.clone(),
        };
        let _ = save_queue_snapshot(&snapshot);
    }

    pub fn start_scheduler(&self) {
        let engine = self.clone();
        thread::spawn(move || loop {
            engine.schedule_step();
            thread::sleep(Duration::from_millis(150));
        });
    }

    pub fn schedule_step(&self) {
        let maybe_next_id = {
            let mut guard = self.inner.lock().unwrap();
            if guard.paused || guard.active_runners >= guard.settings.max_concurrency.max(1) {
                None
            } else {
                // Pick highest-priority Queued job in FIFO order
                let mut best_idx: Option<usize> = None;
                let mut best_rank: u8 = 0;
                for (i, job) in guard.jobs.iter().enumerate() {
                    if job.status == JobStatus::Queued {
                        let r = job.priority.rank();
                        if best_idx.is_none() || r > best_rank {
                            best_idx = Some(i);
                            best_rank = r;
                        }
                    }
                }
                if let Some(idx) = best_idx {
                    guard.jobs[idx].status = JobStatus::Running;
                    guard.jobs[idx].started_at = Some(now_unix_secs());
                    guard.active_runners += 1;
                    let id = guard.jobs[idx].id.clone();
                    Self::persist_locked(&guard);
                    Some(id)
                } else {
                    None
                }
            }
        };

        if let Some(job_id) = maybe_next_id {
            self.notify_ui();
            let engine = self.clone();
            thread::spawn(move || {
                engine.execute_job(&job_id);
            });
        }
    }

    pub fn enqueue(&self, specs: Vec<EnqueueSpec>) -> Vec<Job> {
        let mut created = Vec::with_capacity(specs.len());
        {
            let mut guard = self.inner.lock().unwrap();
            for spec in specs {
                let id = format!("job-{}", guard.next_job_seq);
                guard.next_job_seq += 1;

                let input_size = fs::metadata(&spec.file_path).ok().map(|m| m.len());
                let job = Job {
                    id,
                    file_path: spec.file_path,
                    output_path: spec.output_path_override,
                    profile_name: spec.profile_name,
                    raw_args: spec.raw_args,
                    extra_args: spec.extra_args,
                    prefix_override: spec.prefix_override,
                    suffix_override: spec.suffix_override,
                    ext_override: spec.ext_override,
                    priority: spec.priority,
                    status: JobStatus::Queued,
                    progress: 0.0,
                    logs: Vec::new(),
                    expanded_terminal: false,
                    fps: None,
                    bitrate: None,
                    speed: None,
                    eta: None,
                    est_size: None,
                    compression_ratio: None,
                    input_size_bytes: input_size,
                    output_size_bytes: None,
                    resolved_command: None,
                    started_at: None,
                    finished_at: None,
                    exit_code: None,
                };
                guard.jobs.push(job.clone());
                created.push(job);
            }
            Self::persist_locked(&guard);
        }
        self.notify_ui();
        self.schedule_step();
        created
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        let guard = self.inner.lock().unwrap();
        QueueSnapshot {
            version: APP_VERSION.to_string(),
            paused: guard.paused,
            active_runners: guard.active_runners,
            max_concurrency: guard.settings.max_concurrency,
            jobs: guard.jobs.clone(),
        }
    }

    pub fn get_job(&self, job_id: &str) -> Option<Job> {
        let guard = self.inner.lock().unwrap();
        guard.jobs.iter().find(|j| j.id == job_id).cloned()
    }

    pub fn settings(&self) -> AppSettings {
        self.inner.lock().unwrap().settings.clone()
    }

    pub fn update_settings(&self, new_settings: AppSettings) -> Result<(), String> {
        save_settings(&new_settings)?;
        {
            let mut guard = self.inner.lock().unwrap();
            guard.settings = new_settings;
        }
        self.notify_ui();
        Ok(())
    }

    pub fn reload_settings_from_disk(&self) {
        let loaded = load_settings();
        if let Ok(mut guard) = self.inner.lock() {
            guard.settings = loaded;
        }
        self.notify_ui();
    }

    pub fn set_paused(&self, paused: bool) {
        {
            let mut guard = self.inner.lock().unwrap();
            guard.paused = paused;
            Self::persist_locked(&guard);
        }
        self.notify_ui();
        if !paused {
            self.schedule_step();
        }
    }

    pub fn toggle_terminal(&self, job_id: &str) {
        {
            let mut guard = self.inner.lock().unwrap();
            if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
                job.expanded_terminal = !job.expanded_terminal;
            }
        }
        self.notify_ui();
    }

    pub fn move_to_top(&self, job_id: &str) -> bool {
        let changed = {
            let mut guard = self.inner.lock().unwrap();
            if let Some(pos) = guard.jobs.iter().position(|j| j.id == job_id && j.status == JobStatus::Queued) {
                let mut job = guard.jobs.remove(pos);
                job.priority = JobPriority::High;
                let insert_pos = guard
                    .jobs
                    .iter()
                    .position(|j| j.status == JobStatus::Queued)
                    .unwrap_or(guard.jobs.len());
                guard.jobs.insert(insert_pos, job);
                Self::persist_locked(&guard);
                true
            } else {
                false
            }
        };
        if changed {
            self.notify_ui();
        }
        changed
    }

    pub fn stop_job(&self, target: &str) -> usize {
        let mut to_kill = Vec::new();
        let mut stopped_count = 0usize;

        {
            let mut guard = self.inner.lock().unwrap();
            let EngineInner {
                jobs,
                active_processes,
                ..
            } = &mut *guard;
            let match_all = target.eq_ignore_ascii_case("all");
            for job in jobs.iter_mut() {
                if match_all || job.id == target {
                    if job.status == JobStatus::Running {
                        if let Some(proc_info) = active_processes.get(&job.id) {
                            proc_info.cancelled.store(true, Ordering::SeqCst);
                            to_kill.push(proc_info.pid);
                        }
                        job.status = JobStatus::Cancelled;
                        stopped_count += 1;
                    } else if job.status == JobStatus::Queued || job.status == JobStatus::Paused {
                        job.status = JobStatus::Cancelled;
                        job.finished_at = Some(now_unix_secs());
                        job.logs.push("[SYSTEM] Job cancelled before start.".to_string());
                        stopped_count += 1;
                    }
                }
            }
            Self::persist_locked(&guard);
        }

        for pid in to_kill {
            kill_process_tree(pid);
        }

        self.notify_ui();
        stopped_count
    }

    pub fn retry_job(&self, target: &str) -> usize {
        let retried = {
            let mut guard = self.inner.lock().unwrap();
            let match_failed = target.eq_ignore_ascii_case("failed");
            let mut count = 0usize;
            for job in guard.jobs.iter_mut() {
                let matches = if match_failed {
                    matches!(job.status, JobStatus::Error | JobStatus::Cancelled)
                } else {
                    job.id == target && job.status.is_terminal()
                };
                if matches {
                    job.status = JobStatus::Queued;
                    job.progress = 0.0;
                    job.fps = None;
                    job.bitrate = None;
                    job.speed = None;
                    job.eta = None;
                    job.est_size = None;
                    job.compression_ratio = None;
                    job.output_size_bytes = None;
                    job.started_at = None;
                    job.finished_at = None;
                    job.exit_code = None;
                    job.logs.push("[SYSTEM] Job re-queued for retry.".to_string());
                    count += 1;
                }
            }
            Self::persist_locked(&guard);
            count
        };
        if retried > 0 {
            self.notify_ui();
            self.schedule_step();
        }
        retried
    }

    pub fn clear_jobs(
        &self,
        completed: bool,
        failed: bool,
        all: bool,
        job_id: Option<&str>,
    ) -> usize {
        let removed = {
            let mut guard = self.inner.lock().unwrap();
            let before = guard.jobs.len();
            guard.jobs.retain(|j| {
                if j.status == JobStatus::Running {
                    return true;
                }
                if let Some(id) = job_id {
                    return j.id != id;
                }
                if all {
                    return false;
                }
                if completed && j.status == JobStatus::Completed {
                    return false;
                }
                if failed && matches!(j.status, JobStatus::Error | JobStatus::Cancelled) {
                    return false;
                }
                true
            });
            let count = before.saturating_sub(guard.jobs.len());
            Self::persist_locked(&guard);
            count
        };
        if removed > 0 {
            self.notify_ui();
        }
        removed
    }

    pub fn stop_and_clear_all(&self) {
        self.stop_job("all");
        // Wait briefly for processes to terminate cleanly
        thread::sleep(Duration::from_millis(120));
        {
            let mut guard = self.inner.lock().unwrap();
            guard.jobs.retain(|j| j.status == JobStatus::Running);
            Self::persist_locked(&guard);
        }
        self.notify_ui();
    }

    pub fn append_log(&self, job_id: &str, line: String) {
        {
            let mut guard = self.inner.lock().unwrap();
            if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
                let is_progress_line =
                    line.starts_with("[FFMPEG] frame=") || line.starts_with("[FFMPEG] size=");
                if is_progress_line {
                    if let Some(last) = job.logs.last_mut() {
                        if last.starts_with("[FFMPEG] frame=") || last.starts_with("[FFMPEG] size=")
                        {
                            *last = line;
                            drop(guard);
                            self.notify_ui();
                            return;
                        }
                    }
                }
                job.logs.push(line);
                if job.logs.len() > 1000 {
                    let excess = job.logs.len() - 1000;
                    job.logs.drain(0..excess);
                }
            }
        }
        self.notify_ui();
    }

    fn execute_job(&self, job_id: &str) {
        let job_opt = self.get_job(job_id);
        let Some(job) = job_opt else {
            let mut guard = self.inner.lock().unwrap();
            guard.active_runners = guard.active_runners.saturating_sub(1);
            return;
        };

        let result = self.run_job_inner(&job);
        let auto_clear = self.settings().auto_clear;

        let was_completed = match result {
            Ok((code, cancelled, in_bytes, out_bytes, ratio)) => {
                self.append_log(job_id, format!("[SYSTEM] Job finished with exit code: {}", code));
                let final_status = if cancelled {
                    JobStatus::Cancelled
                } else if code == 0 {
                    JobStatus::Completed
                } else {
                    JobStatus::Error
                };

                {
                    let mut guard = self.inner.lock().unwrap();
                    guard.active_processes.remove(job_id);
                    guard.active_runners = guard.active_runners.saturating_sub(1);
                    if let Some(j) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
                        j.status = final_status;
                        j.exit_code = Some(code);
                        j.finished_at = Some(now_unix_secs());
                        j.input_size_bytes = in_bytes.or(j.input_size_bytes);
                        j.output_size_bytes = out_bytes;
                        if let Some(r) = ratio {
                            j.compression_ratio = Some(r);
                        }
                        if final_status == JobStatus::Completed {
                            j.progress = 100.0;
                            j.eta = Some("00:00:00".to_string());
                        }
                    }
                    Self::persist_locked(&guard);
                }
                final_status == JobStatus::Completed
            }
            Err(err_msg) => {
                self.append_log(job_id, format!("[SYSTEM ERROR] {}", err_msg));
                {
                    let mut guard = self.inner.lock().unwrap();
                    guard.active_processes.remove(job_id);
                    guard.active_runners = guard.active_runners.saturating_sub(1);
                    if let Some(j) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
                        if j.status != JobStatus::Cancelled {
                            j.status = JobStatus::Error;
                        }
                        j.finished_at = Some(now_unix_secs());
                    }
                    Self::persist_locked(&guard);
                }
                false
            }
        };

        self.notify_ui();
        self.schedule_step();

        if was_completed && auto_clear && self.inner.lock().map(|g| g.persist).unwrap_or(false) {
            let engine = self.clone();
            let id_copy = job_id.to_string();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(5));
                engine.clear_jobs(true, false, false, Some(&id_copy));
            });
        }
    }

    fn run_job_inner(
        &self,
        job: &Job,
    ) -> Result<(i32, bool, Option<u64>, Option<u64>, Option<String>), String> {
        let job_id = &job.id;
        self.append_log(job_id, format!("[SYSTEM] Started job for {}", job.file_path));

        if !Path::new(&job.file_path).exists() {
            return Err(format!("Input file does not exist: {}", job.file_path));
        }

        let settings = self.settings();
        let profile: Option<Profile> = if job.raw_args.is_some() {
            None
        } else {
            let found = get_profile_by_name(&job.profile_name)?;
            self.append_log(
                job_id,
                format!("[SYSTEM] Loaded profile '{}'", found.profile.name),
            );
            Some(found.profile)
        };

        self.append_log(
            job_id,
            "[SYSTEM] Running ffprobe to detect audio tracks...".to_string(),
        );
        let (acount, probe_lines) = probe_audio_track_count(&job.file_path)?;
        for pl in probe_lines {
            self.append_log(job_id, format!("[FFPROBE] stream index {}", pl));
        }
        self.append_log(
            job_id,
            format!("[SYSTEM] Detected {} audio tracks", acount),
        );

        let plan = resolve_job_plan(
            &job.file_path,
            profile.as_ref(),
            job.raw_args.as_deref(),
            &job.extra_args,
            job.output_path.as_deref(),
            job.prefix_override.as_deref(),
            job.suffix_override.as_deref(),
            job.ext_override.as_deref(),
            &settings,
            acount,
        )?;

        if let Some(parent) = Path::new(&plan.output_path).parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create output directory: {}", e))?;
            }
        }

        let input_size = fs::metadata(&job.file_path).ok().map(|m| m.len());
        {
            let mut guard = self.inner.lock().unwrap();
            if let Some(j) = guard.jobs.iter_mut().find(|j| j.id == *job_id) {
                j.output_path = Some(plan.output_path.clone());
                j.resolved_command = Some(plan.command_line.clone());
                j.input_size_bytes = input_size;
                if j.status == JobStatus::Cancelled {
                    return Ok((-1, true, input_size, None, None));
                }
            }
        }
        self.notify_ui();

        self.append_log(job_id, format!("[SYSTEM] Output file: {}", plan.output_path));
        self.append_log(
            job_id,
            format!("[SYSTEM] Launching FFmpeg with args: {}", plan.args.join(" ")),
        );

        let ffmpeg_bin = resolve_binary("ffmpeg");
        let mut cmd = Command::new(&ffmpeg_bin);
        cmd.args(&plan.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn FFmpeg ({}): {}", ffmpeg_bin.display(), e))?;

        let pid = child.id();
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut guard = self.inner.lock().unwrap();
            guard.active_processes.insert(
                job_id.to_string(),
                ActiveProcess {
                    pid,
                    cancelled: cancelled.clone(),
                },
            );
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let engine_out = self.clone();
        let job_id_out = job_id.to_string();
        let stdout_handle = thread::spawn(move || {
            if let Some(out) = stdout {
                let reader = BufReader::new(out);
                for line in reader.lines().map_while(Result::ok) {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        engine_out.append_log(&job_id_out, format!("[FFMPEG] {}", trimmed));
                    }
                }
            }
        });

        let mut total_duration_secs = 0.0f64;
        if let Some(mut err_stream) = stderr {
            let mut buf = [0u8; 4096];
            let mut acc = Vec::<u8>::new();

            loop {
                match err_stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        while let Some(pos) = acc.iter().position(|&b| b == b'\n' || b == b'\r') {
                            let raw_line = acc.drain(..=pos).collect::<Vec<u8>>();
                            let text = String::from_utf8_lossy(&raw_line);
                            let line = text.trim();
                            if !line.is_empty() {
                                self.process_stderr_line(job_id, line, &mut total_duration_secs);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }

            if !acc.is_empty() {
                let text = String::from_utf8_lossy(&acc);
                let line = text.trim();
                if !line.is_empty() {
                    self.process_stderr_line(job_id, line, &mut total_duration_secs);
                }
            }
        }

        let status = child
            .wait()
            .map_err(|e| format!("Failed waiting for FFmpeg: {}", e))?;
        let _ = stdout_handle.join();

        let code = status.code().unwrap_or(-1);
        let was_cancelled = cancelled.load(Ordering::SeqCst);

        let mut out_size = None;
        let mut compression_ratio = None;

        if code == 0 && !was_cancelled {
            if let Ok(out_meta) = fs::metadata(&plan.output_path) {
                let osize = out_meta.len();
                out_size = Some(osize);
                if let Some(isize) = input_size {
                    if isize > 0 && osize > 0 {
                        let ratio = (osize as f64 / isize as f64) * 100.0;
                        let ratio_str = format!("{:.1}% of original", ratio);
                        self.append_log(
                            job_id,
                            format!(
                                "[SYSTEM] Final size: {} ({} -> {})",
                                ratio_str,
                                format_bytes(isize),
                                format_bytes(osize)
                            ),
                        );
                        compression_ratio = Some(ratio_str);
                    }
                }
            }
        }

        Ok((code, was_cancelled, input_size, out_size, compression_ratio))
    }

    fn process_stderr_line(&self, job_id: &str, line: &str, total_duration_secs: &mut f64) {
        self.append_log(job_id, format!("[FFMPEG] {}", line));

        static RE_DURATION: OnceLock<Regex> = OnceLock::new();
        static RE_FPS: OnceLock<Regex> = OnceLock::new();
        static RE_BITRATE: OnceLock<Regex> = OnceLock::new();
        static RE_SPEED: OnceLock<Regex> = OnceLock::new();
        static RE_TIME: OnceLock<Regex> = OnceLock::new();

        let re_duration = RE_DURATION
            .get_or_init(|| Regex::new(r"Duration:\s+(\d{2}):(\d{2}):(\d{2}\.\d+)").unwrap());
        let re_fps = RE_FPS.get_or_init(|| Regex::new(r"fps=\s*([\d.]+)").unwrap());
        let re_bitrate =
            RE_BITRATE.get_or_init(|| Regex::new(r"bitrate=\s*([\d.]+)\s*kbits/s").unwrap());
        let re_speed = RE_SPEED.get_or_init(|| Regex::new(r"speed=\s*([\d.]+)x").unwrap());
        let re_time = RE_TIME
            .get_or_init(|| Regex::new(r"time=(-?\d{2}):(\d{2}):(\d{2}\.\d+)").unwrap());

        if let Some(cap) = re_duration.captures(line) {
            let h: f64 = cap[1].parse().unwrap_or(0.0);
            let m: f64 = cap[2].parse().unwrap_or(0.0);
            let s: f64 = cap[3].parse().unwrap_or(0.0);
            *total_duration_secs = h * 3600.0 + m * 60.0 + s;
            self.append_log(
                job_id,
                format!("[DEBUG] PARSED DURATION: {:.2}s", *total_duration_secs),
            );
        }

        let fps_str = re_fps.captures(line).map(|c| c[1].to_string());
        let mut bitrate_str = None;
        let mut est_size_str = None;
        if let Some(cap) = re_bitrate.captures(line) {
            bitrate_str = Some(format!("{}kbits/s", &cap[1]));
            if *total_duration_secs > 0.0 {
                if let Ok(kbps) = cap[1].parse::<f64>() {
                    if kbps > 0.0 {
                        let total_mb = (kbps * *total_duration_secs) / (8.0 * 1024.0);
                        est_size_str = Some(if total_mb >= 1024.0 {
                            format!("{:.2} GB", total_mb / 1024.0)
                        } else {
                            format!("{:.2} MB", total_mb)
                        });
                    }
                }
            }
        }

        let speed_val = re_speed
            .captures(line)
            .and_then(|c| c[1].parse::<f64>().ok());
        let speed_str = speed_val.map(|v| format!("{:.2}x", v));

        let mut progress_val = None;
        let mut eta_str = None;
        if let Some(cap) = re_time.captures(line) {
            if !cap[1].starts_with('-') && *total_duration_secs > 0.0 {
                let h: f64 = cap[1].parse().unwrap_or(0.0);
                let m: f64 = cap[2].parse().unwrap_or(0.0);
                let s: f64 = cap[3].parse().unwrap_or(0.0);
                let cur_secs = h * 3600.0 + m * 60.0 + s;
                let pct = ((cur_secs / *total_duration_secs) * 100.0).clamp(0.0, 100.0) as f32;
                progress_val = Some(pct);

                if let Some(sp) = speed_val {
                    if sp > 0.0 {
                        let remaining = (*total_duration_secs - cur_secs).max(0.0) / sp;
                        eta_str = Some(format_duration(remaining));
                    }
                }
            }
        }

        if progress_val.is_some()
            || fps_str.is_some()
            || bitrate_str.is_some()
            || speed_str.is_some()
            || eta_str.is_some()
            || est_size_str.is_some()
        {
            {
                let mut guard = self.inner.lock().unwrap();
                if let Some(j) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
                    if let Some(p) = progress_val {
                        j.progress = p;
                    }
                    if fps_str.is_some() {
                        j.fps = fps_str;
                    }
                    if bitrate_str.is_some() {
                        j.bitrate = bitrate_str;
                    }
                    if speed_str.is_some() {
                        j.speed = speed_str;
                    }
                    if eta_str.is_some() {
                        j.eta = eta_str;
                    }
                    if est_size_str.is_some() {
                        j.est_size = est_size_str;
                    }
                }
            }
            self.notify_ui();
        }
    }
}

fn kill_process_tree(pid: u32) {
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let _ = cmd.output();
}
