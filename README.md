# GHost Reforged

<img width="1710" height="1230" alt="image" src="https://github.com/user-attachments/assets/e330961d-d3ec-4e68-9d1c-1814ac907147" />

A Warcraft III auto-host for Windows, written in Rust.

1. Download the Windows ZIP from [Releases](https://github.com/shideneyu/ghost-reforged/releases) and extract it.
2. Run **Install.cmd**, then open **GHost Reforged** from the Start menu. Or run **ghost-reforged.exe** directly.
3. Sign in to Battle.net, select a map and click **Host**.

Auto start defaults to **Full**. Choose a minimum player count or use **Force start**. Players can use **?votestart** and **?votekick name**. Edit commands and the welcome message in **Options**, including while hosting. **Stop Hosting** closes the current lobby and stops new ones. Save presets in **Profiles**.

Early test build for Warcraft III **3.0.0.24268**. The game runs in the background and the bot uses an observer slot. Host-name masking on other clients is still being tested.

Build from source: `powershell -File packaging/windows/build.ps1` (Rust and the Visual Studio C++ build tools required).
