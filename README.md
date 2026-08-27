# HDR Game Switcher

HDR Game Switcher is a small Windows-only utility that automatically enables HDR and Console Mode when a configured game is running, then disables them after the last configured game closes.

It uses Windows process-start/process-stop events rather than periodically polling the process list.

> **Important:** Console Mode control currently works only with the **Alienware AW2725D**. It is not supported on other monitors or displays. On other hardware, the application may still toggle Windows HDR, but Console Mode will not work.

## Download and first launch

If you only want to use the application, you do not need Rust or GitHub installed:

1. Open the repository's **Releases** page.
2. Open the newest release, for example `v1.0.0`.
3. In the **Assets** section, download `hdr-game-switcher-windows-x64.zip`.
4. Extract the ZIP file to a folder of your choice.
5. Run `hdr-game-switcher.exe`.

Do not download **Source code (zip)** or **Source code (tar.gz)**. Those files are for developers and do not contain the ready-to-use application.

Windows may show a SmartScreen warning because the executable is not code-signed. If Windows asks for administrator permission, approve it; the application needs elevation for reliable process detection and display control.

## Requirements

- Windows 11
- An HDR-capable display
- Alienware AW2725D for Console Mode switching
- Rust stable with the MSVC Windows toolchain, if building from source

The application automatically requests administrator permission when launched. This is required for reliable process detection and display control.

## Using the application

1. Launch `hdr-game-switcher.exe`.
2. Select **Add game / shortcut**.
3. Choose a `.exe` file or a `.lnk` shortcut.
4. For a shortcut, the target executable is resolved and stored; the shortcut itself is not retained.
5. Use **Remove** to delete a configured game.

When a configured executable starts, the application:

1. Enables Windows HDR.
2. Waits briefly.
3. Sets AW2725D Console Mode to ON.

When the last configured game closes, it turns Console Mode OFF and then disables HDR. Multiple configured games are handled together: display changes happen only when the active-game count changes between zero and non-zero.

## Startup and tray behavior

Enable **Start with Windows** to create a per-user Windows Scheduled Task running at the highest available privilege. No Windows service or administrator installation is required.

The startup task always launches the application hidden. Use the tray icon's **Show** action to open the window.

When **Minimize to tray** is enabled, closing the window hides it in the tray instead of exiting. The tray menu also provides **Show** and **Exit** actions.

Uncheck **Start with Windows** to remove the scheduled task.

## Configuration

Configuration is stored as JSON at:

```text
%APPDATA%\HdrGameSwitcher\config.json
```

The file contains configured executable paths and the startup/tray settings. If it does not exist, the application starts with an empty configuration.

## Display limitations

HDR is controlled through Windows DisplayConfig APIs.

Console Mode uses DDC/CI VCP code `F4` with the verified AW2725D values:

- ON: `113` (`0x71`)
- OFF: `112` (`0x70`)

The application checks for `AW2725D` before sending the manufacturer-specific command. It is not a general monitor-control utility. On other monitors, HDR may still be changed, but Console Mode may report that the AW2725D was not found.

## Building

From the repository root:

```powershell
cargo build --release
```

The executable is created at:

```text
target\release\hdr-game-switcher.exe
```

There is no installer; copy the release executable wherever it is needed.
