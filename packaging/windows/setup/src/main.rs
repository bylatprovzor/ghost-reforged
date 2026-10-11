#![windows_subsystem = "windows"]

use std::{ffi::OsString, os::windows::process::CommandExt, path::PathBuf, process::Command};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO, MessageBoxW,
};

const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));
const EXTRACT: &str = include_str!("../extract.ps1");

#[derive(Default)]
struct Options {
    silent: bool,
    no_shortcut: bool,
    no_launch: bool,
    directory: Option<PathBuf>,
}

fn options(args: impl IntoIterator<Item = OsString>) -> Result<Options, String> {
    let mut result = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--silent") => result.silent = true,
            Some("--no-shortcut") => result.no_shortcut = true,
            Some("--no-launch") => result.no_launch = true,
            Some("--install-dir") => {
                result.directory = Some(PathBuf::from(
                    args.next().ok_or("Missing installation directory.")?,
                ))
            }
            _ => return Err("Unknown installer option.".into()),
        }
    }
    Ok(result)
}

fn dialog(message: &str, flags: u32) -> i32 {
    let title: Vec<u16> = "GHost Reforged Setup\0".encode_utf16().collect();
    let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags) }
}

fn install(options: &Options) -> Result<PathBuf, String> {
    let directory = options
        .directory
        .clone()
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(|root| PathBuf::from(root).join("Programs/GHost Reforged"))
        })
        .ok_or("The Windows application directory is unavailable.")?;
    let powershell = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or("Windows PowerShell is unavailable.")?
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let staging = tempfile::Builder::new()
        .prefix("ghost-setup-")
        .tempdir()
        .map_err(|_| "The installer could not create temporary files.")?;
    let archive = staging.path().join("package.zip");
    let script = staging.path().join("extract.ps1");
    std::fs::write(&archive, PAYLOAD)
        .and_then(|_| std::fs::write(&script, EXTRACT))
        .map_err(|_| "The installer could not extract its files.")?;
    let mut command = Command::new(powershell);
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .arg("-Archive")
        .arg(archive)
        .arg("-InstallDirectory")
        .arg(&directory)
        .creation_flags(0x08000000);
    if options.no_shortcut {
        command.arg("-NoShortcut");
    }
    let output = command
        .output()
        .map_err(|_| "Windows could not start installation.")?;
    if !output.status.success() {
        return Err("Installation could not finish. Close GHost Reforged and try again, or run the portable executable.".into());
    }
    Ok(directory.join("ghost-reforged.exe"))
}

fn main() {
    let options = match options(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            dialog(&error, MB_OK | MB_ICONERROR);
            std::process::exit(2);
        }
    };
    if !options.silent
        && dialog(
            "Install GHost Reforged for your Windows account?",
            MB_YESNO | MB_ICONQUESTION,
        ) != IDYES
    {
        std::process::exit(3);
    }
    match install(&options) {
        Ok(executable) => {
            if !options.silent {
                dialog("GHost Reforged is installed.", MB_OK | MB_ICONINFORMATION);
                if !options.no_launch {
                    let _ = Command::new(executable).spawn();
                }
            }
        }
        Err(error) => {
            if !options.silent {
                dialog(&error, MB_OK | MB_ICONERROR);
            }
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_remain_single_arguments_and_unknown_flags_are_rejected() {
        let parsed = options(
            [
                "--silent",
                "--install-dir",
                "C:/Apps/Example Name",
                "--no-shortcut",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(
            parsed.directory,
            Some(PathBuf::from("C:/Apps/Example Name"))
        );
        assert!(parsed.silent && parsed.no_shortcut);
        assert!(options(["--install-dir"].map(OsString::from)).is_err());
        assert!(options(["--unknown"].map(OsString::from)).is_err());
    }
}
