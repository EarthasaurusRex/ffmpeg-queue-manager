#![windows_subsystem = "windows"]

mod cli;
mod engine;
mod gui;
mod ipc;
mod models;
mod paths;
mod probe;
mod profiles;

use clap::Parser;
use cli::{attach_parent_console, detect_gui_only_launch, run_cli, Cli, GuiLaunchPlan};
use ipc::{is_daemon_running, try_send, IpcRequest};
use models::{EnqueueSpec, JobPriority};
use profiles::{collect_media_files, get_profile_by_name};
use std::env;
use std::process;

fn main() {
    let raw_args: Vec<String> = env::args().collect();

    if let Some(plan) = detect_gui_only_launch(&raw_args) {
        if is_daemon_running() {
            if let Some(ref prof_arg) = plan.enqueue_profile {
                let resolved_name = get_profile_by_name(prof_arg)
                    .map(|p| p.profile.name)
                    .unwrap_or_else(|_| prof_arg.trim_end_matches(".json").to_string());
                let files = collect_media_files(&plan.enqueue_files, false);
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
                    let _ = try_send(&IpcRequest::Enqueue { specs });
                }
            }
            if !plan.minimized {
                let _ = try_send(&IpcRequest::ShowWindow);
            }
            return;
        }

        if let Err(e) = gui::run_gui(plan) {
            attach_parent_console();
            eprintln!("Fatal GUI error: {}", e);
            process::exit(1);
        }
        return;
    }

    // CLI mode: attach to parent console only if stdout/stderr aren't already piped
    attach_parent_console();

    let cli = Cli::parse();
    if cli.gui && cli.command.is_none() && cli.profile.is_none() {
        let plan = GuiLaunchPlan {
            minimized: cli.minimized,
            enqueue_profile: None,
            enqueue_files: Vec::new(),
        };
        if let Err(e) = gui::run_gui(plan) {
            eprintln!("Fatal GUI error: {}", e);
            process::exit(1);
        }
        return;
    }

    let exit_code = run_cli(cli);
    process::exit(exit_code);
}
