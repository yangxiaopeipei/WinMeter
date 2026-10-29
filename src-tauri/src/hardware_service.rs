//! Fixed-purpose Windows sensor service. Ordinary UI only reads its output.
use serde::Serialize;
use std::{path::PathBuf, time::{SystemTime,UNIX_EPOCH}};
use windows::{core::PCWSTR, Win32::{System::{Registry::*,Services::*,Threading::*},Foundation::{CloseHandle,WAIT_OBJECT_0},UI::{Shell::*,WindowsAndMessaging::SW_HIDE}}};
fn wide(value:&str)->Vec<u16>{value.encode_utf16().chain(Some(0)).collect()}
#[derive(Clone,Default,Serialize)]
pub struct ServiceStatus {pub installed:bool,pub running:bool,pub enabled:bool,pub message:String}
struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {fn drop(&mut self){unsafe{let _=CloseServiceHandle(self.0);}}}
pub fn status()->ServiceStatus {unsafe{
    let mut start=0u32; let mut size=4;
    let installed=RegGetValueW(HKEY_LOCAL_MACHINE,PCWSTR(wide(r"SYSTEM\CurrentControlSet\Services\WinMeterSensors").as_ptr()),PCWSTR(wide("Start").as_ptr()),RRF_RT_REG_DWORD,None,Some(&mut start as *mut _ as *mut _),Some(&mut size)).is_ok();
    if !installed {return ServiceStatus{message:"尚未安装自动采集服务".into(),..Default::default()};}
    let running=(||{
        let manager=ServiceHandle(OpenSCManagerW(None,None,SC_MANAGER_CONNECT).ok()?);
        let service=ServiceHandle(OpenServiceW(manager.0,PCWSTR(wide("WinMeterSensors").as_ptr()),SERVICE_QUERY_STATUS).ok()?);
        let mut state=SERVICE_STATUS::default(); QueryServiceStatus(service.0,&mut state).ok()?;
        Some(state.dwCurrentState==SERVICE_RUNNING)
    })().unwrap_or(false);
    let enabled=start!=4;
    ServiceStatus{installed,running,enabled,message:if !enabled {"自动温度采集已停用"}else if running {"自动温度采集服务运行中 · 无需重复授权"}else{"自动采集服务未运行，可点击恢复采集"}.into()}
}}
pub fn snapshot()->Option<(Option<serde_json::Value>,super::hardware::Status)> {
    let state=status(); if !state.installed {return None;}
    let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let path=std::env::var_os("ProgramData").map(PathBuf::from)?.join("WinMeterSensors/sensors.json");
    let value=if state.running {std::fs::metadata(&path).ok().filter(|m|m.len()<256*1024)
        .and_then(|_|std::fs::read(path).ok()).and_then(|bytes|serde_json::from_slice(&bytes).ok())
        .filter(|value|valid_snapshot(value,now))}else{None};
    let error=value.as_ref().and_then(|v|v["Error"].as_str()).filter(|v|!v.is_empty());
    let message=if let Some(error)=error {format!("自动采集服务：{error}；正在自动重试")}
        else if value.is_none()&&state.running {"自动采集服务启动中或数据暂时中断，正在等待恢复".into()}
        else {state.message};
    let valid=value.filter(|v|v["Terminal"].as_bool()!=Some(true));
    Some((valid,super::hardware::Status{message,admin:true,available:true,pending:false,retryable:false}))
}
fn valid_snapshot(value:&serde_json::Value,now:u64)->bool {
    value["Version"].as_u64()==Some(1) && value["Parent"].as_u64()==Some(0) && value["Admin"].as_bool()==Some(true)
        && value["Stamp"].as_u64().is_some_and(|stamp|stamp<=now+2&&now.saturating_sub(stamp)<=10)
}
pub fn manage(exe:PathBuf,action:&str)->Result<ServiceStatus,String>{
    let argument=match action {"install"=>"--install-service","remove"=>"--remove-service","disable"=>"--disable-service","resume"=>"--resume-service",_=>return Err("不支持的服务操作".into())};
    let file=wide(&exe.to_string_lossy());let verb=wide("runas");let args=wide(argument);
    let directory=wide(&exe.parent().ok_or("采集器路径无效")?.to_string_lossy());
    let mut info=SHELLEXECUTEINFOW{cbSize:std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,fMask:SEE_MASK_NOCLOSEPROCESS,
        lpVerb:PCWSTR(verb.as_ptr()),lpFile:PCWSTR(file.as_ptr()),lpParameters:PCWSTR(args.as_ptr()),lpDirectory:PCWSTR(directory.as_ptr()),nShow:SW_HIDE.0,..Default::default()};
    unsafe {
        ShellExecuteExW(&mut info).map_err(|_|"服务操作未完成：Windows 授权被取消或启动失败".to_string())?;
        if info.hProcess.is_invalid(){return Err("未取得服务安装进程句柄".into());}
        let wait=WaitForSingleObject(info.hProcess,90000);
        let mut code=1; let result=GetExitCodeProcess(info.hProcess,&mut code); let _=CloseHandle(info.hProcess);
        if wait!=WAIT_OBJECT_0{return Err("服务操作仍未完成，请稍后查看服务状态，勿重复操作".into());}
        result.map_err(|e|e.to_string())?;
        if code!=0 {return Err(format!("服务操作失败（代码 {code}），详情见 %ProgramData%\\WinMeterSensors\\setup-error.txt"));}
    }
    let state=status();
    let success=match action {"remove"=>!state.installed,"disable"=>state.installed&&!state.enabled&&!state.running,_=>state.installed&&state.running&&state.enabled};
    if !success {return Err(format!("服务状态未达到预期：{}",state.message));}
    Ok(state)
}
#[cfg(test)] mod tests {
    #[test] fn service_readings_reject_stale_user_process_and_non_admin_data(){
        let mut value=serde_json::json!({"Version":1,"Parent":0,"Admin":true,"Stamp":100});
        assert!(super::valid_snapshot(&value,105));assert!(!super::valid_snapshot(&value,111));assert!(!super::valid_snapshot(&value,97));
        value["Parent"]=serde_json::json!(123);assert!(!super::valid_snapshot(&value,105));
        value["Parent"]=serde_json::json!(0);value["Admin"]=serde_json::json!(false);assert!(!super::valid_snapshot(&value,105));
    }
}
