use crate::cli::GuiLaunchPlan;
use crate::engine::QueueEngine;
use crate::ipc::{start_ipc_server, GuiWakeSignals};
use crate::models::{
    AppSettings, EnqueueSpec, Job, JobPriority, JobStatus, OverwriteMode, ProbeResult, Profile,
    ProfileWithPath, APP_VERSION,
};
use crate::paths::{
    download_and_run_installer, fetch_latest_github_release, format_bytes, get_app_data_dir,
    get_profiles_dir, is_autostart_enabled, open_folder_in_explorer, open_with_default_app,
    remove_daemon_info, set_autostart_enabled, show_in_explorer,
};
use crate::probe::probe_media_detailed;
use crate::profiles::{
    builtin_templates, collect_media_files, delete_profile, generate_sendto_shortcuts,
    get_profile_by_name, import_profile_files, load_profiles, normalize_ext, normalize_opt,
    resolve_job_plan, save_profile, tokenize_args,
};
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon as TrayIconImage, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    Queue,
    Profiles,
    Inspector,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JobFilterStatus {
    All,
    Active,
    Completed,
    Errors,
}

impl JobFilterStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Active => "Active",
            Self::Completed => "Completed",
            Self::Errors => "Errors / Cancelled",
        }
    }

    fn matches(&self, s: JobStatus) -> bool {
        match self {
            Self::All => true,
            Self::Active => s.is_active(),
            Self::Completed => s == JobStatus::Completed,
            Self::Errors => matches!(s, JobStatus::Error | JobStatus::Cancelled),
        }
    }
}

struct ToastBanner {
    text: String,
    is_error: bool,
}

struct InspectorState {
    file_path: String,
    profile_name: String,
    probe: Option<ProbeResult>,
    error: Option<String>,
}

struct UpdateCheckState {
    status: String,
    release_url: Option<String>,
    setup_url: Option<String>,
    checking: bool,
    installing: bool,
}

pub struct QueueManagerApp {
    engine: QueueEngine,
    wake_signals: GuiWakeSignals,
    ipc_port: u16,
    active_tab: ActiveTab,

    // Queue tab controls
    selected_profile: String,
    selected_priority: JobPriority,
    queue_search: String,
    queue_filter: JobFilterStatus,

    // Profiles tab state
    profiles: Vec<ProfileWithPath>,
    edit_name: String,
    edit_args: String,
    edit_prefix: String,
    edit_suffix: String,
    edit_ext: String,
    selected_template_idx: usize,

    // Inspector tab state
    inspector: InspectorState,

    // Settings tab state
    settings_draft: AppSettings,
    autostart_enabled: bool,
    custom_output_dir_buf: String,
    update_state: Arc<Mutex<UpdateCheckState>>,

    // System tray & lifecycle
    _tray_icon: Option<TrayIcon>,
    quit_requested: Arc<AtomicBool>,
    tray_toggle_pause_requested: Arc<AtomicBool>,
    toast: Option<ToastBanner>,
    hwnd_registered: bool,
    dark_mode_applied: Option<bool>,
}

impl QueueManagerApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        launch: GuiLaunchPlan,
    ) -> Self {
        let engine = QueueEngine::new(true);
        engine.set_repaint_ctx(cc.egui_ctx.clone());
        engine.start_scheduler();

        let wake_signals = GuiWakeSignals::new();
        let ipc_port = start_ipc_server(
            engine.clone(),
            wake_signals.clone(),
            Some(cc.egui_ctx.clone()),
        )
        .unwrap_or(0);

        let profiles = load_profiles().unwrap_or_default();
        let default_profile = profiles
            .iter()
            .find(|p| p.profile.name == "Encode Clip Audio Merge AV1")
            .or_else(|| profiles.first())
            .map(|p| p.profile.name.clone())
            .unwrap_or_default();

        // Handle launch SendTo enqueue args if passed
        if let Some(ref prof_arg) = launch.enqueue_profile {
            if !launch.enqueue_files.is_empty() {
                let resolved_name = get_profile_by_name(prof_arg)
                    .map(|p| p.profile.name)
                    .unwrap_or_else(|_| prof_arg.trim_end_matches(".json").to_string());
                let files = collect_media_files(&launch.enqueue_files, false);
                let specs: Vec<EnqueueSpec> = files
                    .into_iter()
                    .map(|f| EnqueueSpec {
                        file_path: f,
                        profile_name: resolved_name.clone(),
                        raw_args: None,
                        extra_args: Vec::new(),
                        output_path_override: None,
                        prefix_override: None,
                        suffix_override: None,
                        ext_override: None,
                        priority: JobPriority::Normal,
                    })
                    .collect();
                if !specs.is_empty() {
                    engine.enqueue(specs);
                }
            }
        }

        let settings_draft = engine.settings();
        let custom_output_dir_buf = settings_draft.custom_output_dir.clone().unwrap_or_default();
        let autostart_enabled = is_autostart_enabled();

        let quit_requested = Arc::new(AtomicBool::new(false));
        let tray_toggle_pause_requested = Arc::new(AtomicBool::new(false));

        let tray_icon = setup_tray(
            &wake_signals,
            &quit_requested,
            &tray_toggle_pause_requested,
            cc.egui_ctx.clone(),
        );

        let mut app = Self {
            engine,
            wake_signals,
            ipc_port,
            active_tab: ActiveTab::Queue,
            selected_profile: default_profile.clone(),
            selected_priority: JobPriority::Normal,
            queue_search: String::new(),
            queue_filter: JobFilterStatus::All,
            profiles,
            edit_name: String::new(),
            edit_args: "-c:v libx264 -crf 23 -c:a aac".to_string(),
            edit_prefix: String::new(),
            edit_suffix: String::new(),
            edit_ext: String::new(),
            selected_template_idx: 0,
            inspector: InspectorState {
                file_path: String::new(),
                profile_name: default_profile,
                probe: None,
                error: None,
            },
            settings_draft,
            autostart_enabled,
            custom_output_dir_buf,
            update_state: Arc::new(Mutex::new(UpdateCheckState {
                status: "Idle".to_string(),
                release_url: None,
                setup_url: None,
                checking: false,
                installing: false,
            })),
            _tray_icon: tray_icon,
            quit_requested,
            tray_toggle_pause_requested,
            toast: None,
            hwnd_registered: false,
            dark_mode_applied: None,
        };

        app.apply_theme(&cc.egui_ctx);
        app
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let dark = self.settings_draft.dark_mode;
        if self.dark_mode_applied == Some(dark) {
            return;
        }
        self.dark_mode_applied = Some(dark);
        let mut visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.window_rounding = egui::Rounding::same(8.0);
        visuals.widgets.inactive.rounding = egui::Rounding::same(5.0);
        visuals.widgets.hovered.rounding = egui::Rounding::same(5.0);
        visuals.widgets.active.rounding = egui::Rounding::same(5.0);
        ctx.set_visuals(visuals);
    }

    fn reload_profiles(&mut self) {
        self.profiles = load_profiles().unwrap_or_default();
        if self.selected_profile.is_empty()
            || !self
                .profiles
                .iter()
                .any(|p| p.profile.name == self.selected_profile)
        {
            self.selected_profile = self
                .profiles
                .iter()
                .find(|p| p.profile.name == "Encode Clip Audio Merge AV1")
                .or_else(|| self.profiles.first())
                .map(|p| p.profile.name.clone())
                .unwrap_or_default();
        }
        if self.inspector.profile_name.is_empty() {
            self.inspector.profile_name = self.selected_profile.clone();
        }
    }

    fn set_toast(&mut self, text: impl Into<String>, is_error: bool) {
        self.toast = Some(ToastBanner {
            text: text.into(),
            is_error,
        });
    }

    fn enqueue_paths(&mut self, paths: Vec<PathBuf>) {
        if self.selected_profile.is_empty() {
            self.set_toast("Create or select a profile before queuing files.", true);
            return;
        }
        let inputs: Vec<String> = paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let media_files = collect_media_files(&inputs, true);
        if media_files.is_empty() {
            self.set_toast("No supported media files found to enqueue.", true);
            return;
        }
        let specs: Vec<EnqueueSpec> = media_files
            .into_iter()
            .map(|f| EnqueueSpec {
                file_path: f,
                profile_name: self.selected_profile.clone(),
                raw_args: None,
                extra_args: Vec::new(),
                output_path_override: None,
                prefix_override: None,
                suffix_override: None,
                ext_override: None,
                priority: self.selected_priority,
            })
            .collect();
        let count = specs.len();
        self.engine.enqueue(specs);
        self.set_toast(
            format!("Added {} job(s) using profile '{}'.", count, self.selected_profile),
            false,
        );
    }

    fn register_hwnd_once(&mut self) {
        if self.hwnd_registered {
            return;
        }
        self.hwnd_registered = true;
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;
            let title: Vec<u16> = "FFmpeg Queue Manager\0".encode_utf16().collect();
            let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
            if !hwnd.is_null() {
                self.wake_signals
                    .hwnd
                    .store(hwnd as isize, Ordering::SeqCst);
            }
        }
    }
}

impl eframe::App for QueueManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.register_hwnd_once();
        self.apply_theme(ctx);

        // Process IPC/tray signals
        if self.quit_requested.load(Ordering::SeqCst) {
            remove_daemon_info();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        if self
            .tray_toggle_pause_requested
            .swap(false, Ordering::SeqCst)
        {
            let paused = self.engine.snapshot().paused;
            self.engine.set_paused(!paused);
        }

        if self.wake_signals.show_window.swap(false, Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        if self.wake_signals.reload_profiles.swap(false, Ordering::SeqCst) {
            self.reload_profiles();
        }

        // Sync settings if updated via CLI IPC
        let engine_settings = self.engine.settings();
        if engine_settings.max_concurrency != self.settings_draft.max_concurrency
            || engine_settings.auto_clear != self.settings_draft.auto_clear
            || engine_settings.minimize_to_tray != self.settings_draft.minimize_to_tray
            || engine_settings.overwrite_mode != self.settings_draft.overwrite_mode
            || engine_settings.custom_output_dir != self.settings_draft.custom_output_dir
            || engine_settings.dark_mode != self.settings_draft.dark_mode
        {
            self.settings_draft = engine_settings;
            self.custom_output_dir_buf = self
                .settings_draft
                .custom_output_dir
                .clone()
                .unwrap_or_default();
            self.apply_theme(ctx);
        }

        // Intercept window close to minimize to tray
        let close_req = ctx.input(|i| i.viewport().close_requested());
        if close_req {
            if self.settings_draft.minimize_to_tray && !self.quit_requested.load(Ordering::SeqCst) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                remove_daemon_info();
            }
        }

        // Handle dropped files anywhere in window
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|df| df.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            if self.active_tab == ActiveTab::Inspector && dropped.len() == 1 && dropped[0].is_file()
            {
                self.inspector.file_path = dropped[0].to_string_lossy().to_string();
                match probe_media_detailed(&self.inspector.file_path) {
                    Ok(p) => {
                        self.inspector.probe = Some(p);
                        self.inspector.error = None;
                    }
                    Err(e) => {
                        self.inspector.probe = None;
                        self.inspector.error = Some(e);
                    }
                }
            } else {
                self.enqueue_paths(dropped);
                self.active_tab = ActiveTab::Queue;
            }
        }

        let snap = self.engine.snapshot();
        if snap.active_runners > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        // Top Navigation Bar
        egui::TopBottomPanel::top("top_nav").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("FFmpeg Queue Manager");
                ui.label(
                    egui::RichText::new(format!("v{}", APP_VERSION))
                        .small()
                        .strong()
                        .color(egui::Color32::from_rgb(100, 170, 255)),
                );
                if self.ipc_port > 0 {
                    ui.label(
                        egui::RichText::new(format!("CLI IPC :{}", self.ipc_port))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let theme_label = if self.settings_draft.dark_mode {
                        "Light Theme"
                    } else {
                        "Dark Theme"
                    };
                    if ui.button(theme_label).clicked() {
                        self.settings_draft.dark_mode = !self.settings_draft.dark_mode;
                        let _ = self.engine.update_settings(self.settings_draft.clone());
                        self.apply_theme(ctx);
                    }

                    ui.separator();

                    let settings_sel = self.active_tab == ActiveTab::Settings;
                    if ui.selectable_label(settings_sel, "Settings").clicked() {
                        self.active_tab = ActiveTab::Settings;
                        self.autostart_enabled = is_autostart_enabled();
                    }

                    let insp_sel = self.active_tab == ActiveTab::Inspector;
                    if ui.selectable_label(insp_sel, "Inspector & CLI").clicked() {
                        self.active_tab = ActiveTab::Inspector;
                    }

                    let prof_sel = self.active_tab == ActiveTab::Profiles;
                    let prof_label = format!("Profiles ({})", self.profiles.len());
                    if ui.selectable_label(prof_sel, prof_label).clicked() {
                        self.active_tab = ActiveTab::Profiles;
                    }

                    let queue_sel = self.active_tab == ActiveTab::Queue;
                    let active_cnt = snap
                        .jobs
                        .iter()
                        .filter(|j| j.status == JobStatus::Running || j.status == JobStatus::Queued)
                        .count();
                    let queue_label = if active_cnt > 0 {
                        format!("Queue ({} active)", active_cnt)
                    } else {
                        format!("Queue ({})", snap.jobs.len())
                    };
                    if ui.selectable_label(queue_sel, queue_label).clicked() {
                        self.active_tab = ActiveTab::Queue;
                    }
                });
            });

            if let Some((banner_text, banner_err)) =
                self.toast.as_ref().map(|b| (b.text.clone(), b.is_error))
            {
                ui.add_space(4.0);
                let color = if banner_err {
                    egui::Color32::from_rgb(235, 90, 90)
                } else {
                    egui::Color32::from_rgb(80, 200, 120)
                };
                let mut dismiss = false;
                ui.horizontal(|ui| {
                    ui.colored_label(color, &banner_text);
                    if ui.small_button("Dismiss").clicked() {
                        dismiss = true;
                    }
                });
                if dismiss {
                    self.toast = None;
                }
            }
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.active_tab {
                ActiveTab::Queue => self.render_queue_tab(ui, &snap),
                ActiveTab::Profiles => self.render_profiles_tab(ui),
                ActiveTab::Inspector => self.render_inspector_tab(ui),
                ActiveTab::Settings => self.render_settings_tab(ui),
            }
        });

        // Drag-and-drop overlay feedback
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let screen = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop_overlay"),
            ));
            painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
            let msg = if self.active_tab == ActiveTab::Inspector {
                "Drop a media file to inspect with ffprobe".to_string()
            } else {
                format!(
                    "Drop media files or folders to queue with '{}'",
                    self.selected_profile
                )
            };
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                msg,
                egui::FontId::proportional(20.0),
                egui::Color32::WHITE,
            );
        }
    }
}

impl QueueManagerApp {
    fn render_queue_tab(
        &mut self,
        ui: &mut egui::Ui,
        snap: &crate::models::QueueSnapshot,
    ) {
        // Quick Enqueue Toolbar
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("Profile:").strong());
                egui::ComboBox::from_id_salt("quick_profile_combo")
                    .selected_text(if self.selected_profile.is_empty() {
                        "(none)"
                    } else {
                        &self.selected_profile
                    })
                    .width(230.0)
                    .show_ui(ui, |ui| {
                        for p in &self.profiles {
                            ui.selectable_value(
                                &mut self.selected_profile,
                                p.profile.name.clone(),
                                &p.profile.name,
                            );
                        }
                    });

                ui.label("Priority:");
                egui::ComboBox::from_id_salt("quick_prio_combo")
                    .selected_text(self.selected_priority.as_str())
                    .width(80.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.selected_priority, JobPriority::High, "High");
                        ui.selectable_value(
                            &mut self.selected_priority,
                            JobPriority::Normal,
                            "Normal",
                        );
                        ui.selectable_value(&mut self.selected_priority, JobPriority::Low, "Low");
                    });

                if ui.button("+ Add Files...").clicked() {
                    if let Some(files) = rfd::FileDialog::new()
                        .add_filter(
                            "Media Files",
                            &[
                                "mp4", "mkv", "mov", "avi", "webm", "ts", "flv", "m4v", "wmv",
                                "mp3", "wav", "flac", "opus",
                            ],
                        )
                        .add_filter("All Files", &["*"])
                        .pick_files()
                    {
                        self.enqueue_paths(files);
                    }
                }

                if ui.button("+ Add Folder...").clicked() {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        self.enqueue_paths(vec![folder]);
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Generate SendTo Shortcuts").clicked() {
                        match generate_sendto_shortcuts() {
                            Ok(msg) => self.set_toast(msg, false),
                            Err(err) => self.set_toast(err, true),
                        }
                    }
                });
            });
        });

        ui.add_space(6.0);

        // Summary Stats & Batch Controls Bar
        let running_cnt = snap
            .jobs
            .iter()
            .filter(|j| j.status == JobStatus::Running)
            .count();
        let queued_cnt = snap
            .jobs
            .iter()
            .filter(|j| j.status == JobStatus::Queued)
            .count();
        let completed_cnt = snap
            .jobs
            .iter()
            .filter(|j| j.status == JobStatus::Completed)
            .count();
        let err_cnt = snap
            .jobs
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Error | JobStatus::Cancelled))
            .count();

        let (total_in, total_out) = snap
            .jobs
            .iter()
            .filter(|j| j.status == JobStatus::Completed)
            .fold((0u64, 0u64), |(ti, to), j| {
                if let (Some(i), Some(o)) = (j.input_size_bytes, j.output_size_bytes) {
                    (ti + i, to + o)
                } else {
                    (ti, to)
                }
            });

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                status_pill(
                    ui,
                    format!("Running: {}/{}", running_cnt, snap.max_concurrency),
                    egui::Color32::from_rgb(55, 130, 235),
                );
                status_pill(
                    ui,
                    format!("Queued: {}", queued_cnt),
                    egui::Color32::from_rgb(125, 135, 150),
                );
                status_pill(
                    ui,
                    format!("Completed: {}", completed_cnt),
                    egui::Color32::from_rgb(45, 165, 95),
                );
                if err_cnt > 0 {
                    status_pill(
                        ui,
                        format!("Failed: {}", err_cnt),
                        egui::Color32::from_rgb(205, 70, 70),
                    );
                }
                if total_in > 0 && total_out > 0 {
                    let saved_str = if total_in >= total_out {
                        let saved = total_in - total_out;
                        let pct = (saved as f64 / total_in as f64) * 100.0;
                        format!("Saved {} ({:.1}%)", format_bytes(saved), pct)
                    } else {
                        let growth = total_out - total_in;
                        format!("Output +{}", format_bytes(growth))
                    };
                    status_pill(ui, saved_str, egui::Color32::from_rgb(40, 155, 135));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Stop & Clear All").clicked() {
                        self.engine.stop_and_clear_all();
                    }
                    if ui.button("Stop All").clicked() {
                        self.engine.stop_job("all");
                    }
                    if ui.button("Clear Completed").clicked() {
                        self.engine.clear_jobs(true, false, false, None);
                    }
                    if ui.button("Retry Failed").clicked() {
                        self.engine.retry_job("failed");
                    }
                    let pause_btn = if snap.paused {
                        "Resume Queue"
                    } else {
                        "Pause Queue"
                    };
                    if ui.button(pause_btn).clicked() {
                        self.engine.set_paused(!snap.paused);
                    }
                });
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.queue_search)
                        .hint_text("Search filename or profile...")
                        .desired_width(240.0),
                );
                egui::ComboBox::from_id_salt("queue_filter_status")
                    .selected_text(self.queue_filter.label())
                    .show_ui(ui, |ui| {
                        for opt in [
                            JobFilterStatus::All,
                            JobFilterStatus::Active,
                            JobFilterStatus::Completed,
                            JobFilterStatus::Errors,
                        ] {
                            ui.selectable_value(&mut self.queue_filter, opt, opt.label());
                        }
                    });
                if !self.queue_search.is_empty() && ui.small_button("Clear").clicked() {
                    self.queue_search.clear();
                }
            });
        });

        ui.add_space(6.0);

        // Job list
        let search_lower = self.queue_search.trim().to_lowercase();
        let filtered_jobs: Vec<Job> = snap
            .jobs
            .iter()
            .filter(|j| self.queue_filter.matches(j.status))
            .filter(|j| {
                search_lower.is_empty()
                    || j.file_path.to_lowercase().contains(&search_lower)
                    || j.profile_name.to_lowercase().contains(&search_lower)
                    || j.id.to_lowercase().contains(&search_lower)
            })
            .cloned()
            .collect();

        if filtered_jobs.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new(
                        "No jobs in queue. Drop media files here, click '+ Add Files...', use Windows SendTo, or queue via CLI.",
                    )
                    .color(ui.visuals().weak_text_color()),
                );
            });
            return;
        }

        egui::ScrollArea::vertical()
            .id_salt("queue_jobs_scroll")
            .show(ui, |ui| {
                for job in filtered_jobs {
                    self.render_job_card(ui, &job);
                    ui.add_space(6.0);
                }
            });
    }

    fn render_job_card(&mut self, ui: &mut egui::Ui, job: &Job) {
        let filename = std::path::Path::new(&job.file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| job.file_path.clone());

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&job.id)
                        .monospace()
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                let title_resp = ui.label(egui::RichText::new(&filename).strong());
                title_resp.on_hover_text(format!(
                    "Input: {}\nOutput: {}",
                    job.file_path,
                    job.output_path.as_deref().unwrap_or("(pending)")
                ));

                status_pill(
                    ui,
                    job.profile_name.clone(),
                    egui::Color32::from_rgb(90, 100, 120),
                );
                if job.priority != JobPriority::Normal {
                    let prio_col = if job.priority == JobPriority::High {
                        egui::Color32::from_rgb(220, 130, 40)
                    } else {
                        egui::Color32::from_rgb(100, 110, 125)
                    };
                    status_pill(ui, format!("Priority: {}", job.priority), prio_col);
                }

                let status_col = match job.status {
                    JobStatus::Queued => egui::Color32::from_rgb(110, 120, 135),
                    JobStatus::Running => egui::Color32::from_rgb(45, 135, 240),
                    JobStatus::Paused => egui::Color32::from_rgb(210, 155, 45),
                    JobStatus::Completed => egui::Color32::from_rgb(45, 175, 95),
                    JobStatus::Error => egui::Color32::from_rgb(215, 65, 65),
                    JobStatus::Cancelled => egui::Color32::from_rgb(165, 85, 85),
                };
                status_pill(ui, job.status.as_str().to_string(), status_col);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Triple-dot context menu
                    ui.menu_button("...", |ui| {
                        if ui.button("Show Input in Explorer").clicked() {
                            show_in_explorer(&job.file_path);
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(
                                job.output_path.is_some(),
                                egui::Button::new("Show Output in Explorer"),
                            )
                            .clicked()
                        {
                            if let Some(ref out) = job.output_path {
                                show_in_explorer(out);
                            }
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(
                                job.status == JobStatus::Completed && job.output_path.is_some(),
                                egui::Button::new("Play / Open Output File"),
                            )
                            .clicked()
                        {
                            if let Some(ref out) = job.output_path {
                                open_with_default_app(out);
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(
                                job.resolved_command.is_some(),
                                egui::Button::new("Copy FFmpeg Command"),
                            )
                            .clicked()
                        {
                            if let Some(ref cmd) = job.resolved_command {
                                ui.ctx().copy_text(cmd.clone());
                                self.set_toast("Copied FFmpeg command to clipboard.", false);
                            }
                            ui.close_menu();
                        }
                        if ui.button("Copy Logs").clicked() {
                            ui.ctx().copy_text(job.logs.join("\n"));
                            self.set_toast("Copied job logs to clipboard.", false);
                            ui.close_menu();
                        }
                        if job.status.is_terminal() && ui.button("Retry Job").clicked() {
                            self.engine.retry_job(&job.id);
                            ui.close_menu();
                        }
                        if job.status == JobStatus::Queued
                            && ui.button("Move to Top of Queue").clicked()
                        {
                            self.engine.move_to_top(&job.id);
                            ui.close_menu();
                        }
                        if job.status != JobStatus::Running
                            && ui.button("Remove from Queue").clicked()
                        {
                            self.engine.clear_jobs(false, false, false, Some(&job.id));
                            ui.close_menu();
                        }
                    });

                    if job.status == JobStatus::Running || job.status == JobStatus::Queued {
                        if ui
                            .button(egui::RichText::new("Stop").color(egui::Color32::from_rgb(235, 95, 95)))
                            .clicked()
                        {
                            self.engine.stop_job(&job.id);
                        }
                    } else if ui.button("Clear").clicked() {
                        self.engine.clear_jobs(false, false, false, Some(&job.id));
                    }

                    let log_lbl = if job.expanded_terminal {
                        "Hide Logs"
                    } else {
                        "Logs"
                    };
                    if ui.button(log_lbl).clicked() {
                        self.engine.toggle_terminal(&job.id);
                    }
                });
            });

            ui.add_space(4.0);

            let bar_fraction = match job.status {
                JobStatus::Completed => 1.0,
                JobStatus::Queued => 0.0,
                _ => (job.progress / 100.0).clamp(0.0, 1.0),
            };
            let bar_fill = match job.status {
                JobStatus::Completed => egui::Color32::from_rgb(45, 175, 95),
                JobStatus::Error => egui::Color32::from_rgb(215, 65, 65),
                JobStatus::Cancelled => egui::Color32::from_rgb(160, 90, 90),
                _ => egui::Color32::from_rgb(50, 140, 245),
            };
            ui.add(
                egui::ProgressBar::new(bar_fraction)
                    .fill(bar_fill)
                    .show_percentage(),
            );

            // Live Telemetry / Final Compression Ratio line
            if job.status == JobStatus::Running || job.status == JobStatus::Completed {
                let mut parts: Vec<String> = Vec::new();
                if let Some(ref fps) = job.fps {
                    parts.push(format!("{} fps", fps));
                }
                if let Some(ref sp) = job.speed {
                    parts.push(sp.clone());
                }
                if let Some(ref br) = job.bitrate {
                    parts.push(br.clone());
                }
                if let Some(ref ratio) = job.compression_ratio {
                    let size_detail = match (job.input_size_bytes, job.output_size_bytes) {
                        (Some(i), Some(o)) => {
                            format!("Final Size: {} ({} -> {})", ratio, format_bytes(i), format_bytes(o))
                        }
                        _ => format!("Final Size: {}", ratio),
                    };
                    parts.push(size_detail);
                } else if let Some(ref est) = job.est_size {
                    parts.push(format!("Est. {}", est));
                }
                parts.push(format!("ETA: {}", job.eta.as_deref().unwrap_or("--:--")));

                ui.label(
                    egui::RichText::new(parts.join(" • "))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            }

            // Expandable Monospace Terminal Panel
            if job.expanded_terminal {
                ui.add_space(6.0);
                let term_bg = egui::Color32::from_rgb(18, 20, 24);
                egui::Frame::none()
                    .fill(term_bg)
                    .rounding(egui::Rounding::same(5.0))
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt(format!("term_{}", job.id))
                            .max_height(220.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                if job.logs.is_empty() {
                                    ui.label(
                                        egui::RichText::new("No logs yet...")
                                            .monospace()
                                            .small()
                                            .color(egui::Color32::GRAY),
                                    );
                                } else {
                                    for line in &job.logs {
                                        let color = if line.starts_with("[SYSTEM ERROR]") {
                                            egui::Color32::from_rgb(245, 100, 100)
                                        } else if line.starts_with("[SYSTEM]") {
                                            egui::Color32::from_rgb(100, 205, 245)
                                        } else if line.starts_with("[FFPROBE]") {
                                            egui::Color32::from_rgb(245, 195, 100)
                                        } else {
                                            egui::Color32::from_rgb(215, 220, 228)
                                        };
                                        ui.label(
                                            egui::RichText::new(line)
                                                .monospace()
                                                .small()
                                                .color(color),
                                        );
                                    }
                                }
                            });
                    });
            }
        });
    }

    fn render_profiles_tab(&mut self, ui: &mut egui::Ui) {
        let templates = builtin_templates();

        egui::ScrollArea::vertical()
            .id_salt("profiles_scroll")
            .show(ui, |ui| {
                // Create / Edit Profile Card
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Create / Edit Profile");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Load Template").clicked() {
                                if let Some(tmpl) = templates.get(self.selected_template_idx) {
                                    self.edit_name = tmpl.name.clone();
                                    self.edit_args = tmpl.args.join(" ");
                                    self.edit_prefix = tmpl.prefix.clone().unwrap_or_default();
                                    self.edit_suffix = tmpl.suffix.clone().unwrap_or_default();
                                    self.edit_ext = tmpl.extension.clone().unwrap_or_default();
                                }
                            }
                            egui::ComboBox::from_id_salt("template_picker")
                                .selected_text(&templates[self.selected_template_idx].name)
                                .show_ui(ui, |ui| {
                                    for (i, t) in templates.iter().enumerate() {
                                        ui.selectable_value(
                                            &mut self.selected_template_idx,
                                            i,
                                            &t.name,
                                        );
                                    }
                                });
                        });
                    });

                    ui.add_space(6.0);
                    ui.label("Profile Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.edit_name)
                            .hint_text("e.g. Convert to MP4")
                            .desired_width(f32::INFINITY),
                    );

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("FFmpeg Arguments");
                        if ui.small_button("+ {{AUTO_AMERGE_LOUDNORM}}").clicked() {
                            if !self.edit_args.is_empty() && !self.edit_args.ends_with(' ') {
                                self.edit_args.push(' ');
                            }
                            self.edit_args.push_str("-filter_complex {{AUTO_AMERGE_LOUDNORM}}");
                        }
                        if ui.small_button("+ {{AUTO_AMIX}}").clicked() {
                            if !self.edit_args.is_empty() && !self.edit_args.ends_with(' ') {
                                self.edit_args.push(' ');
                            }
                            self.edit_args.push_str("-filter_complex {{AUTO_AMIX}}");
                        }
                        if ui.small_button("+ {{MASTER_RAW}} (Suffix)").clicked() {
                            self.edit_suffix.push_str("{{MASTER_RAW}}");
                        }
                    });
                    ui.add(
                        egui::TextEdit::multiline(&mut self.edit_args)
                            .code_editor()
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );

                    ui.add_space(6.0);
                    ui.columns(3, |cols| {
                        cols[0].label("Prefix (Optional)");
                        cols[0].add(
                            egui::TextEdit::singleline(&mut self.edit_prefix)
                                .hint_text("e.g. encoded_")
                                .desired_width(f32::INFINITY),
                        );
                        cols[1].label("Suffix (Optional)");
                        cols[1].add(
                            egui::TextEdit::singleline(&mut self.edit_suffix)
                                .hint_text("e.g.  [AV1{{MASTER_RAW}}]")
                                .desired_width(f32::INFINITY),
                        );
                        cols[2].label("Extension (Optional)");
                        cols[2].add(
                            egui::TextEdit::singleline(&mut self.edit_ext)
                                .hint_text("e.g. .mkv")
                                .desired_width(f32::INFINITY),
                        );
                    });

                    // Live dry-run preview (1 track vs 3 tracks)
                    let draft_profile = Profile {
                        name: if self.edit_name.trim().is_empty() {
                            "Draft".to_string()
                        } else {
                            self.edit_name.trim().to_string()
                        },
                        args: tokenize_args(&self.edit_args),
                        prefix: normalize_opt(Some(self.edit_prefix.clone())),
                        extension: normalize_ext(Some(self.edit_ext.clone())),
                        suffix: if self.edit_suffix.is_empty() {
                            None
                        } else {
                            Some(self.edit_suffix.clone())
                        },
                    };
                    if let (Ok(p1), Ok(p3)) = (
                        resolve_job_plan(
                            r"C:\Videos\clip.mp4",
                            Some(&draft_profile),
                            None,
                            &[],
                            None,
                            None,
                            None,
                            None,
                            &self.settings_draft,
                            1,
                        ),
                        resolve_job_plan(
                            r"C:\Videos\clip.mp4",
                            Some(&draft_profile),
                            None,
                            &[],
                            None,
                            None,
                            None,
                            None,
                            &self.settings_draft,
                            3,
                        ),
                    ) {
                        ui.add_space(6.0);
                        ui.collapsing("Live Command Preview (1 Audio Track vs 3 Audio Tracks)", |ui| {
                            ui.label(egui::RichText::new("1 Track:").strong().small());
                            ui.label(
                                egui::RichText::new(&p1.command_line)
                                    .monospace()
                                    .small(),
                            );
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("3 Tracks:").strong().small());
                            ui.label(
                                egui::RichText::new(&p3.command_line)
                                    .monospace()
                                    .small(),
                            );
                        });
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save Profile").clicked() {
                            match save_profile(&draft_profile) {
                                Ok(_) => {
                                    self.set_toast(
                                        format!("Saved profile '{}'.", draft_profile.name),
                                        false,
                                    );
                                    self.reload_profiles();
                                }
                                Err(e) => self.set_toast(e, true),
                            }
                        }
                        if ui.button("Clear Editor").clicked() {
                            self.edit_name.clear();
                            self.edit_args = "-c:v libx264 -crf 23 -c:a aac".to_string();
                            self.edit_prefix.clear();
                            self.edit_suffix.clear();
                            self.edit_ext.clear();
                        }
                        if ui.button("Import Profile(s)...").clicked() {
                            if let Some(files) = rfd::FileDialog::new()
                                .add_filter("JSON Profile", &["json"])
                                .pick_files()
                            {
                                match import_profile_files(&files) {
                                    Ok(cnt) => {
                                        self.reload_profiles();
                                        self.set_toast(
                                            format!("Imported {} profile(s).", cnt),
                                            false,
                                        );
                                    }
                                    Err(e) => self.set_toast(e, true),
                                }
                            }
                        }
                        if ui.button("Open Profiles Folder").clicked() {
                            if let Ok(dir) = get_profiles_dir() {
                                open_folder_in_explorer(&dir);
                            }
                        }
                    });
                });

                ui.add_space(10.0);
                ui.heading("Existing Profiles");
                ui.add_space(4.0);

                if self.profiles.is_empty() {
                    ui.label("No profiles found.");
                } else {
                    let list = self.profiles.clone();
                    for p in list {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&p.profile.name).strong());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .button(
                                                egui::RichText::new("Delete")
                                                    .color(egui::Color32::from_rgb(235, 95, 95)),
                                            )
                                            .clicked()
                                        {
                                            match delete_profile(&p.profile.name) {
                                                Ok(_) => {
                                                    self.reload_profiles();
                                                    self.set_toast(
                                                        format!("Deleted '{}'.", p.profile.name),
                                                        false,
                                                    );
                                                }
                                                Err(e) => self.set_toast(e, true),
                                            }
                                        }
                                        if ui.button("Duplicate").clicked() {
                                            self.edit_name = format!("{} (Copy)", p.profile.name);
                                            self.edit_args = p.profile.args.join(" ");
                                            self.edit_prefix =
                                                p.profile.prefix.clone().unwrap_or_default();
                                            self.edit_suffix =
                                                p.profile.suffix.clone().unwrap_or_default();
                                            self.edit_ext =
                                                p.profile.extension.clone().unwrap_or_default();
                                        }
                                        if ui.button("Edit").clicked() {
                                            self.edit_name = p.profile.name.clone();
                                            self.edit_args = p.profile.args.join(" ");
                                            self.edit_prefix =
                                                p.profile.prefix.clone().unwrap_or_default();
                                            self.edit_suffix =
                                                p.profile.suffix.clone().unwrap_or_default();
                                            self.edit_ext =
                                                p.profile.extension.clone().unwrap_or_default();
                                        }
                                    },
                                );
                            });

                            ui.label(
                                egui::RichText::new(p.profile.args.join(" "))
                                    .monospace()
                                    .small(),
                            );
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Prefix: {}  |  Suffix: {}  |  Extension: {}",
                                        p.profile.prefix.as_deref().unwrap_or("(none)"),
                                        p.profile.suffix.as_deref().unwrap_or("(none)"),
                                        p.profile.extension.as_deref().unwrap_or("(none)")
                                    ))
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                                );
                            });
                        });
                        ui.add_space(4.0);
                    }
                }
            });
    }

    fn render_inspector_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("inspector_scroll")
            .show(ui, |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Media Inspector & Dry-Run Command Preview");
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label("Media File:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.inspector.file_path)
                                .hint_text("Select or drop a video/audio file...")
                                .desired_width(360.0),
                        );
                        if ui.button("Browse...").clicked() {
                            if let Some(file) = rfd::FileDialog::new().pick_file() {
                                self.inspector.file_path = file.to_string_lossy().to_string();
                                match probe_media_detailed(&self.inspector.file_path) {
                                    Ok(p) => {
                                        self.inspector.probe = Some(p);
                                        self.inspector.error = None;
                                    }
                                    Err(e) => {
                                        self.inspector.probe = None;
                                        self.inspector.error = Some(e);
                                    }
                                }
                            }
                        }
                        if ui.button("Run FFprobe").clicked()
                            && !self.inspector.file_path.trim().is_empty()
                        {
                            match probe_media_detailed(self.inspector.file_path.trim()) {
                                Ok(p) => {
                                    self.inspector.probe = Some(p);
                                    self.inspector.error = None;
                                }
                                Err(e) => {
                                    self.inspector.probe = None;
                                    self.inspector.error = Some(e);
                                }
                            }
                        }
                    });

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Preview with Profile:");
                        egui::ComboBox::from_id_salt("inspector_profile_combo")
                            .selected_text(&self.inspector.profile_name)
                            .show_ui(ui, |ui| {
                                for p in &self.profiles {
                                    ui.selectable_value(
                                        &mut self.inspector.profile_name,
                                        p.profile.name.clone(),
                                        &p.profile.name,
                                    );
                                }
                            });
                    });

                    if let Some(ref err) = self.inspector.error {
                        ui.colored_label(egui::Color32::from_rgb(235, 90, 90), err);
                    }

                    let inspector_probe = self.inspector.probe.clone();
                    let inspector_prof = self
                        .profiles
                        .iter()
                        .find(|p| p.profile.name == self.inspector.profile_name)
                        .cloned();

                    if let Some(ref probe) = inspector_probe {
                        ui.add_space(6.0);
                        ui.label(format!(
                            "Format: {} | Duration: {} ({:.2}s) | Size: {}",
                            probe.format_name,
                            probe.duration_formatted,
                            probe.duration_secs,
                            format_bytes(probe.size_bytes)
                        ));
                        for v in &probe.video_streams {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Video #{}: {} {}x{} @ {} fps | pix_fmt={} | colorspace={}",
                                    v.index,
                                    v.codec,
                                    v.width.unwrap_or(0),
                                    v.height.unwrap_or(0),
                                    v.fps.as_deref().unwrap_or("?"),
                                    v.pix_fmt.as_deref().unwrap_or("?"),
                                    v.color_space.as_deref().unwrap_or("?")
                                ))
                                .monospace()
                                .small(),
                            );
                        }
                        for a in &probe.audio_streams {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Audio #{}: {} {}ch ({}) {}Hz {}",
                                    a.index,
                                    a.codec,
                                    a.channels.unwrap_or(0),
                                    a.channel_layout.as_deref().unwrap_or("?"),
                                    a.sample_rate.as_deref().unwrap_or("?"),
                                    a.language.as_deref().unwrap_or("")
                                ))
                                .monospace()
                                .small(),
                            );
                        }

                        if let Some(ref prof) = inspector_prof {
                            if let Ok(plan) = resolve_job_plan(
                                &probe.file_path,
                                Some(&prof.profile),
                                None,
                                &[],
                                None,
                                None,
                                None,
                                None,
                                &self.settings_draft,
                                probe.audio_streams.len(),
                            ) {
                                ui.add_space(6.0);
                                ui.label(egui::RichText::new("Dry-Run Output Path:").strong());
                                ui.label(egui::RichText::new(&plan.output_path).monospace().small());
                                ui.label(egui::RichText::new("Dry-Run FFmpeg Command:").strong());
                                ui.label(egui::RichText::new(&plan.command_line).monospace().small());

                                ui.horizontal(|ui| {
                                    if ui.button("Queue This File").clicked() {
                                        self.selected_profile = prof.profile.name.clone();
                                        self.enqueue_paths(vec![PathBuf::from(&probe.file_path)]);
                                        self.active_tab = ActiveTab::Queue;
                                    }
                                    if ui.button("Copy Command").clicked() {
                                        ui.ctx().copy_text(plan.command_line.clone());
                                        self.set_toast("Copied dry-run command.", false);
                                    }
                                });
                            }
                        }
                    }
                });

                ui.add_space(10.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("AI & CLI Automation Quick Reference");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Copy Agent Command Cheatsheet").clicked() {
                                ui.ctx().copy_text(cli_cheatsheet().to_string());
                                self.set_toast("Copied CLI cheatsheet to clipboard.", false);
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(cli_cheatsheet())
                            .monospace()
                            .small(),
                    );
                });
            });
    }

    fn render_settings_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("settings_scroll")
            .show(ui, |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Application Settings");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("Version v{}", APP_VERSION))
                                    .strong()
                                    .color(egui::Color32::from_rgb(100, 170, 255)),
                            );
                        });
                    });
                    ui.add_space(8.0);

                    let mut changed = false;

                    if ui
                        .checkbox(
                            &mut self.autostart_enabled,
                            "Run on Startup (launch minimized to tray on login)",
                        )
                        .changed()
                    {
                        if let Err(e) = set_autostart_enabled(self.autostart_enabled) {
                            self.autostart_enabled = !self.autostart_enabled;
                            self.set_toast(format!("Failed updating Startup registry: {}", e), true);
                        } else {
                            self.set_toast("Updated Windows Startup setting.", false);
                        }
                    }

                    if ui
                        .checkbox(
                            &mut self.settings_draft.minimize_to_tray,
                            "Minimize to System Tray when closing window",
                        )
                        .changed()
                    {
                        changed = true;
                    }

                    if ui
                        .checkbox(&mut self.settings_draft.dark_mode, "Dark Theme")
                        .changed()
                    {
                        changed = true;
                    }

                    if ui
                        .checkbox(
                            &mut self.settings_draft.auto_clear,
                            "Auto-clear Completed jobs after 5 seconds",
                        )
                        .changed()
                    {
                        changed = true;
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Max Concurrent Encoding Jobs:");
                        if ui
                            .add(egui::Slider::new(
                                &mut self.settings_draft.max_concurrency,
                                1..=16,
                            ))
                            .changed()
                        {
                            changed = true;
                        }
                    });

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Output Overwrite Behavior:");
                        let prev = self.settings_draft.overwrite_mode;
                        egui::ComboBox::from_id_salt("overwrite_mode_combo")
                            .selected_text(self.settings_draft.overwrite_mode.as_str())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.settings_draft.overwrite_mode,
                                    OverwriteMode::Overwrite,
                                    OverwriteMode::Overwrite.as_str(),
                                );
                                ui.selectable_value(
                                    &mut self.settings_draft.overwrite_mode,
                                    OverwriteMode::Skip,
                                    OverwriteMode::Skip.as_str(),
                                );
                                ui.selectable_value(
                                    &mut self.settings_draft.overwrite_mode,
                                    OverwriteMode::Rename,
                                    OverwriteMode::Rename.as_str(),
                                );
                            });
                        if self.settings_draft.overwrite_mode != prev {
                            changed = true;
                        }
                    });

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Custom Output Directory:");
                        let text_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.custom_output_dir_buf)
                                .hint_text("(Default: save alongside source file)")
                                .desired_width(300.0),
                        );
                        if text_resp.changed() {
                            self.settings_draft.custom_output_dir =
                                normalize_opt(Some(self.custom_output_dir_buf.clone()));
                            changed = true;
                        }
                        if ui.button("Browse...").clicked() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                self.custom_output_dir_buf = dir.to_string_lossy().to_string();
                                self.settings_draft.custom_output_dir =
                                    Some(self.custom_output_dir_buf.clone());
                                changed = true;
                            }
                        }
                        if !self.custom_output_dir_buf.is_empty() && ui.button("Reset").clicked() {
                            self.custom_output_dir_buf.clear();
                            self.settings_draft.custom_output_dir = None;
                            changed = true;
                        }
                    });

                    if changed {
                        if let Err(e) = self.engine.update_settings(self.settings_draft.clone()) {
                            self.set_toast(format!("Failed saving settings: {}", e), true);
                        }
                    }

                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // Updates checker
                    let (upd_checking, upd_installing, upd_status, upd_release_url, upd_setup_url) = {
                        let g = self.update_state.lock().unwrap();
                        (
                            g.checking,
                            g.installing,
                            g.status.clone(),
                            g.release_url.clone(),
                            g.setup_url.clone(),
                        )
                    };
                    let busy = upd_checking || upd_installing;
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Check for Updates"))
                            .clicked()
                        {
                            let st = self.update_state.clone();
                            let ctx = ui.ctx().clone();
                            {
                                let mut g = st.lock().unwrap();
                                g.checking = true;
                                g.status = "Checking GitHub releases...".to_string();
                                g.release_url = None;
                                g.setup_url = None;
                            }
                            thread::spawn(move || {
                                let res = fetch_latest_github_release();
                                {
                                    let mut g = st.lock().unwrap();
                                    g.checking = false;
                                    match res {
                                        Ok(info) => {
                                            let clean_tag = info.tag.trim_start_matches('v');
                                            if clean_tag == APP_VERSION {
                                                g.status = format!(
                                                    "You are on the latest version (v{}).",
                                                    APP_VERSION
                                                );
                                            } else {
                                                g.status = format!("Release {} is available!", info.tag);
                                                g.release_url = Some(info.html_url);
                                                g.setup_url = info.setup_download_url;
                                            }
                                        }
                                        Err(e) => {
                                            g.status = format!("Update check failed: {}", e);
                                        }
                                    }
                                }
                                ctx.request_repaint();
                            });
                        }

                        if upd_status != "Idle" {
                            ui.label(&upd_status);
                        }
                        if let Some(ref dl_url) = upd_setup_url {
                            if ui
                                .add_enabled(!busy, egui::Button::new("Install Update Now"))
                                .clicked()
                            {
                                let st = self.update_state.clone();
                                let quit_flag = self.quit_requested.clone();
                                let ctx = ui.ctx().clone();
                                let dl = dl_url.clone();
                                {
                                    let mut g = st.lock().unwrap();
                                    g.installing = true;
                                    g.status = "Downloading & launching installer...".to_string();
                                }
                                thread::spawn(move || {
                                    let res = download_and_run_installer(&dl);
                                    {
                                        let mut g = st.lock().unwrap();
                                        g.installing = false;
                                        match res {
                                            Ok(()) => {
                                                g.status =
                                                    "Launching installer and restarting...".to_string();
                                                quit_flag.store(true, Ordering::SeqCst);
                                            }
                                            Err(e) => {
                                                g.status = format!("Install failed: {}", e);
                                            }
                                        }
                                    }
                                    ctx.request_repaint();
                                });
                            }
                        }
                        if let Some(ref url) = upd_release_url {
                            if ui.button("Open Release Page").clicked() {
                                open_with_default_app(url);
                            }
                        }
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Open Profiles Folder").clicked() {
                            if let Ok(dir) = get_profiles_dir() {
                                open_folder_in_explorer(&dir);
                            }
                        }
                        if ui.button("Open AppData Folder").clicked() {
                            let dir = get_app_data_dir();
                            open_folder_in_explorer(&dir);
                        }
                    });
                });
            });
    }
}

fn status_pill(ui: &mut egui::Ui, label: String, color: egui::Color32) {
    let bg = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 36);
    egui::Frame::none()
        .fill(bg)
        .stroke(egui::Stroke::new(1.0, color))
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::symmetric(8.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).small().strong().color(color));
        });
}

fn cli_cheatsheet() -> &'static str {
    r#"# Full AI Reference Manual:
ffmpeg-queue-manager agent-help

# 1. Discover saved profiles:
ffmpeg-queue-manager profiles list --json

# 2. Inspect media & preview resolved FFmpeg command:
ffmpeg-queue-manager probe "video.mp4" -p "Encode Clip Audio Merge AV1" --json

# 3. Queue into GUI daemon (non-blocking) or wait (-w):
ffmpeg-queue-manager queue add -p "Encode Clip Audio Merge AV1" -w --json "video.mp4"

# 4. Run custom FFmpeg args headlessly without a profile:
ffmpeg-queue-manager run --raw-args "-c:v libx264 -crf 23 -c:a aac" --ext .mp4 --json "video.mp4"

# 5. Monitor, tail logs, or control active queue:
ffmpeg-queue-manager queue list --json
ffmpeg-queue-manager queue logs job-1 -n 50 --json
ffmpeg-queue-manager queue stop job-1"#
}

fn decode_embedded_png(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let buf_size = reader.output_buffer_size()?;
    let mut buf = vec![0; buf_size];
    let info = reader.next_frame(&mut buf).ok()?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((info.width * info.height * 4) as usize);
            for chunk in buf[..info.buffer_size()].chunks_exact(3) {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        _ => return None,
    };
    Some((rgba, info.width, info.height))
}

fn setup_tray(
    signals: &GuiWakeSignals,
    quit_flag: &Arc<AtomicBool>,
    pause_flag: &Arc<AtomicBool>,
    ctx: egui::Context,
) -> Option<TrayIcon> {
    let icon_bytes = include_bytes!("../src-tauri/icons/32x32.png");
    let tray_image = decode_embedded_png(icon_bytes)
        .and_then(|(rgba, w, h)| TrayIconImage::from_rgba(rgba, w, h).ok());

    let menu = Menu::new();
    let show_item = MenuItem::with_id("show", "Show FFmpeg Queue Manager", true, None);
    let pause_item = MenuItem::with_id("toggle_pause", "Pause / Resume Queue", true, None);
    let quit_item = MenuItem::with_id("quit", "Quit", true, None);
    let _ = menu.append(&show_item);
    let _ = menu.append(&pause_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("FFmpeg Queue Manager");
    if let Some(img) = tray_image {
        builder = builder.with_icon(img);
    }

    let tray = builder.build().ok()?;

    // Menu click listener thread
    let sig_menu = signals.clone();
    let quit_menu = quit_flag.clone();
    let pause_menu = pause_flag.clone();
    let ctx_menu = ctx.clone();
    thread::spawn(move || {
        let rx = MenuEvent::receiver();
        while let Ok(ev) = rx.recv() {
            match ev.id.0.as_str() {
                "show" => {
                    sig_menu.wake_window();
                    ctx_menu.request_repaint();
                }
                "toggle_pause" => {
                    pause_menu.store(true, Ordering::SeqCst);
                    ctx_menu.request_repaint();
                }
                "quit" => {
                    quit_menu.store(true, Ordering::SeqCst);
                    sig_menu.wake_window();
                    ctx_menu.request_repaint();
                }
                _ => {}
            }
        }
    });

    // Left-click tray icon listener thread
    let sig_tray = signals.clone();
    thread::spawn(move || {
        let rx = TrayIconEvent::receiver();
        while let Ok(ev) = rx.recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                sig_tray.wake_window();
                ctx.request_repaint();
            }
        }
    });

    Some(tray)
}

pub fn run_gui(launch: GuiLaunchPlan) -> Result<(), String> {
    let icon_bytes = include_bytes!("../src-tauri/icons/128x128.png");
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("FFmpeg Queue Manager")
        .with_inner_size([1080.0, 720.0])
        .with_min_inner_size([780.0, 520.0])
        .with_visible(!launch.minimized);

    if let Some((rgba, width, height)) = decode_embedded_png(icon_bytes) {
        viewport = viewport.with_icon(Arc::new(egui::IconData {
            rgba,
            width,
            height,
        }));
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "FFmpeg Queue Manager",
        options,
        Box::new(move |cc| Ok(Box::new(QueueManagerApp::new(cc, launch)))),
    )
    .map_err(|e| format!("GUI error: {}", e))
}
