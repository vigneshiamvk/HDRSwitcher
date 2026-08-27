use anyhow::{Result, bail};
use std::mem::size_of;
use std::{thread, time::Duration};
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, POINT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint,
};
use windows::core::BOOL;

pub fn set_hdr(enabled: bool) -> Result<()> {
    unsafe {
        let mut paths = 0;
        let mut modes = 0;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes)
            != ERROR_SUCCESS
        {
            bail!("could not query display configuration")
        }
        let mut path_data = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
        let mut mode_data = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        let result = QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut paths,
            path_data.as_mut_ptr(),
            &mut modes,
            mode_data.as_mut_ptr(),
            None,
        );
        if result != ERROR_SUCCESS {
            bail!("could not read display configuration ({})", result.0)
        }
        for path in &path_data[..paths as usize] {
            let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                    size: size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
                    adapterId: path.targetInfo.adapterId,
                    id: path.targetInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut info.header) != 0 {
                continue;
            }
            if (info.Anonymous.value & 1) == 0 {
                continue;
            }
            let mut packet = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
                    size: size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32,
                    adapterId: path.targetInfo.adapterId,
                    id: path.targetInfo.id,
                },
                ..Default::default()
            };
            packet.Anonymous.value = if enabled { 1 } else { 0 };
            if DisplayConfigSetDeviceInfo(&packet.header) != 0 {
                bail!("could not change HDR state")
            }
        }
    }
    Ok(())
}

unsafe extern "system" fn monitor_callback(
    monitor: HMONITOR,
    _: windows::Win32::Graphics::Gdi::HDC,
    _: *mut windows::Win32::Foundation::RECT,
    data: LPARAM,
) -> BOOL {
    unsafe {
        (*(data.0 as *mut Vec<HMONITOR>)).push(monitor);
    }
    BOOL(1)
}

pub fn set_console_mode(enabled: bool) -> Result<()> {
    let mut monitors: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_callback),
            LPARAM(&mut monitors as *mut _ as isize),
        );
    }
    unsafe {
        let primary = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        if !primary.0.is_null() {
            if let Some(index) = monitors.iter().position(|monitor| monitor.0 == primary.0) {
                let monitor = monitors.remove(index);
                monitors.insert(0, monitor);
            }
        }
        monitors.truncate(1);
    }
    for monitor in monitors {
        unsafe {
            let mut count = 0;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count).is_err() {
                continue;
            }
            let mut physical = vec![PHYSICAL_MONITOR::default(); count as usize];
            if GetPhysicalMonitorsFromHMONITOR(monitor, &mut physical).is_err() {
                continue;
            }
            let mut selected = None;
            let mut fallback = None;
            for (index, item) in physical.iter().enumerate() {
                let description_chars =
                    std::ptr::read_unaligned(std::ptr::addr_of!(item.szPhysicalMonitorDescription));
                let end = description_chars
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(128);
                let description = String::from_utf16_lossy(&description_chars[..end]);
                let mut ty = MC_VCP_CODE_TYPE(0);
                let mut current = 0;
                let mut maximum = 0;
                let readable = GetVCPFeatureAndVCPFeatureReply(
                    item.hPhysicalMonitor,
                    0xF4,
                    Some(&mut ty),
                    &mut current,
                    Some(&mut maximum),
                ) != 0;
                let is_aw2725d = description.to_ascii_uppercase().contains("AW2725D");
                let compatible_fallback = readable && (current == 112 || current == 113);
                if is_aw2725d {
                    selected = Some(index);
                    break;
                }
                if compatible_fallback && fallback.is_none() {
                    fallback = Some(index);
                }
            }
            if selected.is_none() {
                selected = fallback;
            }
            if let Some(index) = selected {
                let item = &physical[index];
                let desired = if enabled { 113 } else { 112 };
                let mut verified_value = None;
                for attempt in 0..3 {
                    if SetVCPFeature(item.hPhysicalMonitor, 0xF4, desired) == 0 {
                        DestroyPhysicalMonitors(&physical)?;
                        bail!("could not set AW2725D Console Mode")
                    }
                    let _ = SaveCurrentSettings(item.hPhysicalMonitor);
                    thread::sleep(Duration::from_millis(150));
                    let mut verify_type = MC_VCP_CODE_TYPE(0);
                    let mut verify_value = 0;
                    let mut verify_maximum = 0;
                    if GetVCPFeatureAndVCPFeatureReply(
                        item.hPhysicalMonitor,
                        0xF4,
                        Some(&mut verify_type),
                        &mut verify_value,
                        Some(&mut verify_maximum),
                    ) != 0
                    {
                        verified_value = Some(verify_value);
                        break;
                    }
                    if attempt < 2 {
                        thread::sleep(Duration::from_millis(150));
                    }
                }
                let Some(verify_value) = verified_value else {
                    DestroyPhysicalMonitors(&physical)?;
                    bail!("VCP F4 write succeeded, but read-back failed")
                };
                if verify_value != desired {
                    DestroyPhysicalMonitors(&physical)?;
                    bail!(
                        "AW2725D rejected Console Mode value {desired} (read back {verify_value})"
                    )
                }
                DestroyPhysicalMonitors(&physical)?;
                return Ok(());
            }
            DestroyPhysicalMonitors(&physical)?;
        }
    }
    bail!("AW2725D was not found through DDC/CI")
}
