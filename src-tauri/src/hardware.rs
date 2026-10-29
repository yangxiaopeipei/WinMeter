//! Bundled read-only LHM bridge. The main UI never needs elevation.
use serde::Serialize;
use std::{path::PathBuf, process::{Child, Command}, sync::{Mutex, OnceLock}, time::{Instant, SystemTime, UNIX_EPOCH}};
use windows::Win32::Foundation::{CloseHandle,HANDLE,WAIT_OBJECT_0};

struct Elevated(isize);
impl Drop for Elevated {fn drop(&mut self){unsafe{let _=CloseHandle(HANDLE(self.0 as *mut _));}}}
struct Bridge { exe: PathBuf, directory: PathBuf, output: PathBuf, child: Option<Child>,
    elevated:Option<Elevated>,started:Instant,failure:Option<String>,stopping:bool }
static BRIDGE: OnceLock<Mutex<Bridge>> = OnceLock::new();
static REQUESTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[derive(Clone, Default, Serialize)]
pub struct Status {pub message: String, pub admin: bool, pub available: bool,pub pending:bool,pub retryable:bool}

fn session_path(directory: &std::path::Path) -> PathBuf {
    directory.join(format!("sensors-{}-{}.json",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()))
}
fn spawn(bridge: &mut Bridge) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    bridge.child=Some(Command::new(&bridge.exe).arg("--watch").arg(std::process::id().to_string())
        .arg(&bridge.output).current_dir(bridge.exe.parent().ok_or("传感器路径无效")?)
        .creation_flags(0x08000000).spawn().map_err(|e|e.to_string())?);
    bridge.started=Instant::now();bridge.failure=None;
    Ok(())
}
pub fn init(resources: PathBuf, directory: PathBuf) {
    let exe=resources.join("hardware/WinMeter.Sensors.exe");
    let _=std::fs::create_dir_all(&directory);
    let mut bridge=Bridge {exe,directory:directory.clone(),output:session_path(&directory),child:None,
        elevated:None,started:Instant::now(),failure:None,stopping:false};
    if bridge.exe.is_file() && !crate::hardware_service::status().installed {if let Err(error)=spawn(&mut bridge){bridge.failure=Some(format!("采集器启动失败：{error}"));}}
    let _=BRIDGE.set(Mutex::new(bridge));
}
fn fresh(value: &serde_json::Value, now: u64) -> bool {
    value["Version"].as_u64()==Some(1) && value["Parent"].as_u64()==Some(std::process::id() as u64)
        && value["Stamp"].as_u64().is_some_and(|stamp|stamp<=now+2 && now.saturating_sub(stamp)<=10)
}
pub fn snapshot() -> (Option<serde_json::Value>, Status) {
    if let Some(snapshot)=crate::hardware_service::snapshot(){return snapshot;}
    let Some(mut bridge)=BRIDGE.get().and_then(|b|b.lock().ok()) else {return (None,Status {message:"内置传感器尚未初始化".into(),..Default::default()});};
    read_snapshot(&mut bridge,SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),REQUESTING.load(std::sync::atomic::Ordering::SeqCst))
}
fn process_exit(bridge:&mut Bridge)->Option<String>{
    if let Some(process)=&bridge.elevated {unsafe{
        if windows::Win32::System::Threading::WaitForSingleObject(HANDLE(process.0 as *mut _),0)==WAIT_OBJECT_0 {
            let mut code=0;let _=windows::Win32::System::Threading::GetExitCodeProcess(HANDLE(process.0 as *mut _),&mut code);
            return Some(format!("管理员采集器已退出（代码 {code}），请点击重试"));
        }
    }}else if let Some(child)=bridge.child.as_mut(){match child.try_wait(){
        Ok(Some(code))=>return Some(format!("采集器已退出（{code}），请点击重试")),
        Err(error)=>return Some(format!("无法检查采集器状态：{error}")),_=>{},
    }}
    None
}
fn read_snapshot(bridge:&mut Bridge,now:u64,requesting:bool)->(Option<serde_json::Value>,Status){
    let available=bridge.exe.is_file();
    let raw=std::fs::metadata(&bridge.output).ok().filter(|m|m.len()<256*1024)
        .and_then(|_|std::fs::read(&bridge.output).ok()).and_then(|b|serde_json::from_slice::<serde_json::Value>(&b).ok())
        .filter(|v|v["Version"].as_u64()==Some(1)&&v["Parent"].as_u64()==Some(std::process::id() as u64)&&v["Stamp"].as_u64().is_some_and(|t|t<=now+2));
    if let Some(v)=&raw {if v["Terminal"].as_bool()==Some(true){bridge.failure=Some(format!("采集器启动失败：{}；可点击重试",v["Error"].as_str().unwrap_or("未知错误")));}}
    if !requesting && !bridge.stopping && bridge.failure.is_none(){bridge.failure=process_exit(bridge);}
    let value=raw.filter(|v|fresh(v,now)&&v["Terminal"].as_bool()!=Some(true)&&bridge.failure.is_none());
    if !requesting && value.is_none() && bridge.failure.is_none() && bridge.started.elapsed().as_secs()>=30 {
        bridge.failure=Some("采集器 30 秒内未返回有效数据或采集已中断，请点击重试".into());
    }
    let admin=value.as_ref().and_then(|v|v["Admin"].as_bool()).unwrap_or(false);
    let pending=available && bridge.failure.is_none() && (requesting||value.is_none());
    let sensor_error=value.as_ref().and_then(|v|v["Error"].as_str()).is_some_and(|e|!e.is_empty());
    let message=if !available {"缺少内置传感器文件，请保留程序旁的 hardware 文件夹".into()}
        else if requesting {"正在等待 Windows 授权，请处理系统提示".into()}
        else if let Some(error)=&bridge.failure {error.clone()}
        else if let Some(v)=&value {
            let error=v["Error"].as_str().unwrap_or("");
            if !error.is_empty() {format!("内置传感器：{error}")}
            else if admin {"内置传感器已启用 · 管理员采集".into()}
            else {"普通权限采集中；CPU 温度需授权管理员采集，可点击按钮重试".into()}
        } else {"内置传感器启动中或暂未返回数据".into()};
    (value,Status {message,admin,available,pending,retryable:available&&!pending&&(!admin||sensor_error||bridge.failure.is_some())})
}
pub fn enable() -> Result<Status, String> {
    use std::sync::atomic::Ordering;
    // An installed (including disabled) service owns authorization. Never
    // prompt again or spawn a second sensor session at UI startup.
    if let Some((_,status))=crate::hardware_service::snapshot(){return Ok(status);}
    let current=snapshot().1;
    if current.admin&&!current.retryable&&!current.pending {return Ok(current);}
    if REQUESTING.compare_exchange(false,true,Ordering::SeqCst,Ordering::SeqCst).is_err() {
        return Err("正在请求管理员采集，请先处理 Windows 授权提示".into());
    }
    struct Reset;
    impl Drop for Reset {fn drop(&mut self){REQUESTING.store(false,Ordering::SeqCst);}}
    let _reset=Reset;
    use windows::{core::PCWSTR, Win32::UI::{Shell::{ShellExecuteExW,SHELLEXECUTEINFOW,SEE_MASK_NOCLOSEPROCESS},WindowsAndMessaging::SW_HIDE}};
    let (exe,output)= {
        let mut bridge=BRIDGE.get().ok_or("传感器尚未初始化")?.lock().map_err(|e|e.to_string())?;
        if !bridge.exe.is_file() {return Err("内置传感器文件缺失".into());}
        if bridge.stopping {return Err("程序正在退出".into());}
        if bridge.elevated.is_some()&&bridge.failure.is_none()&&bridge.started.elapsed().as_secs()<30 {
            return Err("管理员采集器正在启动，请等待返回数据".into());
        }
        let _=std::fs::write(bridge.output.with_extension("json.stop"),b"stop");
        // Give each launch its own file; a previous helper cannot overwrite it.
        bridge.output=session_path(&bridge.directory);
        bridge.child=None;bridge.elevated=None;bridge.failure=None;bridge.started=Instant::now();
        (bridge.exe.clone(),bridge.output.clone())
    };
    let wide=|s: &str|s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let verb=wide("runas"); let file=wide(&exe.to_string_lossy());
    let args=wide(&format!("--watch {} \"{}\"",std::process::id(),output.display()));
    let dir=wide(&exe.parent().ok_or("无效路径")?.to_string_lossy());
    let mut info=SHELLEXECUTEINFOW {cbSize:std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,fMask:SEE_MASK_NOCLOSEPROCESS,
        lpVerb:PCWSTR(verb.as_ptr()),lpFile:PCWSTR(file.as_ptr()),lpParameters:PCWSTR(args.as_ptr()),lpDirectory:PCWSTR(dir.as_ptr()),nShow:SW_HIDE.0,..Default::default()};
    if unsafe{ShellExecuteExW(&mut info)}.is_err() {
        if let Some(mut bridge)=BRIDGE.get().and_then(|b|b.lock().ok()) {if !bridge.stopping {if let Err(error)=spawn(&mut bridge){bridge.failure=Some(error);}}}
        return Err("未启用管理员采集：授权已取消或系统拒绝启动".into());
    }
    let process=(!info.hProcess.is_invalid()).then(||Elevated(info.hProcess.0 as isize));
    if let Some(mut bridge)=BRIDGE.get().and_then(|b|b.lock().ok()) {
        bridge.elevated=process;bridge.started=Instant::now();
        if bridge.elevated.is_none(){bridge.failure=Some("未取得采集进程句柄，请点击重试".into());}
    }
    Ok(Status {message:"正在启动管理员采集，请等待传感器数据".into(),pending:true,available:true,..Default::default()})
}
pub fn shutdown() {
    if let Some(mut bridge)=BRIDGE.get().and_then(|b|b.lock().ok()) {
        bridge.stopping=true;
        let _=std::fs::write(bridge.output.with_extension("json.stop"),b"stop");
    }
}

pub fn manage_service(action:&str)->Result<crate::hardware_service::ServiceStatus,String>{
    use std::sync::atomic::Ordering;
    if REQUESTING.compare_exchange(false,true,Ordering::SeqCst,Ordering::SeqCst).is_err(){return Err("正在处理采集授权，请稍后重试".into());}
    struct Reset;impl Drop for Reset{fn drop(&mut self){REQUESTING.store(false,Ordering::SeqCst);}}
    let _reset=Reset;
    let exe={let bridge=BRIDGE.get().ok_or("传感器尚未初始化")?.lock().map_err(|e|e.to_string())?;bridge.exe.clone()};
    let result=crate::hardware_service::manage(exe,action);
    if result.as_ref().is_ok_and(|s|s.installed) {
        if let Some(mut bridge)=BRIDGE.get().and_then(|b|b.lock().ok()) {
            let _=std::fs::write(bridge.output.with_extension("json.stop"),b"stop");
            bridge.child=None;bridge.elevated=None;
        }
    } else if result.as_ref().is_ok_and(|s|!s.installed) {
        if let Some(mut bridge)=BRIDGE.get().and_then(|b|b.lock().ok()){let _=spawn(&mut bridge);}
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture()->Bridge {
        let directory=std::env::temp_dir();
        Bridge{exe:std::env::current_exe().unwrap(),output:session_path(&directory),directory,child:None,elevated:None,
            started:Instant::now(),failure:None,stopping:false}
    }
    #[test]
    fn startup_timeout_is_retryable_but_waiting_for_uac_does_not_timeout(){
        let mut bridge=fixture();bridge.started=Instant::now()-std::time::Duration::from_secs(31);
        let (_,waiting)=read_snapshot(&mut bridge,100,true);assert!(waiting.pending);assert!(!waiting.retryable);
        let (data,failed)=read_snapshot(&mut bridge,100,false);
        assert!(data.is_none());assert!(failed.retryable);assert!(!failed.pending);assert!(failed.message.contains("30 秒"));
    }
    #[test]
    fn terminal_error_remains_visible_after_sensor_freshness_expires(){
        let mut bridge=fixture();
        let value=serde_json::json!({"Version":1,"Parent":std::process::id(),"Stamp":100,"Admin":true,"Terminal":true,"Error":"driver unavailable","Children":[]});
        std::fs::write(&bridge.output,serde_json::to_vec(&value).unwrap()).unwrap();
        for now in [101,1000] {let (data,status)=read_snapshot(&mut bridge,now,false);
            assert!(data.is_none());assert!(status.retryable);assert!(status.message.contains("driver unavailable"));}
        std::fs::remove_file(&bridge.output).unwrap();
    }
    #[test]
    fn exited_child_is_reported_without_waiting_for_a_sensor_file(){
        use std::os::windows::process::CommandExt;
        let mut bridge=fixture();
        let mut child=Command::new("cmd.exe").args(["/c","exit","7"]).creation_flags(0x08000000).spawn().unwrap();
        child.wait().unwrap();bridge.child=Some(child);
        let (data,status)=read_snapshot(&mut bridge,100,false);
        assert!(data.is_none());assert!(status.retryable);assert!(status.message.contains("已退出"));
    }
    #[test]
    fn rejects_stale_and_wrong_owner_sensor_files() {
        let mut v=serde_json::json!({"Version":1,"Parent":std::process::id(),"Stamp":100});
        assert!(super::fresh(&v,105)); assert!(!super::fresh(&v,111));
        v["Parent"]=serde_json::json!(0); assert!(!super::fresh(&v,105));
    }
}
