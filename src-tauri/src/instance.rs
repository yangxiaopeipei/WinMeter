//! Session-local ownership acquired before any hardware or taskbar work starts.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use windows::{core::PCWSTR, Win32::{Foundation::{CloseHandle, GetLastError, HANDLE, ERROR_ALREADY_EXISTS, WAIT_OBJECT_0},
    System::Threading::{CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject}}};

pub struct Instance { mutex:isize, event:isize, stopped:AtomicBool }
fn wide(s:&str)->Vec<u16>{s.encode_utf16().chain(Some(0)).collect()}
impl Instance {
    pub fn acquire()->Result<Option<Arc<Self>>,String>{Self::named(r"Local\WinMeter.com.winmeter.app")}
    fn named(name:&str)->Result<Option<Arc<Self>>,String>{unsafe{
        let mutex=CreateMutexW(None,false,PCWSTR(wide(name).as_ptr())).map_err(|e|e.to_string())?;
        let existing=GetLastError()==ERROR_ALREADY_EXISTS;
        let event=match CreateEventW(None,false,false,PCWSTR(wide(&format!("{name}.activate")).as_ptr())) {
            Ok(event)=>event,Err(e)=>{let _=CloseHandle(mutex);return Err(e.to_string());}
        };
        if existing {
            let result=SetEvent(event).map_err(|e|e.to_string());
            let _=CloseHandle(event);let _=CloseHandle(mutex);result?;return Ok(None);
        }
        Ok(Some(Arc::new(Self{mutex:mutex.0 as isize,event:event.0 as isize,stopped:AtomicBool::new(false)})))
    }}
    pub fn listen(self:&Arc<Self>, activate:impl Fn()+Send+'static){
        let owner=self.clone();
        std::thread::spawn(move||while !owner.stopped.load(Ordering::SeqCst){
            let signaled=unsafe{WaitForSingleObject(HANDLE(owner.event as *mut _),500)==WAIT_OBJECT_0};
            if signaled && !owner.stopped.load(Ordering::SeqCst){activate();}
        });
    }
    pub fn stop(&self){self.stopped.store(true,Ordering::SeqCst);unsafe{let _=SetEvent(HANDLE(self.event as *mut _));}}
}
impl Drop for Instance {fn drop(&mut self){unsafe{let _=CloseHandle(HANDLE(self.event as *mut _));let _=CloseHandle(HANDLE(self.mutex as *mut _));}}}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secondary_signals_primary_and_releases_without_starting_workers(){
        let name=format!(r"Local\WinMeter.test.{}",std::process::id());
        let first=Instance::named(&name).unwrap().unwrap();
        assert!(Instance::named(&name).unwrap().is_none());
        assert_eq!(unsafe{WaitForSingleObject(HANDLE(first.event as *mut _),1000)},WAIT_OBJECT_0);
        drop(first);
        assert!(Instance::named(&name).unwrap().is_some());
    }
}
