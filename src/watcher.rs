use crate::{State, display};
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use wmi::WMIConnection;

#[derive(Deserialize)]
#[serde(rename = "Win32_ProcessStartTrace")]
struct Start {
    #[serde(rename = "ProcessID")]
    pid: u32,
    #[serde(rename = "ProcessName")]
    name: String,
}
#[derive(Deserialize)]
struct Process {
    #[serde(rename = "ProcessId")]
    pid: u32,
    #[serde(rename = "Name")]
    name: String,
}
#[derive(Deserialize)]
#[serde(rename = "Win32_ProcessStopTrace")]
struct Stop {
    #[serde(rename = "ProcessID")]
    pid: u32,
}
fn configured(state: &State) -> Vec<PathBuf> {
    state.config.games.iter().map(PathBuf::from).collect()
}
fn relevant(name: &str, games: &[PathBuf]) -> bool {
    games.iter().any(|p| {
        p.file_name()
            .and_then(|x| x.to_str())
            .map(|x| x.eq_ignore_ascii_case(name))
            .unwrap_or(false)
    })
}
fn same_path(a: &PathBuf, b: &PathBuf) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

fn process_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        if ok {
            Some(PathBuf::from(String::from_utf16_lossy(
                &buf[..len as usize],
            )))
        } else {
            None
        }
    }
}
fn change_mode(shared: &Arc<Mutex<State>>, on: bool) {
    let result = if on {
        display::set_hdr(true).and_then(|_| {
            thread::sleep(Duration::from_millis(350));
            display::set_console_mode(true)
        })
    } else {
        display::set_console_mode(false).and_then(|_| {
            thread::sleep(Duration::from_millis(100));
            display::set_hdr(false)
        })
    };
    let mut s = shared.lock().unwrap();
    s.mode_on = on;
    s.status = match result {
        Ok(()) => if on {
            "Game detected. HDR and Console Mode enabled."
        } else {
            "No configured games running."
        }
        .into(),
        Err(e) => format!("Display change failed: {e}"),
    };
}
fn add_process(shared: &Arc<Mutex<State>>, pid: u32, path: PathBuf) {
    let transition = {
        let mut s = shared.lock().unwrap();
        if s.active.contains_key(&pid) {
            false
        } else {
            let was_empty = s.active.is_empty();
            s.active.insert(pid, path.clone());
            s.status = format!(
                "{} detected",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            was_empty
        }
    };
    if transition {
        change_mode(shared, true);
    }
}
pub fn resync(shared: &Arc<Mutex<State>>, ctx: &eframe::egui::Context) {
    let snapshot = {
        let s = shared.lock().unwrap();
        (configured(&s), !s.watcher_started)
    };
    if snapshot.0.is_empty() {
        return;
    }
    if snapshot.1 {
        let mut s = shared.lock().unwrap();
        s.watcher_started = true;
        drop(s);
        spawn(shared.clone(), ctx.clone());
    }
    let shared = shared.clone();
    thread::spawn(move || {
        if let Ok(wmi) = WMIConnection::new() {
            if let Ok(rows) = wmi.raw_query::<Process>("SELECT ProcessId, Name FROM Win32_Process")
            {
                for p in rows {
                    if relevant(&p.name, &snapshot.0) {
                        if let Some(path) = process_path(p.pid) {
                            if snapshot.0.iter().any(|x| same_path(x, &path)) {
                                add_process(&shared, p.pid, path);
                            }
                        }
                    }
                }
            }
        }
    });
}
fn spawn(shared: Arc<Mutex<State>>, ctx: eframe::egui::Context) {
    let for_start = shared.clone();
    thread::spawn(move || event_start(for_start, ctx));
    thread::spawn(move || event_stop(shared));
}

fn event_start(shared: Arc<Mutex<State>>, ctx: eframe::egui::Context) {
    let Ok(wmi) = WMIConnection::new() else {
        return;
    };
    let Ok(events) = wmi.notification::<Start>() else {
        return;
    };

    for result in events {
        let event = match result {
            Ok(event) => event,
            Err(_) => {
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        let games = {
            let s = shared.lock().unwrap();
            configured(&s)
        };

        if relevant(&event.name, &games) {
            if let Some(path) = process_path(event.pid) {
                if games.iter().any(|x| same_path(x, &path)) {
                    add_process(&shared, event.pid, path);
                    ctx.request_repaint();
                }
            }
        }
    }
}

fn event_stop(shared: Arc<Mutex<State>>) {
    let Ok(wmi) = WMIConnection::new() else {
        return;
    };
    let Ok(events) = wmi.notification::<Stop>() else {
        return;
    };

    for result in events {
        let event = match result {
            Ok(event) => event,
            Err(_) => {
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        let transition = {
            let mut s = shared.lock().unwrap();
            s.active.remove(&event.pid).is_some() && s.active.is_empty()
        };

        if transition {
            change_mode(&shared, false);
        }
    }
}
