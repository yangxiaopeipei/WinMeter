#[cfg(windows)]
mod imp {
    use windows::{core::PCWSTR, Win32::{Foundation::ERROR_FILE_NOT_FOUND, System::Registry::*}};
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
    struct Key(HKEY);
    impl Drop for Key { fn drop(&mut self) { unsafe { let _ = RegCloseKey(self.0); } } }

    pub fn enabled() -> bool {
        unsafe {
            let mut size = 0;
            RegGetValueW(HKEY_CURRENT_USER,PCWSTR(wide(RUN_KEY).as_ptr()),PCWSTR(wide("WinMeter").as_ptr()),
                RRF_RT_REG_SZ,None,None,Some(&mut size)).is_ok() && size > 2
        }
    }
    pub fn set(enabled: bool) -> Result<(), String> {
        unsafe {
            let mut key = HKEY::default();
            if !enabled {
                let result = RegOpenKeyExW(HKEY_CURRENT_USER,PCWSTR(wide(RUN_KEY).as_ptr()),0,KEY_SET_VALUE,&mut key);
                if result == ERROR_FILE_NOT_FOUND { return Ok(()); }
                result.ok().map_err(|e|e.to_string())?;
                let key=Key(key);
                let result=RegDeleteValueW(key.0,PCWSTR(wide("WinMeter").as_ptr()));
                return if result == ERROR_FILE_NOT_FOUND {Ok(())} else {result.ok().map_err(|e|e.to_string())};
            }
            let executable=std::env::current_exe().map_err(|e|e.to_string())?;
            let command=super::startup_command(&executable)?;
            RegCreateKeyExW(HKEY_CURRENT_USER,PCWSTR(wide(RUN_KEY).as_ptr()),0,PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,KEY_SET_VALUE,None,&mut key,None).ok().map_err(|e|e.to_string())?;
            let key=Key(key);
            let data:Vec<u8>=wide(&command).iter().flat_map(|c|c.to_le_bytes()).collect();
            RegSetValueExW(key.0,PCWSTR(wide("WinMeter").as_ptr()),0,REG_SZ,Some(&data)).ok().map_err(|e|e.to_string())
        }
    }
}

fn startup_command(executable: &std::path::Path) -> Result<String, String> {
    let path=executable.to_str().ok_or("程序路径不是有效 Unicode")?;
    if path.contains('"') || path.contains('\0') {return Err("程序路径无效".into());}
    Ok(format!("\"{}\" --tray",path))
}

#[cfg(windows)]
pub use imp::{enabled, set};
#[cfg(not(windows))]
pub fn enabled() -> bool {false}
#[cfg(not(windows))]
pub fn set(enabled: bool) -> Result<(),String> {if enabled {Err("开机自启仅支持 Windows".into())} else {Ok(())}}

#[cfg(test)]
mod tests {
    #[test]
    fn startup_paths_with_spaces_are_quoted_and_start_hidden() {
        assert_eq!(super::startup_command(std::path::Path::new(r"C:\Program Files\WinMeter\winmeter.exe")).unwrap(),
            "\"C:\\Program Files\\WinMeter\\winmeter.exe\" --tray");
    }
}
