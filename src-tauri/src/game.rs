#[cfg(windows)]
#[tauri::command]
pub fn is_wow_running() -> bool {
    use std::mem::size_of;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    let Ok(snapshot) = snapshot else {
        return false;
    };

    let mut entry = PROCESSENTRY32W::default();
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut has_process = unsafe { Process32FirstW(snapshot, &mut entry).is_ok() };
    let mut found = false;

    while has_process {
        let end = entry
            .szExeFile
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(entry.szExeFile.len());
        let executable = String::from_utf16_lossy(&entry.szExeFile[..end]);
        if executable.eq_ignore_ascii_case("run.exe") {
            found = true;
            break;
        }
        has_process = unsafe { Process32NextW(snapshot, &mut entry).is_ok() };
    }

    unsafe {
        let _ = CloseHandle(snapshot);
    }
    found
}

#[cfg(not(windows))]
#[tauri::command]
pub fn is_wow_running() -> bool {
    false
}
