# GHost Reforged

<img width="1710" height="1230" alt="image" src="https://github.com/user-attachments/assets/e330961d-d3ec-4e68-9d1c-1814ac907147" />

A Warcraft III auto-host for Windows, written in Rust.

Download [ghost-reforged.exe](https://github.com/shideneyu/ghost-reforged/releases/download/v0.2.1/ghost-reforged.exe) and run it. No installation required.

For a Start menu shortcut, run the [installer](https://github.com/shideneyu/ghost-reforged/releases/download/v0.2.1/GHost-Reforged-Setup-x64.exe), or paste this into PowerShell:

```powershell
irm https://github.com/shideneyu/ghost-reforged/releases/download/v0.2.1/install.ps1 | iex
```

Sign in to Battle.net, select a map and click **Host**. Windows 10/11, 64-bit. No administrator rights required.

Auto start defaults to **Full**. Choose a minimum player count or use **Force start**. Players can use **?votestart** and **?votekick name**. Edit commands and the welcome message in **Options**, including while hosting. **Stop Hosting** closes the current lobby and stops new ones. Save presets in **Profiles**.

Closing the window keeps hosting in the system tray. Click the ghost to reopen it, or right-click and choose **Quit** to exit.

Early test build for Warcraft III **3.0.0.24268**. The game runs in the background and the bot uses an observer slot. The host appears with a blank name in the lobby, including for joining players.

Build from source: `powershell -File packaging/windows/build.ps1 -Setup` (Rust and the Visual Studio C++ build tools required).
