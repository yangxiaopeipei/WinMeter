//! Native taskbar monitor: transparent overlay using existing taskbar space only.
use crate::{metrics::Sample, settings::{Settings, SettingsStore}};
use serde::Serialize;
use std::sync::{Arc, Mutex, mpsc};

#[derive(Clone, Default, Serialize)]
pub struct HudStatus {
    pub attached: bool,
    pub message: String,
}

#[derive(Clone, Default, PartialEq)]
struct Values { down: f64, up: f64, cpu: f64, memory: u32, cpu_temp: Option<f64>, gpu_temp: Option<f64>, disk_temp: Option<f64> }

pub struct TaskbarHud {
    sender: mpsc::SyncSender<Values>,
    status: Arc<Mutex<HudStatus>>,
}

pub fn shutdown() {
    #[cfg(windows)] native::shutdown();
}
pub fn system_dark() -> bool {
    #[cfg(windows)] {native::system_dark()}
    #[cfg(not(windows))] {false}
}

impl TaskbarHud {
    pub fn spawn(app: tauri::AppHandle, store: Arc<SettingsStore>) -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        let status = Arc::new(Mutex::new(HudStatus::default()));
        let shared = status.clone();
        let for_panic = status.clone();
        std::thread::spawn(move || {
            // HUD 线程自己出问题不该让整个程序消失；记下原因，状态栏会如实显示。
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                native::run(app, store, receiver, shared)
            }));
            if result.is_err() {
                if let Ok(mut s) = for_panic.lock() {
                    *s = HudStatus { attached: false, message: "任务栏监控已停止（内部错误）".into() };
                }
            }
        });
        Self { sender, status }
    }
    pub fn update(&self, sample: &Sample) {
        let _ = self.sender.try_send(Values {
            down: sample.net_down_bps, up: sample.net_up_bps,
            cpu: sample.cpu_usage, memory: sample.mem_load,
            cpu_temp: sample.cpu_temp_c, gpu_temp: sample.gpu_temp_c, disk_temp: sample.disk_temp_c,
        });
    }
    pub fn status(&self) -> HudStatus {
        self.status.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

fn temperature(value: Option<f64>) -> String {
    value.filter(|v| v.is_finite()).map(|v| format!("{v:.0} ℃")).unwrap_or_else(|| "—".into())
}

fn rate(bps: f64) -> String {
    let mut value=if bps.is_finite(){bps.max(0.0)}else{0.0};
    let units=["B/s","KB/s","MB/s","GB/s","TB/s"];
    let mut unit=0;
    while value>=1024.0 && unit<units.len()-1 {value/=1024.0;unit+=1;}
    if value>=1000.0 && unit==units.len()-1 {return "999+ TB/s".into();}
    if unit>=2 && value<100.0 {format!("{value:.1} {}",units[unit])}
    else {format!("{value:.0} {}",units[unit])}
}

fn transient_shell_surface(class:&str)->bool {
    matches!(class,"#32768"|"Xaml_WindowedPopupClass"|"TopLevelWindowForOverflowXamlIsland"|"TaskListThumbnailWnd"|
        "NotifyIconOverflowWindow"|"ControlCenterWindow")
}

fn shell_host_image(path:&str)->bool {
    matches!(path.rsplit(['\\','/']).next().unwrap_or("").to_ascii_lowercase().as_str(),
        "explorer.exe"|"shellexperiencehost.exe"|"startmenuexperiencehost.exe"|"searchhost.exe"|"searchapp.exe"|"textinputhost.exe")
}

fn fullscreen_app(class: &str, window: [i32; 4], monitor: [i32; 4]) -> bool {
    !matches!(class, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "WinMeterTaskbarHud")
        && !transient_shell_surface(class)
        && window[0] <= monitor[0] && window[1] <= monitor[1]
        && window[2] >= monitor[2] && window[3] >= monitor[3]
}

// Find the closest safe interval, rather than placing text over application buttons.
fn free_slot(left: i32, right: i32, width: i32, preferred: i32, blocked: &[(i32, i32)], gap: i32) -> Option<i32> {
    if width <= 0 || right - left < width { return None; }
    let mut occupied: Vec<_> = blocked.iter().map(|&(a,b)| ((a-gap).max(left),(b+gap).min(right)))
        .filter(|&(a,b)| b > a).collect();
    occupied.sort_unstable();
    let mut cursor = left;
    let mut candidates = Vec::new();
    for (a,b) in occupied.into_iter().chain(Some((right,right))) {
        if a - cursor >= width { candidates.push(preferred.clamp(cursor,a-width)); }
        cursor = cursor.max(b);
    }
    candidates.into_iter().min_by_key(|&x| (x as i64 - preferred as i64).abs())
}

fn docked_slot(left:i32,right:i32,width:i32,offset:i32,on_right:bool,blocked:&[(i32,i32)],gap:i32)->Option<i32> {
    // Exhaust reduced-metric variants on the requested side instead of choosing a
    // wide free interval to the left of centered application icons.
    let start=if on_right {blocked.iter().filter(|&&(a,b)|a<right && b>left)
        .map(|&(_,b)|b.min(right)).max().unwrap_or(left).max(left)}else{left};
    let preferred=if on_right {right-width-offset}else{left+offset};
    free_slot(start,right,width,preferred,blocked,gap)
}

// Temporary display choices only: never alter the configured typography.
// Keep network/CPU longest; details remain available in the popup. An empty
// order requests a small, labeled popup button as the final fallback.
fn display_variants(settings: &Settings) -> Vec<Settings> {
    let mut variants=vec![settings.clone()];
    let mut compact=settings.clone();
    compact.taskbar_order.retain(|key|match key.as_str() {
        "network"=>settings.taskbar_network,"cpu"=>settings.taskbar_cpu,"memory"=>settings.taskbar_memory,
        "cpu_temp"=>settings.taskbar_cpu_temp,"gpu_temp"=>settings.taskbar_gpu_temp,
        "disk_temp"=>settings.taskbar_disk_temp,_=>false,
    });
    variants.push(compact.clone());
    for key in ["disk_temp","gpu_temp","cpu_temp","memory","cpu","network"] {
        if compact.taskbar_order.len()>1 && compact.taskbar_order.iter().any(|k|k==key) {
            compact.taskbar_order.retain(|k|k!=key);
            variants.push(compact.clone());
        }
    }
    compact.taskbar_order.clear();
    variants.push(compact);
    variants
}

fn text_alpha(pixels: &mut [u8], color: u8) {
    for px in pixels.chunks_exact_mut(4) {
        let alpha = px[0].max(px[1]).max(px[2]);
        let value = (color as u16 * alpha as u16 / 255) as u8;
        px.copy_from_slice(&[value,value,value,alpha]);
    }
}

#[cfg(windows)]
#[path = "taskbar_native.rs"]
mod native;

#[cfg(not(windows))]
mod native {
    use super::*;
    pub fn run(_: tauri::AppHandle, _: Arc<SettingsStore>, _: mpsc::Receiver<Values>, status: Arc<Mutex<HudStatus>>) {
        *status.lock().unwrap() = HudStatus { attached: false, message: "任务栏显示仅支持 Windows".into() };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn temperature_distinguishes_missing_and_invalid_readings() {
        assert_eq!(super::temperature(Some(57.6)), "58 ℃");
        assert_eq!(super::temperature(Some(0.0)), "0 ℃");
        assert_eq!(super::temperature(None), "—");
        assert_eq!(super::temperature(Some(f64::NAN)), "—");
        assert_eq!(super::temperature(Some(f64::INFINITY)), "—");
    }
    #[test]
    fn right_slot_docks_before_tray_and_avoids_application_buttons() {
        assert_eq!(super::free_slot(700,1170,140,1030,&[(500,950)],4),Some(1030));
        assert_eq!(super::free_slot(700,1170,140,1030,&[(500,1050)],4),None);
        assert_eq!(super::free_slot(-1000,0,150,-150,&[(-400,-100)],4),Some(-554));
    }
    #[test]
    fn centered_icons_are_avoided_without_requesting_more_space() {
        let icons=[(650,1270)];
        assert_eq!(super::free_slot(960,1770,300,1470,&icons,7),Some(1470));
        assert_eq!(super::free_slot(960,1500,300,1200,&icons,7),None);
        assert_eq!(super::free_slot(7,1770,300,7,&icons,7),Some(7));
        // Same geometry translated onto a monitor left of the primary display.
        assert_eq!(super::free_slot(-960,-150,300,-450,&[(-1270,-650)],7),Some(-450));
    }
    #[test]
    fn tight_taskbar_keeps_a_clickable_entry_without_changing_saved_settings() {
        let mut settings=super::Settings::default();
        settings.taskbar_cpu_temp=true; settings.taskbar_gpu_temp=true; settings.taskbar_gap=12;
        let original=settings.clone();
        let variants=super::display_variants(&settings);
        assert_eq!(settings,original);
        assert_eq!(variants[0],original);
        assert_eq!(variants[1].taskbar_order.len(),5);
        assert_eq!(variants[variants.len()-2].taskbar_order,vec!["network"]);
        assert!(variants.last().unwrap().taskbar_order.is_empty());
        let blocked=[(0,1400),(1640,1800)];
        assert_eq!(super::free_slot(900,1800,650,1150,&blocked,4),None);
        assert!(super::free_slot(900,1800,120,1680,&blocked,4).is_some());
        assert!(super::free_slot(900,1800,30,1770,&[(0,1760)],4).is_some());
    }
    #[test]
    fn compact_fallback_does_not_enable_unselected_metrics() {
        let mut settings=super::Settings::default();
        settings.taskbar_network=false; settings.taskbar_cpu=false; settings.taskbar_memory=false;
        settings.taskbar_gpu_temp=true;
        let variants=super::display_variants(&settings);
        assert_eq!(variants[1].taskbar_order,vec!["gpu_temp"]);
        assert!(variants.last().unwrap().taskbar_order.is_empty());
    }
    #[test]
    fn every_space_fallback_preserves_the_configured_typography() {
        let mut settings=super::Settings::default();
        settings.taskbar_font="Microsoft YaHei UI".into(); settings.taskbar_font_size=16;
        settings.taskbar_gap=13; settings.taskbar_text_align="center".into();
        settings.taskbar_theme="light".into();
        for variant in super::display_variants(&settings) {
            assert_eq!(variant.taskbar_font,settings.taskbar_font);
            assert_eq!(variant.taskbar_font_size,16);
            assert_eq!(variant.taskbar_gap,13);
            assert_eq!(variant.taskbar_text_align,"center");
            assert_eq!(variant.taskbar_theme,"light");
        }
    }
    #[test]
    fn right_docking_compacts_before_considering_space_left_of_centered_icons() {
        let blocked=[(640,1270)];
        assert_eq!(super::docked_slot(7,1500,600,0,true,&blocked,7),None);
        assert_eq!(super::docked_slot(7,1500,180,0,true,&blocked,7),Some(1320));
        assert_eq!(super::docked_slot(7,1500,600,0,false,&blocked,7),Some(7));
        assert_eq!(super::docked_slot(-1913,-420,180,0,true,&[(-1280,-650)],7),Some(-600));
        assert_eq!(super::docked_slot(7,1500,30,0,true,&[(640,1480)],7),None);
    }
    #[test]
    fn alpha_background_is_fully_transparent_and_text_is_premultiplied() {
        let mut pixels=[0,0,0,255,128,128,128,0,255,255,255,0];
        super::text_alpha(&mut pixels,38);
        assert_eq!(&pixels[0..4],&[0,0,0,0]);
        assert_eq!(&pixels[4..8],&[19,19,19,128]);
        assert_eq!(&pixels[8..12],&[38,38,38,255]);
    }
    #[test]
    fn desktop_and_maximized_windows_do_not_hide_taskbar_monitor() {
        let monitor = [-1920, 0, 0, 1080];
        assert!(!super::fullscreen_app("Progman", monitor, monitor));
        assert!(!super::fullscreen_app("WorkerW", monitor, monitor));
        assert!(!super::fullscreen_app("App", [-1920, 0, 0, 1032], monitor));
        assert!(super::fullscreen_app("Game", monitor, monitor));
        for class in ["#32768","Xaml_WindowedPopupClass","TopLevelWindowForOverflowXamlIsland","TaskListThumbnailWnd"] {
            assert!(!super::fullscreen_app(class,monitor,monitor));
        }
        assert!(!super::fullscreen_app("NotifyIconOverflowWindow",monitor,monitor));
        // UWP games can share CoreWindow with shell flyouts: only the shell
        // process identity may exempt those windows from fullscreen handling.
        assert!(super::fullscreen_app("Windows.UI.Core.CoreWindow",monitor,monitor));
        assert!(super::shell_host_image(r"C:\Windows\SystemApps\ShellExperienceHost.exe"));
        assert!(!super::shell_host_image(r"C:\Games\Game.exe"));
    }
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn inspect_taskbar() {
        use windows::{core::PCWSTR, Win32::{Foundation::{BOOL, HWND, LPARAM, POINT, RECT}, UI::WindowsAndMessaging::*}};
        unsafe extern "system" fn visit(hwnd: HWND, _: LPARAM) -> BOOL {
            let mut class = [0u16; 256]; let n = GetClassNameW(hwnd, &mut class);
            let mut title = [0u16; 256]; let len = GetWindowTextW(hwnd, &mut title);
            let mut rect = RECT::default(); let _ = GetWindowRect(hwnd, &mut rect);
            let name = String::from_utf16_lossy(&class[..n.max(0) as usize]);
            println!("class={} title={} rect={},{},{},{} visible={}", name,
                String::from_utf16_lossy(&title[..len.max(0) as usize]),rect.left,rect.top,rect.right,rect.bottom,IsWindowVisible(hwnd).as_bool());
            if name == "WinMeterTaskbarHud" {
                let hit = WindowFromPoint(POINT{x:(rect.left+rect.right)/2,y:(rect.top+rect.bottom)/2});
                let mut hitclass = [0u16;256]; let n = GetClassNameW(hit,&mut hitclass);
                println!("HUD center topmost class={} self={}",String::from_utf16_lossy(&hitclass[..n.max(0) as usize]), hit == hwnd);
                println!("HUD style={:#x} exstyle={:#x}",GetWindowLongPtrW(hwnd,GWL_STYLE),GetWindowLongPtrW(hwnd,GWL_EXSTYLE));
            }
            BOOL(1)
        }
        unsafe extern "system" fn root(hwnd: HWND, lp: LPARAM) -> BOOL {
            let mut class = [0u16;256]; let n = GetClassNameW(hwnd,&mut class);
            if String::from_utf16_lossy(&class[..n.max(0) as usize]) == "WinMeterTaskbarHud" { let _ = visit(hwnd,lp); }
            BOOL(1)
        }
        unsafe {
            let class: Vec<u16> = "Shell_TrayWnd".encode_utf16().chain(Some(0)).collect();
            let shell = FindWindowW(PCWSTR(class.as_ptr()),PCWSTR::null()).unwrap();
            let _ = visit(shell,LPARAM(0));
            let _ = EnumChildWindows(shell,Some(visit),LPARAM(0));
            let _ = EnumWindows(Some(root),LPARAM(0));
            use windows::Win32::{System::Com::*, UI::Accessibility::*, UI::HiDpi::*};
            let _ = CoInitializeEx(None,COINIT_MULTITHREADED);
            SetThreadDpiAwarenessContext(GetWindowDpiAwarenessContext(shell));
            let uia: IUIAutomation=CoCreateInstance(&CUIAutomation,None,CLSCTX_INPROC_SERVER).unwrap();
            let buttons=super::native::taskbar_buttons(&uia,shell).unwrap();
            for r in &buttons { println!("button rect={},{},{},{}",r.left,r.top,r.right,r.bottom); }
            assert!(!buttons.is_empty(),"Taskbar exposes no button geometry");
        }
    }
    #[test]
    fn speed_uses_readable_units() {
        assert_eq!(super::rate(0.0), "0 B/s");
        assert_eq!(super::rate(2048.0), "2 KB/s");
        assert_eq!(super::rate(1572864.0), "1.5 MB/s");
    }
}
