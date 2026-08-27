#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod display;
mod watcher;

use anyhow::{Context, Result};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::os::windows::ffi::OsStrExt;
use std::{
    collections::HashMap,
    env, fs,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
use windows::Win32::System::Com::STGM_READ;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SW_RESTORE, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow,
};
use windows::core::Interface;
use windows::core::PCWSTR;
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

#[derive(Clone, Serialize, Deserialize, Default)]
struct Config {
    games: Vec<String>,
    start_with_windows: bool,
    #[serde(default)]
    minimize_to_tray: bool,
}

struct State {
    config: Config,
    active: HashMap<u32, PathBuf>,
    mode_on: bool,
    status: String,
    watcher_started: bool,
}

fn config_path() -> Result<PathBuf> {
    Ok(
        PathBuf::from(env::var_os("APPDATA").context("APPDATA is unavailable")?)
            .join("HdrGameSwitcher")
            .join("config.json"),
    )
}

fn load_config() -> Config {
    config_path()
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(config: &Config) -> Result<()> {
    let path = config_path()?;
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, serde_json::to_vec_pretty(config)?)?;
    Ok(())
}

fn resolve_shortcut(path: &PathBuf) -> Result<PathBuf> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }?;
    let persist: IPersistFile = link.cast()?;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        persist.Load(PCWSTR(wide.as_ptr()), STGM_READ)?;
    }
    let mut target = vec![0u16; 32768];
    unsafe {
        link.GetPath(&mut target, std::ptr::null_mut(), 0)?;
    }
    let end = target.iter().position(|&c| c == 0).unwrap_or(target.len());
    if end == 0 {
        anyhow::bail!("shortcut has no target")
    }
    Ok(PathBuf::from(String::from_utf16(&target[..end])?))
}

fn set_startup(enabled: bool, _minimized: bool) -> Result<()> {
    let task_name = "HDR Game Switcher";
    let _ = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            winreg::enums::KEY_SET_VALUE,
        )
        .and_then(|key| key.delete_value("HdrGameSwitcher"));
    if enabled {
        let exe = env::current_exe()?;
        let suffix = " --minimized";
        let action = format!("\"{}\"{suffix}", exe.display());
        let output = Command::new("schtasks.exe")
            .args([
                "/Create", "/TN", task_name, "/SC", "ONLOGON", "/TR", &action, "/RL", "HIGHEST",
                "/IT", "/F",
            ])
            .output()?;
        if !output.status.success() {
            anyhow::bail!(
                "Task Scheduler failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    } else {
        let _ = Command::new("schtasks.exe")
            .args(["/Delete", "/TN", task_name, "/F"])
            .output();
    }
    Ok(())
}

fn ensure_elevated() -> Result<()> {
    if unsafe { IsUserAnAdmin().as_bool() } {
        return Ok(());
    }
    let exe = env::current_exe()?;
    let args: Vec<u16> = env::args_os()
        .skip(1)
        .flat_map(|arg| {
            let mut part: Vec<u16> = arg.encode_wide().collect();
            part.push(b' ' as u16);
            part
        })
        .chain(std::iter::once(0))
        .collect();
    let exe_wide: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let runas: Vec<u16> = "runas".encode_utf16().chain(std::iter::once(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::PCWSTR(runas.as_ptr()),
            windows::core::PCWSTR(exe_wide.as_ptr()),
            windows::core::PCWSTR(args.as_ptr()),
            None,
            SW_SHOWNORMAL,
        )
    };
    if result.0 as usize <= 32 {
        anyhow::bail!("administrator permission was not granted")
    }
    std::process::exit(0);
}

fn show_main_window() {
    let title: Vec<u16> = "HDR Game Switcher"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    if let Ok(hwnd) = unsafe { FindWindowW(None, PCWSTR(title.as_ptr())) } {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

struct App {
    shared: Arc<Mutex<State>>,
    egui_ctx: egui::Context,
    _tray: Option<TrayIcon>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Mutex<State>>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        watcher::resync(&shared, &cc.egui_ctx);
        let tray = make_tray(&cc.egui_ctx);
        Self {
            shared,
            egui_ctx: cc.egui_ctx.clone(),
            _tray: tray,
        }
    }
}

fn make_tray(ctx: &egui::Context) -> Option<TrayIcon> {
    let mut rgba = vec![0u8; 16 * 16 * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[40, 180, 210, 255]);
    }
    let icon = Icon::from_rgba(rgba, 16, 16).ok()?;
    let menu = Menu::new();
    let show = MenuItem::new("Show", true, None);
    let exit = MenuItem::new("Exit", true, None);
    menu.append(&show).ok()?;
    menu.append(&exit).ok()?;
    let show_id = show.id().clone();
    let exit_id = exit.id().clone();
    let event_ctx = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if matches!(event, TrayIconEvent::Click { .. }) {
            show_main_window();
            event_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            event_ctx.request_repaint();
        }
    }));
    let menu_ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == show_id {
            show_main_window();
            menu_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            menu_ctx.request_repaint();
        } else if event.id == exit_id {
            std::process::exit(0);
        }
    }));
    TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_tooltip("HDR Game Switcher")
        .build()
        .ok()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        let mut state = self.shared.lock().unwrap();
        if close_requested && state.config.minimize_to_tray {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        let mut request_resync = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("HDR Game Switcher");
            ui.separator();
            ui.label("Configured games");
            if state.config.games.is_empty() {
                ui.label("No games configured.");
            }
            let mut remove = None;
            for (i, game) in state.config.games.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(
                        PathBuf::from(game)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(game),
                    );
                    if ui.button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
                ui.small(game);
            }
            if let Some(i) = remove {
                state.config.games.remove(i);
                if let Err(e) = save_config(&state.config) {
                    state.status = e.to_string();
                }
            }
            if ui.button("Add game / shortcut").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    let result = if path
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.eq_ignore_ascii_case("lnk"))
                        .unwrap_or(false)
                    {
                        resolve_shortcut(&path)
                    } else {
                        Ok(path)
                    };
                    match result {
                        Ok(path)
                            if path
                                .extension()
                                .and_then(|x| x.to_str())
                                .map(|x| x.eq_ignore_ascii_case("exe"))
                                .unwrap_or(false) =>
                        {
                            let value = path.to_string_lossy().to_string();
                            if !state
                                .config
                                .games
                                .iter()
                                .any(|x| x.eq_ignore_ascii_case(&value))
                            {
                                state.config.games.push(value);
                            }
                            if let Err(e) = save_config(&state.config) {
                                state.status = e.to_string();
                            }
                            request_resync = true;
                        }
                        Ok(_) => state.status = "Shortcut target is not an .exe".into(),
                        Err(e) => state.status = format!("Could not resolve shortcut: {e}"),
                    }
                }
            }
            let mut startup = state.config.start_with_windows;
            if ui.checkbox(&mut startup, "Start with Windows").changed() {
                match set_startup(startup, state.config.minimize_to_tray) {
                    Ok(()) => {
                        state.config.start_with_windows = startup;
                        let _ = save_config(&state.config);
                    }
                    Err(e) => state.status = format!("Startup setting failed: {e}"),
                }
            }
            if ui
                .checkbox(&mut state.config.minimize_to_tray, "Minimize to tray")
                .changed()
            {
                let _ = save_config(&state.config);
                if state.config.start_with_windows {
                    let _ = set_startup(true, state.config.minimize_to_tray);
                }
            }
            ui.separator();
            ui.label(format!(
                "Game mode: {}",
                if state.mode_on {
                    "HDR + Console Mode ON"
                } else {
                    "HDR + Console Mode OFF"
                }
            ));
            ui.label(format!("Active games: {}", state.active.len()));
            ui.label(format!("Status: {}", state.status));
        });
        drop(state);
        if request_resync {
            watcher::resync(&self.shared, &self.egui_ctx);
        }
    }
}

fn main() -> Result<()> {
    ensure_elevated()?;
    let config = load_config();
    let minimize_to_tray = config.minimize_to_tray;
    if config.start_with_windows {
        let _ = set_startup(true, minimize_to_tray);
    }
    let shared = Arc::new(Mutex::new(State {
        config,
        active: HashMap::new(),
        mode_on: false,
        status: "Waiting for configured games".into(),
        watcher_started: false,
    }));
    let startup_hidden = env::args().any(|arg| arg == "--minimized");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("HDR Game Switcher")
            .with_inner_size([520.0, 420.0])
            .with_visible(!startup_hidden),
        ..Default::default()
    };
    eframe::run_native(
        "HDR Game Switcher",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc, shared.clone())))),
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}
