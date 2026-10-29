use super::*;
use std::{ffi::c_void, mem::size_of, time::{Duration, Instant}};
use windows::{core::{PCWSTR, PWSTR, VARIANT}, Win32::{
    Foundation::{CloseHandle, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
    Graphics::Gdi::*,
    System::{Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
        LibraryLoader::GetModuleHandleW, Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
        Threading::{GetCurrentProcessId,OpenProcess,QueryFullProcessImageNameW,PROCESS_QUERY_LIMITED_INFORMATION,PROCESS_NAME_WIN32}},
    UI::{Accessibility::*, HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, GetWindowDpiAwarenessContext, SetThreadDpiAwarenessContext}, WindowsAndMessaging::*},
}};

#[path = "taskbar_classic.rs"]
mod classic;

static MENU_HOLD: Mutex<Option<Instant>> = Mutex::new(None);

static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

// ---------- HUD 线程阶段计时 ----------
//
// DWM 会在某个窗口线程连续约 5 秒不处理消息时，在完全相同的位置盖一张
// 同尺寸快照（类名 `Ghost`）。鼠标停在快照上显示的就是等待光标——也就是
// 用户看到的「挪上去就转圈圈」。所以幽灵窗口和转圈光标是同一个病：窗口线程
// 的消息泵被卡住了。
//
// 这里把 HUD 主循环里每个可能耗时的阶段都掐一次表，超过 300ms 就追加一行到
// `%APPDATA%\WinMeter\hud-timing.log`。正常运行时这个文件基本不增长，
// 出问题时能直接看出是哪一步卡住、卡了多久、在第几秒。
static HUD_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn hud_log_path() -> std::path::PathBuf {
    let dir = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("WinMeter");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("hud-timing.log")
}

fn hud_slow(tag: &str, since: Instant) {
    let took = since.elapsed();
    if took < Duration::from_millis(300) { return; }
    let at = HUD_START.get().map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
    let line = format!("{at:7.2}s  {:>7}ms  {tag}\n", took.as_millis());
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(hud_log_path()) {
        use std::io::Write;
        let _ = file.write_all(line.as_bytes());
    }
}

// 只有设了 `WINMETER_HUD_TRACE=1` 才打开：每个刷新周期记一行心跳。
//
// 这行日志回答一个关键问题——系统报「窗口无响应」的那段时间，主循环到底还在
// 不在跑。如果心跳照常往下走，那 IsHungAppWindow 的结论就另有来源，不是线程
// 被卡住了；如果心跳停了，看门狗那边还会同时记下当时停在哪个阶段。
static HUD_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn hud_trace() -> bool {
    *HUD_TRACE.get_or_init(|| std::env::var_os("WINMETER_HUD_TRACE").is_some())
}

fn hud_trace_line(phase: usize) {
    let at = HUD_START.get().map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0);
    let line = format!("{at:7.2}s  HB  phase={phase}\n");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(hud_log_path()) {
        use std::io::Write;
        let _ = file.write_all(line.as_bytes());
    }
}

// 上面的 hud_slow 只能记录「已经返回」的调用。如果某个跨进程调用根本不返回，
// 那行日志永远写不出来——这正好是我们要找的情况。
//
// 所以再放一层：主循环在进入每个可能阻塞的调用之前，先把「当前阶段」写进
// 原子变量，并留一个心跳时间戳。看门狗线程每 400ms 检查一次，一旦发现心跳
// 停了超过 2 秒，就直接把「停了多久 + 当时在哪个阶段」写进日志。
// 因为阶段名是在调用之前写的，所以能精确指向那个没返回的调用。
const HUD_PHASES: [&str; 12] = [
    "空闲（recv_timeout 等数据）",
    "PeekMessage / DispatchMessage",
    "store.get()",
    "FindWindowW(Shell_TrayWnd)",
    "menu_active（EnumWindows + 菜单状态）",
    "read_taskbar_layout（UIA FindAll 跨进程）",
    "taskbar_exposed（shell 状态 + 前台窗口）",
    "surface（建 DIB + DrawText）",
    "UpdateLayeredWindow",
    "SetWindowPos（分层/定位）",
    "keep_above_shell",
    "CreateWindowExW（重建窗口）",
];

static HUD_PHASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static HUD_BEAT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[inline]
fn hud_phase(id: usize) {
    HUD_PHASE.store(id, std::sync::atomic::Ordering::Relaxed);
    // 进入阶段的同时打心跳：看门狗算出的间隔就是「在现在这个阶段待了多久」。
    hud_beat();
}

#[inline]
fn hud_beat() {
    let ms = HUD_START.get().map(|s| s.elapsed().as_millis() as u64).unwrap_or(0);
    HUD_BEAT.store(ms, std::sync::atomic::Ordering::Relaxed);
}

fn spawn_stall_watch() {    std::thread::spawn(|| {
        use std::sync::atomic::Ordering;
        let mut reported = false;
        loop {
            std::thread::sleep(Duration::from_millis(400));
            let Some(start) = HUD_START.get() else { continue };
            let now = start.elapsed().as_millis() as u64;
            let beat = HUD_BEAT.load(Ordering::Relaxed);
            let gap = now.saturating_sub(beat);
            if gap > 2000 {
                // 只在从「正常」跳进「停摆」时记一次，避免每 400ms 刷屏。
                if !reported {
                    reported = true;
                    let id = HUD_PHASE.load(Ordering::Relaxed);
                    let phase = HUD_PHASES.get(id).copied().unwrap_or("未知阶段");
                    let line = format!("{:7.2}s  停摆（已 {:.1}s 没心跳）  卡在: {}\n",
                        now as f64 / 1000.0, gap as f64 / 1000.0, phase);
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true).append(true).open(hud_log_path()) {
                        use std::io::Write;
                        let _ = file.write_all(line.as_bytes());
                    }
                }
            } else {
                reported = false;
            }
        }
    });
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

// Explorer owns all taskbar geometry. Never resize or move its child windows:
// classic taskbar replacements may recenter icons when a child is resized.
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(super) fn shutdown() {
    SHUTTING_DOWN.store(true,std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod rendering_tests {
    use super::*;
    #[test]
    fn native_font_uses_saved_size_and_the_preview_weight() {
        unsafe {
            let mut settings=Settings::default(); settings.taskbar_font_size=16;
            settings.taskbar_font="Microsoft YaHei UI".into();
            let handle=font(&settings,1.75);
            assert!(!handle.is_invalid());
            let mut actual=LOGFONTW::default();
            let read=GetObjectW(handle,size_of::<LOGFONTW>() as i32,Some(&mut actual as *mut _ as *mut c_void));
            let _=DeleteObject(handle);
            assert_eq!(read,size_of::<LOGFONTW>() as i32);
            assert_eq!(actual.lfHeight,-28);
            assert_eq!(actual.lfWeight,400);
        }
    }
    #[test]
    fn network_field_does_not_move_when_digits_or_units_change() {
        unsafe {
            let dc=CreateCompatibleDC(None);
            assert!(!dc.is_invalid());
            let mut settings=Settings::default();
            settings.taskbar_order=vec!["network".into()];
            settings.taskbar_network=true;
            settings.taskbar_text_align="center".into();
            let f=font(&settings,1.75); let old=SelectObject(dc,f);
            let mut fields=Vec::new();
            for up in [0.0,8.6*1024.0,13.0*1024.0,100.0*1048576.0,999.9*1073741824.0,f64::MAX] {
                let cols=columns(dc,&settings,&Values {up,down:up,..Default::default()});
                let c=&cols[0];
                fields.push((c.width,c.arrow_width,text_width(dc,&c.top)));
            }
            SelectObject(dc,old); let _=DeleteObject(f); let _=DeleteDC(dc);
            let (width,arrow,_)=fields[0];
            assert!(arrow>0);
            for (w,a,text) in fields {
                assert_eq!((w,a),(width,arrow),"network and arrow rectangles must stay fixed");
                assert!(text<=width-arrow,"every formatted speed must fit its field");
            }
        }
    }
}

unsafe fn dark_theme(setting: &str) -> bool {
    if setting != "system" { return setting == "dark"; }
    let mut light = 0u32; let mut bytes = size_of::<u32>() as u32;
    let _ = RegGetValueW(HKEY_CURRENT_USER,
        PCWSTR(wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize").as_ptr()),
        PCWSTR(wide("SystemUsesLightTheme").as_ptr()), RRF_RT_REG_DWORD, None,
        Some(&mut light as *mut _ as *mut c_void), Some(&mut bytes));
    light == 0
}
pub(super) fn system_dark() -> bool {unsafe {dark_theme("system")}}

unsafe fn taskbar_present(shell: HWND, rect: &RECT) -> bool {
    if !IsWindowVisible(shell).as_bool() { return false; }
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !GetMonitorInfoW(MonitorFromWindow(shell, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() { return false; }
    let visible_height = (rect.bottom.min(info.rcMonitor.bottom) - rect.top.max(info.rcMonitor.top)).max(0);
    if visible_height < (rect.bottom - rect.top) / 2 { return false; }
    rect.bottom>rect.top && rect.right>rect.left
}

unsafe fn foreground_is_shell(shell:HWND,foreground:HWND)->bool {
    let mut shell_pid=0; let mut pid=0;
    GetWindowThreadProcessId(shell,Some(&mut shell_pid));
    GetWindowThreadProcessId(foreground,Some(&mut pid));
    if pid!=0 && (pid==shell_pid || pid==GetCurrentProcessId()) {return true;}
    let Ok(process)=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid) else {return false;};
    let mut path=[0u16;1024]; let mut count=path.len() as u32;
    let result=QueryFullProcessImageNameW(process,PROCESS_NAME_WIN32,PWSTR(path.as_mut_ptr()),&mut count);
    let _=CloseHandle(process);
    result.is_ok() && shell_host_image(&String::from_utf16_lossy(&path[..count.min(path.len() as u32) as usize]))
}

unsafe fn taskbar_exposed(shell: HWND, rect: &RECT) -> bool {
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState,QUNS_RUNNING_D3D_FULL_SCREEN,QUNS_PRESENTATION_MODE};
    if !taskbar_present(shell,rect) {return false;}
    let fg=GetForegroundWindow();
    // Tray flyouts and our own popup can use monitor-sized transparent hosts.
    // They must never be interpreted as fullscreen applications.
    if fg.is_invalid() || foreground_is_shell(shell,fg) {return true;}
    if SHQueryUserNotificationState().is_ok_and(|state|state==QUNS_RUNNING_D3D_FULL_SCREEN||state==QUNS_PRESENTATION_MODE){return false;}
    let mut info=MONITORINFO {cbSize:size_of::<MONITORINFO>() as u32,..Default::default()};
    if !GetMonitorInfoW(MonitorFromWindow(shell,MONITOR_DEFAULTTONEAREST),&mut info).as_bool() {return false;}
    if !fg.is_invalid() && fg != shell {
        let mut class = [0u16;128]; let len = GetClassNameW(fg, &mut class);
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_ok() && fullscreen_app(
            &String::from_utf16_lossy(&class[..len.max(0) as usize]), [r.left,r.top,r.right,r.bottom],
            [info.rcMonitor.left,info.rcMonitor.top,info.rcMonitor.right,info.rcMonitor.bottom]) { return false; }
    }
    true
}

// Leave the existing overlay and its Z order alone during menu tracking.
// Hit-testing a popup as though it were a game would hide the whole monitor;
// repeatedly raising the overlay would instead draw its text above the menu.
unsafe fn menu_active(shell:HWND) -> bool {
    if MENU_HOLD.lock().ok().is_some_and(|hold|hold.is_some_and(|until|Instant::now()<until)) {return true;}
    // Explorer's taskbar menus may track on a different thread without becoming
    // the foreground window. Query both threads, then visible menu windows.
    for thread in [0,GetWindowThreadProcessId(shell,None)] {
        let mut info=GUITHREADINFO {cbSize:size_of::<GUITHREADINFO>() as u32,..Default::default()};
        if GetGUIThreadInfo(thread,&mut info).is_ok()
            && info.flags.0 & (GUI_INMENUMODE.0|GUI_POPUPMENUMODE.0|GUI_SYSTEMMENUMODE.0)!=0 {return true;}
    }
    unsafe extern "system" fn visit(hwnd:HWND,lp:LPARAM)->windows::Win32::Foundation::BOOL {
        if IsWindowVisible(hwnd).as_bool() {
            let mut name=[0u16;128]; let n=GetClassNameW(hwnd,&mut name);
            if transient_shell_surface(&String::from_utf16_lossy(&name[..n.max(0) as usize])) {
                let mut r=RECT::default();
                if !IsIconic(hwnd).as_bool() && GetWindowRect(hwnd,&mut r).is_ok() && r.right>r.left && r.bottom>r.top {
                    *(lp.0 as *mut bool)=true; return windows::Win32::Foundation::BOOL(0);
                }
            }
        }
        windows::Win32::Foundation::BOOL(1)
    }
    let mut found=false;
    let _=EnumWindows(Some(visit),LPARAM(&mut found as *mut bool as isize));
    found
}

// Read geometry only. No names, clicks, or changes to Explorer's settings.
unsafe fn uia_buttons(automation: &IUIAutomation, shell: HWND) -> windows::core::Result<Vec<RECT>> {
    let button = automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_ButtonControlTypeId.0))?;
    let item = automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_ListItemControlTypeId.0))?;
    let menu = automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_MenuItemControlTypeId.0))?;
    let condition = automation.CreateOrCondition(&automation.CreateOrCondition(&button, &item)?, &menu)?;
    let elements = automation.ElementFromHandle(shell)?.FindAll(TreeScope_Descendants, &condition)?;
    let mut out = Vec::new();
    for i in 0..elements.Length()?.min(512) {
        if let Ok(e) = elements.GetElement(i) {
            if e.CurrentIsOffscreen().map(|v| v.as_bool()).unwrap_or(true) { continue; }
            if let Ok(r) = e.CurrentBoundingRectangle() {
                if r.right > r.left && r.bottom > r.top { out.push(r); }
            }
        }
    }
    Ok(out)
}

unsafe fn read_taskbar_layout(automation:Option<&IUIAutomation>,shell:HWND)->windows::core::Result<(Vec<RECT>,bool)> {
    let uia=automation.map(|uia|uia_buttons(uia,shell));
    if let Some((bounds,classic))=classic::buttons(shell) {
        // Replace the whole classic task-list region, including UIA wrapper
        // rectangles. Only real accessible application buttons occupy space.
        let mut out=uia.and_then(Result::ok).unwrap_or_default();
        out.retain(|r|r.right<=bounds.left || r.left>=bounds.right || r.bottom<=bounds.top || r.top>=bounds.bottom);
        out.extend(classic);
        return Ok((out,true));
    }
    uia.unwrap_or_else(||Err(windows::core::Error::from_win32())).map(|rects|(rects,false))
}

#[cfg(test)]
pub(super) unsafe fn taskbar_buttons(automation:&IUIAutomation,shell:HWND)->windows::core::Result<Vec<RECT>> {
    read_taskbar_layout(Some(automation),shell).map(|(rects,_)|rects)
}

unsafe fn child_rect(shell: HWND, class: &str) -> Option<RECT> {
    let hwnd = classic::descendant(shell,class)?;
    if !IsWindowVisible(hwnd).as_bool() { return None; }
    let mut rect = RECT::default(); GetWindowRect(hwnd, &mut rect).ok()?;
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

unsafe fn font(settings: &Settings, scale: f64) -> HFONT {
    CreateFontW(-(settings.taskbar_font_size as f64 * scale).round() as i32,
        0,0,0,400,0,0,0,DEFAULT_CHARSET.0 as u32,OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,ANTIALIASED_QUALITY.0 as u32,DEFAULT_PITCH.0 as u32,
        PCWSTR(wide(&settings.taskbar_font).as_ptr()))
}

struct Column { top: String, bottom: String, width: i32, arrow_width: i32 }
unsafe fn text_width(dc:HDC,s:&str)->i32 {
    let mut size=SIZE::default();
    if GetTextExtentPoint32W(dc,&s.encode_utf16().collect::<Vec<_>>(),&mut size).as_bool(){size.cx}else{0}
}
unsafe fn columns(dc: HDC, settings: &Settings, values: &Values) -> Vec<Column> {
    let mut out = Vec::new();
    if settings.taskbar_order.is_empty() {
        let width=text_width(dc,"速览").max(text_width(dc,"打开"));
        return vec![Column {top:"速览".into(),bottom:"打开".into(),width,arrow_width:0}];
    }
    for key in &settings.taskbar_order {
        let (top, bottom, template) = match key.as_str() {
            "network" if settings.taskbar_network => (rate(values.up),rate(values.down),"999.9 MB/s"),
            "cpu" if settings.taskbar_cpu => ("CPU".into(),format!("{:.0}%",values.cpu),"100%"),
            "memory" if settings.taskbar_memory => ("内存".into(),format!("{}%",values.memory),"100%"),
            "cpu_temp" if settings.taskbar_cpu_temp => ("CPU温度".into(),temperature(values.cpu_temp),"100 ℃"),
            "gpu_temp" if settings.taskbar_gpu_temp => ("显卡温度".into(),temperature(values.gpu_temp),"100 ℃"),
            "disk_temp" if settings.taskbar_disk_temp => ("磁盘温度".into(),temperature(values.disk_temp),"100 ℃"),
            _ => continue,
        };
        let arrow_width=if key=="network" {text_width(dc,"↑ ").max(text_width(dc,"↓ "))}else{0};
        let width=if arrow_width>0 {
            // Reserve the same field through every digit count and unit change.
            ["999.9 MB/s","999.9 GB/s","999.9 KB/s","999+ TB/s","1024 B/s"]
                .iter().map(|s|text_width(dc,s)).max().unwrap_or(0)+arrow_width
        } else {text_width(dc,&top).max(text_width(dc,&bottom)).max(text_width(dc,template))};
        out.push(Column { top,bottom,width,arrow_width });
    }
    out
}

// A 32-bit premultiplied alpha surface: only glyph pixels have nonzero alpha.
struct Surface { dc: HDC, bitmap: HBITMAP, previous: HGDIOBJ, width: i32, height: i32 }
impl Drop for Surface {
    fn drop(&mut self) { unsafe { SelectObject(self.dc,self.previous); let _ = DeleteObject(self.bitmap); let _ = DeleteDC(self.dc); } }
}
unsafe fn surface(settings: &Settings, values: &Values, scale: f64, height: i32) -> Option<Surface> {
    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() { return None; }
    let text_font = font(settings,scale); let old_font = SelectObject(dc,text_font);
    let columns = columns(dc,settings,values);
    let padding = (4.0 * scale).ceil() as i32;
    let gap = (settings.taskbar_gap as f64 * scale).round() as i32;
    let width = columns.iter().map(|c| c.width).sum::<i32>() + gap * columns.len().saturating_sub(1) as i32 + padding * 2;
    if width < 1 || height < 1 { SelectObject(dc,old_font); let _ = DeleteObject(text_font); let _ = DeleteDC(dc); return None; }
    let info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize:size_of::<BITMAPINFOHEADER>() as u32,
        biWidth:width, biHeight:-height, biPlanes:1, biBitCount:32, biCompression:BI_RGB.0, ..Default::default() }, ..Default::default() };
    let mut bits = std::ptr::null_mut();
    let bitmap = match CreateDIBSection(dc,&info,DIB_RGB_COLORS,&mut bits,None,0) {
        Ok(b) => b,
        Err(_) => { SelectObject(dc,old_font); let _ = DeleteObject(text_font); let _ = DeleteDC(dc); return None; }
    };
    let previous = SelectObject(dc,bitmap);
    // DIB 成功但位指针为空属于极端情况，解引用会直接崩，这里先挡住。
    if bits.is_null() {
        SelectObject(dc,old_font); SelectObject(dc,previous);
        let _ = DeleteObject(bitmap); let _ = DeleteObject(text_font); let _ = DeleteDC(dc);
        return None;
    }
    let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, width as usize * height as usize * 4);
    pixels.fill(0);
    SetBkMode(dc,TRANSPARENT); SetTextColor(dc,COLORREF(0x00ffffff));
    let line_height = ((settings.taskbar_font_size as f64 + 2.0) * scale).round() as i32;
    let top = ((height - 2*line_height)/2).max(0);
    let mut x = padding;
    for c in columns {
        for (row,(text,y)) in [(c.top,top),(c.bottom,top+line_height)].into_iter().enumerate() {
            let mut rect = RECT {left:x,right:x+c.width+1,top:y,bottom:y+line_height};
            let alignment=if c.arrow_width>0 {
                let mut arrow_rect=RECT{right:x+c.arrow_width,..rect};
                let arrow=if row==0 {"↑"}else{"↓"};
                DrawTextW(dc,&mut arrow.encode_utf16().collect::<Vec<_>>(),&mut arrow_rect,DT_LEFT|DT_VCENTER|DT_SINGLELINE|DT_NOPREFIX);
                rect.left+=c.arrow_width;
                DT_RIGHT
            } else if settings.taskbar_text_align=="center" {DT_CENTER}else{DT_LEFT};
            DrawTextW(dc,&mut text.encode_utf16().collect::<Vec<_>>(),&mut rect,alignment|DT_VCENTER|DT_SINGLELINE|DT_NOPREFIX);
        }
        x += c.width + gap;
    }
    let _ = GdiFlush();
    text_alpha(pixels,if dark_theme(&settings.taskbar_theme) {239} else {38});
    // Alpha-zero pixels pass mouse events through a layered window. A 1/255
    // alpha background makes the full reserved HUD rectangle clickable.
    for px in pixels.chunks_exact_mut(4) {if px[3]==0 {px[3]=1;}}
    SelectObject(dc,old_font); let _ = DeleteObject(text_font);
    Some(Surface { dc,bitmap,previous,width,height })
}

// Explorer raises Shell_TrayWnd to the top of its Z group whenever the taskbar
// or the Start menu is activated. The taskbar window spans the whole bar, so
// that single reorder buries the overlay completely. Repairing only on the slow
// layout pass left a visible flicker on every click, so this cheap check runs on
// every message-loop turn and keeps the overlay directly above the taskbar.
unsafe fn keep_above_shell(hwnd: HWND, shell: HWND, same_level: bool) -> bool {
    if !same_level || !IsWindow(hwnd).as_bool() || !IsWindowVisible(hwnd).as_bool() { return false; }
    // One hit test settles the common case without walking the Z chain.
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() { return false; }
    let center = POINT { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 };
    if WindowFromPoint(center) == hwnd { return false; }
    let mut next = GetWindow(shell, GW_HWNDNEXT).unwrap_or_default();
    for _ in 0..64 {
        if next.is_invalid() { return false; }
        if next == hwnd {
            // Insert above the taskbar but below anything already stacked on it
            // (tray flyouts, Start menu), so those keep covering the overlay.
            let before = GetWindow(shell, GW_HWNDPREV).unwrap_or_default();
            let insert = if before.is_invalid() { HWND_TOP } else { before };
            let _ = SetWindowPos(hwnd, insert, 0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOREDRAW);
            return true;
        }
        next = GetWindow(next, GW_HWNDNEXT).unwrap_or_default();
    }
    false
}

// Polling alone always leaves a gap: Explorer reorders its own Z chain between
// two of our turns, and even a 20 ms window is visible as a flicker. This hook
// reacts to the reorder itself, so the overlay is put back within a frame.
static GUARD_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static GUARD_SHELL: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static GUARD_LEVEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static GUARD_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn on_reorder(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    use std::sync::atomic::Ordering;
    if GUARD_BUSY.swap(true, Ordering::SeqCst) { return; }
    // 跨 extern "system" 边界 panic 是 abort，跟 profile 里的 panic 设置无关，
    // 这里必须自己兜住，否则一次意外就能带走整个进程。
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let hwnd = HWND(GUARD_HWND.load(Ordering::Relaxed) as *mut c_void);
        let shell = HWND(GUARD_SHELL.load(Ordering::Relaxed) as *mut c_void);
        if !hwnd.is_invalid() && !shell.is_invalid() {
            let _ = keep_above_shell(hwnd, shell, GUARD_LEVEL.load(Ordering::Relaxed));
        }
    }));
    GUARD_BUSY.store(false, Ordering::SeqCst);
}

fn spawn_z_guard() {
    std::thread::spawn(|| {
        // 守护线程只负责摆 Z 序，挂了也不该连累主界面。
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hook = SetWinEventHook(EVENT_OBJECT_REORDER, EVENT_OBJECT_REORDER, None,
            Some(on_reorder), 0, 0, WINEVENT_OUTOFCONTEXT);
        if hook.0.is_null() { return; }
        // A dedicated blocking loop keeps the callback off the 20 ms refresh
        // cadence; the event is handled as soon as Windows delivers it.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWinEvent(hook);
        }));
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => { let _ = ValidateRect(hwnd,None); LRESULT(0) },
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        // 类光标已经设成箭头，这里再显式设一次，避免某些情况下
        // DefWindowProc 把光标留成上一处遗留的等待光标。
        WM_SETCURSOR => { let _=SetCursor(LoadCursorW(None,IDC_ARROW).unwrap_or_default()); LRESULT(1) },
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONUP => {
            if let Some(app)=APP.get() {
                let mut anchor=POINT::default();
                if GetCursorPos(&mut anchor).is_ok() {
                    let ui=app.clone();
                    let _=app.run_on_main_thread(move||crate::popup::toggle(&ui,anchor.x as f64,anchor.y as f64));
                }
            }
            LRESULT(0)
        },
        WM_CONTEXTMENU => {
            if let Ok(shell)=FindWindowW(PCWSTR(wide("Shell_TrayWnd").as_ptr()),PCWSTR::null()) {
                if let Ok(mut hold)=MENU_HOLD.lock() {*hold=Some(Instant::now()+Duration::from_millis(1000));}
                let _=PostMessageW(shell,WM_CONTEXTMENU,WPARAM(shell.0 as usize),lp);
            }
            LRESULT(0)
        },
        _ => DefWindowProcW(hwnd,msg,wp,lp),
    }
}

pub fn run(app: tauri::AppHandle, store: Arc<SettingsStore>, receiver: mpsc::Receiver<Values>, status: Arc<Mutex<HudStatus>>) {
    let _=APP.set(app);
    let _=HUD_START.set(Instant::now());
    hud_beat();
    spawn_stall_watch();
    unsafe {
        let _ = CoInitializeEx(None,COINIT_MULTITHREADED);
        let automation: Option<IUIAutomation> = CoCreateInstance(&CUIAutomation,None,CLSCTX_INPROC_SERVER).ok();
        spawn_z_guard();
        let class = wide("WinMeterTaskbarHud");
        let module = match GetModuleHandleW(None) { Ok(m) => m,Err(_) => return };
        RegisterClassW(&WNDCLASSW {lpfnWndProc:Some(wndproc),hInstance:module.into(),lpszClassName:PCWSTR(class.as_ptr()),
            // 类光标必须显式给箭头。留空的话 Windows 在 WM_SETCURSOR 时不会重设光标，
            // 鼠标移进来会一直沿用上一个窗口留下的形状——只要之前碰到过等待光标
            // （WebView2 加载、别处的忙碌状态），停在监控条上就一直转圈。
            hCursor:LoadCursorW(None,IDC_ARROW).unwrap_or_default(),..Default::default()});
        let mut hwnd = HWND::default(); let mut parent = HWND::default();
        let mut values = Values::default();
        let mut buttons = Vec::new(); let mut layout_ok = false; let mut classic_layout=false;
        let mut last_layout = Instant::now() - Duration::from_secs(5);
        let mut previous_settings:Option<Settings>=None;
        let mut layout_bar=RECT::default();
        let mut layout_dpi=0;
        let mut last_valid_layout=Instant::now()-Duration::from_secs(5);
        // Cached Z-order guard state, refreshed by the slower layout pass.
        let mut guard_shell=HWND::default();
        let mut guard_level=false;
        let mut last_full=Instant::now()-Duration::from_secs(5);
        // Consecutive failed layout passes. A single bad UIA read must never
        // blink a valid overlay, so hiding waits for three in a row.
        let mut hide_ticks=0u32;
        // Last painted state, so unchanged frames do no DWM work at all.
        let mut last_values:Option<Values>=None;
        let mut last_geom=(i32::MIN,i32::MIN,i32::MIN,i32::MIN);
        let mut last_variant=usize::MAX;
        let mut turn=Instant::now();
        loop {
            // 上一轮结束后到现在的空档。任何阶段卡住都会在恢复后的这一行暴露出来，
            // 包括走 continue 提前结束的路径，不需要在每个出口单独埋点。
            hud_slow("循环空档（上一轮结束后停住的时间）",turn);
            turn=Instant::now();
            hud_beat();
            // Wake on every frame so a taskbar or Start menu activation cannot
            // bury the overlay for a whole refresh interval. The expensive
            // layout, UIA and GDI work below stays throttled.
            hud_phase(0);
            // 这里必须用 MsgWaitForMultipleObjects 等消息，不能在数据通道上干睡。
            //
            // Windows 判断「窗口无响应」看的是线程有没有处在等待消息的状态
            // （GetMessage / MsgWaitForMultipleObjects）。原来的写法是
            // recv_timeout(20ms) + PeekMessage 轮询：线程一直在通道上睡觉，
            // 从来不进入消息等待，于是被判定成无响应——DWM 在完全相同的位置
            // 盖一张 `Ghost` 快照，鼠标移上去就显示等待光标（转圈）。
            // 而线程其实活得好好的：从外部发 WM_NULL，15ms 就回了。
            let _ = MsgWaitForMultipleObjects(None, false, 20, QS_ALLINPUT);
            hud_phase(1);
            let mut msg=MSG::default();
            while PeekMessageW(&mut msg,None,0,0,PM_REMOVE).as_bool() {let _=TranslateMessage(&msg);DispatchMessageW(&msg);}
            // 数据通道改成非阻塞取，等待交给上面的消息等待。取到最新的那条即可，
            // 免得积压时越跑越旧。
            let mut latest=None; let mut closed=false;
            for _ in 0..16 {
                match receiver.try_recv() {
                    Ok(v)=>latest=Some(v),
                    Err(std::sync::mpsc::TryRecvError::Disconnected)=>{closed=true;break}
                    Err(std::sync::mpsc::TryRecvError::Empty)=>break,
                }
            }
            if closed { break; }
            if let Some(v)=latest {values=v; last_full=Instant::now()-Duration::from_secs(5);}
            if SHUTTING_DOWN.load(std::sync::atomic::Ordering::SeqCst) { break; }
            if !guard_shell.is_invalid() && !IsWindow(guard_shell).as_bool() {guard_shell=HWND::default();}
            if guard_shell.is_invalid() {
                guard_shell=FindWindowW(PCWSTR(wide("Shell_TrayWnd").as_ptr()),PCWSTR::null()).unwrap_or_default();
            }
            // Publish state for the reorder hook; the 20 ms pass below stays as
            // a fallback in case the hook never fires on this shell build.
            GUARD_HWND.store(hwnd.0 as isize,std::sync::atomic::Ordering::Relaxed);
            GUARD_SHELL.store(guard_shell.0 as isize,std::sync::atomic::Ordering::Relaxed);
            GUARD_LEVEL.store(guard_level,std::sync::atomic::Ordering::Relaxed);
            if !guard_shell.is_invalid() {
                hud_phase(10);
                let t=Instant::now();
                let _=keep_above_shell(hwnd,guard_shell,guard_level);
                hud_slow("keep_above_shell（每 20ms 一次）",t);
            }
            if last_full.elapsed() < Duration::from_millis(250) { continue; }
            last_full=Instant::now();
            hud_phase(2);
            let t=Instant::now();
            let settings = store.get();
            hud_slow("store.get()",t);
            if hud_trace() { hud_trace_line(2); }
            let settings_changed=previous_settings.as_ref()!=Some(&settings);
            if settings_changed {
                last_layout=Instant::now()-Duration::from_secs(5);
                previous_settings=Some(settings.clone());
            }
            let attached; let mut message = "已关闭".to_string();
            if !settings.taskbar_enabled {
                if IsWindow(hwnd).as_bool() { let _ = DestroyWindow(hwnd); }
                hwnd=HWND::default(); parent=HWND::default();
            } else {
                hud_phase(3);
                let t=Instant::now();
                let shell = FindWindowW(PCWSTR(wide("Shell_TrayWnd").as_ptr()),PCWSTR::null()).unwrap_or_default();
                hud_slow("FindWindowW(Shell_TrayWnd)",t);
                guard_shell=shell;
                if shell.is_invalid() { message="等待 Windows 任务栏".into(); }
                else {
                    SetThreadDpiAwarenessContext(GetWindowDpiAwarenessContext(shell));
                    let mut bar = RECT::default();
                    if GetWindowRect(shell,&mut bar).is_err() {
                        let _=ShowWindow(hwnd,SW_HIDE);
                        continue;
                    }
                    let dpi=GetDpiForWindow(shell).max(96);
                    if (bar.left,bar.top,bar.right,bar.bottom,dpi)!=
                        (layout_bar.left,layout_bar.top,layout_bar.right,layout_bar.bottom,layout_dpi) {
                        layout_ok=false;
                        last_valid_layout=Instant::now()-Duration::from_secs(5);
                        last_layout=Instant::now()-Duration::from_secs(5);
                        layout_bar=bar; layout_dpi=dpi;
                    }
                    if shell != parent || !IsWindow(hwnd).as_bool() {
                        hud_phase(11);
                        let t=Instant::now();
                        if IsWindow(hwnd).as_bool() { let _ = DestroyWindow(hwnd); }
                        hwnd=CreateWindowExW(WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE|WS_EX_LAYERED,
                            PCWSTR(class.as_ptr()),PCWSTR(wide("WinMeter 任务栏监控").as_ptr()),WS_POPUP,
                            0,0,1,1,None,None,module,None).unwrap_or_default();
                        hud_slow("CreateWindowExW（重建监控条窗口）",t);
                        parent=shell; layout_ok=false;
                        guard_level=false;
                        last_valid_layout=Instant::now()-Duration::from_secs(5);
                        last_layout=Instant::now()-Duration::from_secs(5);
                    }
                    // The HUD is deliberately unowned: StartAllBack can hide
                    // Explorer-owned popups when it starts a context menu.
                    // Freeze before fullscreen/occlusion/layout checks, even if
                    // UIA momentarily loses its tree during menu tracking.
                    hud_phase(4);
                    let t=Instant::now();
                    let menu = menu_active(shell);
                    hud_slow("menu_active（EnumWindows + 菜单状态）",t);
                    if !settings_changed && menu && taskbar_present(shell,&bar) {
                        if layout_ok && !buttons.is_empty() {last_valid_layout=Instant::now();}
                        if let Ok(mut s)=status.lock() {
                            *s=HudStatus {attached:IsWindowVisible(hwnd).as_bool(),message:"菜单打开 · 保持监控位置".into()};
                        }
                        continue;
                    }
                    if last_layout.elapsed() >= Duration::from_secs(1) {
                        hud_phase(5);
                        let t=Instant::now();
                        if let Ok((rects,classic))=read_taskbar_layout(automation.as_ref(),shell) {
                            if !rects.is_empty() {
                                buttons=rects; classic_layout=classic;
                                last_valid_layout=Instant::now();
                            }
                        }
                        hud_slow("read_taskbar_layout（UIA FindAll 跨进程）",t);
                        // A transient UIA failure must not blink a valid overlay.
                        // Expire stale bounds quickly, and never reuse them across DPI/geometry changes.
                        layout_ok=last_valid_layout.elapsed()<Duration::from_secs(2);
                        last_layout=Instant::now();
                    }
                    // 提前算好，才能单独给这一步计时：它内部要问 shell 通知状态，
                    // 全屏判定还会枚举前台窗口，都是跨进程调用。
                    hud_phase(6);
                    let t=Instant::now();
                    let exposed=taskbar_exposed(shell,&bar);
                    hud_slow("taskbar_exposed（shell 状态 + 前台窗口）",t);
                    if !IsWindow(hwnd).as_bool() { message="任务栏窗口创建失败".into(); }
                    else if bar.bottom-bar.top > bar.right-bar.left { let _=ShowWindow(hwnd,SW_HIDE); hide_ticks=0; message="暂支持横向主任务栏".into(); }
                    else if !exposed { let _=ShowWindow(hwnd,SW_HIDE); hide_ticks=0; message="任务栏隐藏或全屏应用中".into(); }
                    else if !layout_ok {
                        hide_ticks+=1;
                        if hide_ticks>=3 {let _=ShowWindow(hwnd,SW_HIDE);}
                        message="等待任务栏按钮布局，避免遮挡程序".into();
                    }
                    else {
                        let t_draw=Instant::now();
                        let scale=GetDpiForWindow(shell).max(96) as f64/96.0;
                        let gap=(4.0*scale).ceil() as i32;
                        let tray=child_rect(shell,"TrayNotifyWnd");
                        let right=tray.map_or(bar.right-gap,|r|r.left-gap);
                        // Keep the selected side while trying compact variants.
                        let left=bar.left+gap;
                        let offset=(settings.taskbar_offset as f64*scale).round() as i32;
                        let blocked:Vec<_>=buttons.iter().filter(|r|r.top<bar.bottom&&r.bottom>bar.top).map(|r|(r.left,r.right)).collect();
                        let mut selected=None;
                        for (index,display) in display_variants(&settings).iter().enumerate() {
                            hud_phase(7);
                            let t=Instant::now();
                            if let Some(image)=surface(display,&values,scale,bar.bottom-bar.top) {
                                if let Some(x)=docked_slot(left,right,image.width,offset,settings.taskbar_position=="right",&blocked,gap) {
                                    hud_slow("surface（建 DIB + DrawText 位图）",t);
                                    selected=Some((image,x,index,display.taskbar_order.is_empty()));
                                    break;
                                }
                            }
                            hud_slow("surface（建 DIB + DrawText 位图）",t);
                        }
                        if let Some((image,x,variant,button))=selected {
                            hide_ticks=0;
                            // Let normal Windows Z ordering cover the HUD beneath
                            // flyouts. Point hit-testing here used to hide the
                            // entire strip during tray/popup activation races.
                            let geom=(x,bar.top,image.width,image.height);
                            // Repainting or repositioning an unchanged layered
                            // window makes DWM rebuild its surface and drops the
                            // overlay for a frame; on a 250 ms refresh that reads
                            // as a flicker. Only touch the window when the pixels
                            // or the geometry really changed.
                            let redraw=last_values.as_ref()!=Some(&values) || geom!=last_geom
                                || last_variant!=variant || settings_changed;
                            let mut drawn_ok=true;
                            if redraw {
                                hud_phase(8);
                                let t=Instant::now();
                                let position=POINT{x,y:bar.top}; let source=POINT{x:0,y:0}; let size=SIZE{cx:image.width,cy:image.height};
                                let blend=BLENDFUNCTION{BlendOp:AC_SRC_OVER as u8,BlendFlags:0,SourceConstantAlpha:255,AlphaFormat:AC_SRC_ALPHA as u8};
                                drawn_ok=UpdateLayeredWindow(hwnd,None,Some(&position),Some(&size),image.dc,Some(&source),COLORREF(0),Some(&blend),ULW_ALPHA).is_ok();
                                hud_slow("UpdateLayeredWindow（提交分层窗口）",t);
                                if drawn_ok {last_values=Some(values.clone()); last_variant=variant;}
                            }
                            hud_phase(9);
                            let t=Instant::now();
                            let shell_topmost=GetWindowLongPtrW(shell,GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0!=0;
                            let hud_topmost=(GetWindowLongPtrW(hwnd,GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0)!=0;
                            if hud_topmost!=shell_topmost {
                                // Follow the taskbar's layer once; from then on the
                                // reorder hook keeps us directly above it.
                                let _=SetWindowPos(hwnd,if shell_topmost {HWND_TOPMOST}else{HWND_NOTOPMOST},
                                    0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE|SWP_NOOWNERZORDER);
                                last_geom=(i32::MIN,i32::MIN,i32::MIN,i32::MIN);
                            } else if geom!=last_geom || !IsWindowVisible(hwnd).as_bool() {
                                // Z order belongs to keep_above_shell; never fight it here.
                                let _=SetWindowPos(hwnd,HWND_TOP,x,bar.top,image.width,image.height,
                                    SWP_NOACTIVATE|SWP_NOZORDER|SWP_SHOWWINDOW);
                                last_geom=geom;
                            }
                            hud_slow("SetWindowPos（分层/定位）",t);
                            // Both sides now share a layer, so the guard may act.
                            guard_level=true;
                            message=if !drawn_ok || !IsWindowVisible(hwnd).as_bool() {"任务栏绘制失败，正在重试".into()}
                                else if button {"空位较小 · 点击速览查看全部指标".into()}
                                else if variant>0 {"空间不足，已精简指标 · 字号与间距按设置 · 点击查看全部".into()}
                                else {"监控已显示 · 点击打开资源速览".into()};
                            if drawn_ok && IsWindowVisible(hwnd).as_bool() {
                                message=format!("{} · {}{}",message,if classic_layout {"经典任务栏 / "}else{""},
                                    if settings.taskbar_position=="right" {"图标右侧"}else{"靠左"});
                            }
                        } else {
                            // A click can briefly reorder the task list, leaving
                            // no free slot for one pass. Keep the last frame
                            // instead of dropping the overlay for that pass.
                            hide_ticks+=1;
                            if hide_ticks>=3 {let _=ShowWindow(hwnd,SW_HIDE);}
                            message=format!("{}所选侧没有可用空位，可点击托盘图标打开速览",if classic_layout {"经典任务栏 · "}else{""});
                        }
                        hud_slow("布局+绘制整段",t_draw);
                    }
                }
            }
            // Report what is actually on screen: a deferred hide keeps the last
            // frame visible, so the status follows the window, not the branch.
            attached=IsWindow(hwnd).as_bool() && IsWindowVisible(hwnd).as_bool();
            if let Ok(mut s)=status.lock() {*s=HudStatus{attached,message};}
        }
        if IsWindow(hwnd).as_bool() {let _=DestroyWindow(hwnd);}
    }
}
