use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use tauri::AppHandle;

pub fn run_if_requested() -> bool {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(OsStr::new("--apply-update")) {
        return false;
    }
    let result = apply_arguments(arguments);
    if let Err(error) = result {
        show_update_error(&error);
        std::process::exit(1);
    }
    true
}

fn apply_arguments(mut arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<(), String> {
    let target = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "the update helper did not receive the browser path".to_string())?;
    let staged = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "the update helper did not receive the staged update path".to_string())?;
    if arguments.next().is_some() {
        return Err("the update helper received unexpected arguments".to_string());
    }
    apply_update(&target, &staged)
}

fn apply_update(target: &Path, staged: &Path) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "the browser executable has no parent directory".to_string())?;
    if staged.parent() != Some(parent) || !staged.is_file() {
        return Err("the staged update is missing or is outside the browser folder".to_string());
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match std::fs::rename(staged, target) {
            Ok(()) => break,
            Err(error)
                if Instant::now() < deadline
                    && (error.kind() == std::io::ErrorKind::PermissionDenied
                        || error.kind() == std::io::ErrorKind::WouldBlock) =>
            {
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => {
                return Err(format!(
                    "could not replace the browser executable. Close other Eefi Browser windows and make sure this folder is writable: {error}"
                ));
            }
        }
    }
    Command::new(target)
        .current_dir(parent)
        .spawn()
        .map_err(|error| {
            format!("the update was installed, but the browser could not restart: {error}")
        })?;
    Ok(())
}

#[cfg(windows)]
pub async fn launch_helper(app: &AppHandle, target: &Path, staged: &Path) -> Result<(), String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::fs;

    let current_exe = std::env::current_exe()
        .map_err(|error| format!("could not locate the updater helper executable: {error}"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is invalid: {error}"))?
        .as_millis();
    let helper = std::env::temp_dir().join(format!(
        "eefi-browser-updater-{}-{nonce}.exe",
        std::process::id()
    ));
    fs::copy(current_exe, &helper)
        .await
        .map_err(|error| format!("could not prepare the update helper: {error}"))?;

    let mut command = Command::new(&helper);
    command.arg("--apply-update").arg(target).arg(staged);
    if let Err(error) = command.spawn() {
        if let Err(cleanup_error) = fs::remove_file(&helper).await {
            eprintln!("could not remove unused update helper: {cleanup_error}");
        }
        return Err(format!("could not start the update helper: {error}"));
    }
    app.exit(0);
    Ok(())
}

#[cfg(not(windows))]
pub async fn launch_helper(_app: &AppHandle, _target: &Path, _staged: &Path) -> Result<(), String> {
    Err("automatic executable replacement is currently supported only on Windows.".to_string())
}

#[cfg(windows)]
fn show_update_error(error: &str) {
    use windows::{
        core::HSTRING,
        Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
    };
    let title = HSTRING::from("Eefi Browser update failed");
    let message = HSTRING::from(error);
    unsafe {
        let _ = MessageBoxW(None, &message, &title, MB_ICONERROR | MB_OK);
    }
}

#[cfg(not(windows))]
fn show_update_error(error: &str) {
    eprintln!("{error}");
}
