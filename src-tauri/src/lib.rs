mod metrics;
mod settings;
mod taskbar;
mod autostart;
mod icons;
mod hardware;
mod hardware_service;
mod instance;
mod worker;
mod storage;
mod app_io;
mod fonts;
mod popup;

use std::{sync::Arc, time::{Duration, Instant}};
use tauri::{Emitter, Manager};

// 同步命令在 Tauri 里跑在主线程上，读注册表、写配置文件、查 SCM 这类
// 操作一旦慢下来就会冻住整个界面（鼠标悬停直接变忙碌光标）。
// 这几个都挪到阻塞线程池，主线程只负责窗口。
#[tauri::command]
async fn get_settings(app: tauri::AppHandle) -> Result<settings::Settings, String> {
    // 窗口比 setup 里的 manage() 更早存在，前端可能抢在注册之前调用。
    // 这里必须用 try_state 返回错误（前端会重试），不能用 state() —— 那个会 panic。
    let store = app
        .try_state::<Arc<settings::SettingsStore>>()
        .ok_or_else(|| "state not managed".to_string())?
        .inner()
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut settings = store.get();
        settings.launch_at_startup = autostart::enabled();
        settings
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_settings(settings: settings::Settings, app: tauri::AppHandle) -> Result<settings::Settings, String> {
    let store = app
        .try_state::<Arc<settings::SettingsStore>>()
        .ok_or_else(|| "state not managed".to_string())?
        .inner()
        .clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<settings::Settings, String> {
        settings.validate()?;
        let previous_startup = autostart::enabled();
        let startup_changed = settings.launch_at_startup != previous_startup;
        if startup_changed { autostart::set(settings.launch_at_startup)?; }
        match store.save(settings) {
            Ok(result) => Ok(result),
            Err(error) => {
                if startup_changed { let _ = autostart::set(previous_startup); }
                Err(error)
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Ok(saved) = &result { let _ = app.emit("settings-changed", saved); }
    result
}

#[tauri::command]
async fn get_system_fonts()->Result<Vec<String>,String>{
    tauri::async_runtime::spawn_blocking(fonts::installed).await.map_err(|e|e.to_string())?
}

#[tauri::command]
fn taskbar_status(hud: tauri::State<'_, Arc<taskbar::TaskbarHud>>) -> taskbar::HudStatus { hud.status() }

#[tauri::command]
fn refresh_network(){metrics::refresh_network();}

#[tauri::command]
async fn sensor_service_status()->Result<hardware_service::ServiceStatus,String> {
    // 查 SCM 在服务启动/停止期间会明显变慢，不能压在主线程上。
    tauri::async_runtime::spawn_blocking(hardware_service::status).await.map_err(|e|e.to_string())
}

#[tauri::command]
async fn manage_sensor_service(action:String)->Result<hardware_service::ServiceStatus,String>{
    tauri::async_runtime::spawn_blocking(move||hardware::manage_service(&action)).await.map_err(|e|e.to_string())?
}

#[tauri::command]
async fn enable_hardware() -> Result<hardware::Status,String> {
    tauri::async_runtime::spawn_blocking(hardware::enable).await.map_err(|e|e.to_string())?
}

#[tauri::command]
fn minimize_main(app: tauri::AppHandle, store: tauri::State<'_, Arc<settings::SettingsStore>>) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("主窗口不存在")?;
    if store.get().minimize_to_tray { window.hide() } else { window.minimize() }.map_err(|e| e.to_string())
}

/// 主窗口的「可信尺寸」下限（逻辑像素）。低于这个值说明 WINDOWPLACEMENT 的
/// 正常位置已经被最小化状态污染，必须重置，否则恢复出来就是一个小不点。
const SANE_MIN_W: f64 = 480.0;
const SANE_MIN_H: f64 = 360.0;
/// 配置里的默认窗口尺寸（逻辑像素），与 tauri.conf.json 保持一致。
const DEFAULT_W: f64 = 1120.0;
const DEFAULT_H: f64 = 740.0;

fn log_window(msg: &str) {
    let dir = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("WinMeter");
    let _ = std::fs::create_dir_all(&dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("window.log"))
    {
        use std::io::Write;
        let _ = writeln!(file, "[{stamp}] {msg}");
    }
}

/// 把主窗口拉回用户能看见的状态。
///
/// 这里踩过一个很隐蔽的坑：窗口**最小化**时 `outer_position()` / `outer_size()`
/// 返回的是「最小化图标」那个矩形（本机实测 158×26，位置 -18286），不是真实尺寸。
/// 旧代码拿它去判断「窗口是否整个跑到屏幕外」，于是永远判为真，接着调 `center()`
/// —— 而 `center()` 会把这个极小矩形当成正常尺寸写进 `WINDOWPLACEMENT.rcNormalPosition`。
/// 之后 `unminimize()` 按这个被污染的正常位置恢复，主界面就只剩屏幕中间一点点大。
///
/// 所以顺序必须是：先还原（还原前先藏起来，避免用户看到中间态），再测量，
/// 最后对尺寸和位置各做一次兜底修正。
fn show_main(app: &tauri::AppHandle, page: &str) {
    let Some(w) = app.get_webview_window("main") else { return };

    let minimized = w.is_minimized().unwrap_or(false);
    if minimized {
        // 最小化 + 隐藏的窗口用户本来也看不见，先藏住再还原可以彻底避免闪烁。
        let _ = w.hide();
        let _ = w.unminimize();
    }

    if let Ok(Some(monitor)) = w.current_monitor() {
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        let (ax, ay) = (area.position.x as f64, area.position.y as f64);
        // 工作区的逻辑尺寸：物理像素 ÷ 缩放比。
        let avail_w = area.size.width as f64 / scale;
        let avail_h = area.size.height as f64 / scale;
        // 小屏幕上默认尺寸可能装不下，按工作区 92% 收一收。
        let want_w = DEFAULT_W.min(avail_w * 0.92).max(SANE_MIN_W);
        let want_h = DEFAULT_H.min(avail_h * 0.92).max(SANE_MIN_H);

        // 最小尺寸也不能超过工作区，否则系统会把窗口压成一条缝。
        let min_w = 900.0_f64.min(avail_w * 0.92);
        let min_h = 620.0_f64.min(avail_h * 0.92);
        let _ = w.set_min_size(Some(tauri::LogicalSize::new(min_w, min_h)));

        let cur = w
            .outer_size()
            .ok()
            .map(|s| (s.width as f64 / scale, s.height as f64 / scale));
        let broken = match cur {
            None => true,
            Some((cw, ch)) => {
                cw < SANE_MIN_W || ch < SANE_MIN_H || cw > avail_w + 1.0 || ch > avail_h + 1.0
            }
        };

        if broken {
            let _ = w.set_size(tauri::LogicalSize::new(want_w, want_h));
            let x = ax / scale + (avail_w - want_w) / 2.0;
            let y = ay / scale + (avail_h - want_h) / 2.0;
            let _ = w.set_position(tauri::LogicalPosition::new(x, y));
            log_window(&format!(
                "主窗口尺寸异常 {:?}（缩放 {scale}），已恢复为 {want_w:.0}×{want_h:.0}",
                cur
            ));
        } else if let (Ok(pos), Some((cw, ch))) = (w.outer_position(), cur) {
            // 尺寸正常但整块落在工作区外（换过显示器 / 改过分辨率），才拉回中央。
            let rel_x = (pos.x as f64 - ax) / scale;
            let rel_y = (pos.y as f64 - ay) / scale;
            if rel_x + cw <= 0.0 || rel_y + ch <= 0.0 || rel_x >= avail_w || rel_y >= avail_h {
                let x = ax / scale + (avail_w - cw) / 2.0;
                let y = ay / scale + (avail_h - ch) / 2.0;
                let _ = w.set_position(tauri::LogicalPosition::new(x, y));
                log_window(&format!("主窗口在屏幕外 {:?}，已拉回中央", (rel_x, rel_y)));
            }
        }
    }

    let _ = w.show();
    let _ = w.unminimize();
    let _ = w.set_focus();
    let _ = w.emit("navigate", page);
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

/// 启动阶段的致命错误以前只会静悄悄退出，用户看到的就是「双击没反应」。
/// 这类问题（WebView2 建不出来、运行时找不到）必须说清楚，否则没法排查。
fn show_fatal_dialog(message: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_SETFOREGROUND, MB_TOPMOST};
    let text = wide(message);
    let caption = wide("WinMeter 启动失败");
    unsafe {
        MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(caption.as_ptr()),
            MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST);
    }
}

/// 把 panic 写进 `%APPDATA%\WinMeter\crash.log`。日志只追加，超过 256 KB 时/// 轮转成 `.old`，避免长期运行把磁盘写满。
fn install_panic_log() {
    let dir = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("WinMeter");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("crash.log");
    std::panic::set_hook(Box::new(move |info| {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let body = format!("=== panic @ unix {stamp} ===\n{info}\n\n");
        if std::fs::metadata(&path).map(|m| m.len() > 262_144).unwrap_or(false) {
            let _ = std::fs::rename(&path, path.with_extension("log.old"));
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            let _ = file.write_all(body.as_bytes());
        }
        eprintln!("{body}");
        // 只有「起不来」这一类才弹框。HUD、Z 序守护里的 panic 是被 catch_unwind
        // 兜住的，弹框会变成每次偶发都糊用户一脸。
        let text = info.to_string();
        if text.contains("Failed to setup app") || text.contains("webview") {
            show_fatal_dialog(&format!(
                "WinMeter 无法启动：WebView2 运行环境有问题。\n\n{}\n\n\
                 处理办法：安装或修复「Microsoft Edge WebView2 Runtime」后重试。\n\
                 详细日志：{}",
                text.lines().next().unwrap_or(""), path.display()
            ));
        }
    }));
}

/// 选一个 WebView2 运行时，挑版本最高的。
///
/// 背景：Evergreen Runtime 默认只认注册表里那一个，scoop 之类解压安装的
/// 不会被识别；而 `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` 一旦被设成某个旧版本，
/// 程序就会一直跑那个旧版本，哪怕系统里已经装了更新的。
///
/// 实测教训：本机 scoop 的 155.0.4283.62 在 `--disable-gpu` 下仍会拉起一个
/// GPU 进程，该进程反复崩溃，累计三次后 Chromium 主动崩掉整个 browser 进程
/// （日志 `GPU process isn't usable. Goodbye.`），窗口还在但内容消失。
/// 系统里的 156.0.4314.42 没有这个问题。所以这里不再无条件信任旧的环境变量。
///
/// 优先级：`WINMETER_WEBVIEW_RUNTIME`（本项目专用，显式钉住，永远优先）
/// → 版本最高的那个 → `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` 里有效的那个。
/// 必须在创建任何 WebView 之前调用。
fn prefer_newest_webview_runtime() {
    use std::path::PathBuf;

    let dir_log = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("WinMeter");
    let _ = std::fs::create_dir_all(&dir_log);
    let log_path = dir_log.join("runtime.log");

    fn version_of(name: &str) -> Option<Vec<u32>> {
        let parts: Vec<&str> = name.split('.').collect();
        if parts.len() < 3 {
            return None;
        }
        parts.iter().map(|p| p.parse::<u32>().ok()).collect()
    }

    /// 目录名可能叫 `current`（scoop 的软链），解引用后按真实目录名取版本。
    fn version_of_dir(dir: &PathBuf) -> Option<Vec<u32>> {
        if let Some(v) = dir.file_name().and_then(|n| n.to_str()).and_then(version_of) {
            return Some(v);
        }
        let real = std::fs::canonicalize(dir).ok()?;
        real.file_name().and_then(|n| n.to_str()).and_then(version_of)
    }

    fn usable(dir: &PathBuf) -> bool {
        dir.join("msedgewebview2.exe").is_file()
    }

    let shown = |v: &Vec<u32>| v.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(".");

    // 显式钉住：用户明确指定就用它，不做版本比较。
    if let Some(raw) = std::env::var_os("WINMETER_WEBVIEW_RUNTIME") {
        let path = PathBuf::from(&raw);
        if usable(&path) {
            let _ = std::fs::write(
                &log_path,
                format!(
                    "WebView2 runtime: {}\npath: {}\n来源: WINMETER_WEBVIEW_RUNTIME（显式指定）\n",
                    version_of_dir(&path).as_ref().map(shown).unwrap_or_else(|| "未知版本".into()),
                    path.display()
                ),
            );
            std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", path);
            return;
        }
    }

    let mut roots: Vec<PathBuf> = vec![
        PathBuf::from(r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application"),
        PathBuf::from(r"C:\Program Files\Microsoft\EdgeWebView\Application"),
    ];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join(r"Microsoft\EdgeWebView\Application"));
    }
    if let Some(scoop) = std::env::var_os("SCOOP") {
        roots.push(PathBuf::from(scoop).join(r"apps\webview2"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        roots.push(PathBuf::from(home).join(r"scoop\apps\webview2"));
    }

    let inherited = std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").map(PathBuf::from);

    // 环境变量指向的运行时有效就沿用它。
    //
    // 这里曾经改成「一律选版本最高的」，但实测发现：本机 scoop 的 155 在软件渲染下
    // 能正常出画面，系统里的 156 无论硬件还是软件渲染都拿不到任何一帧
    // （CDP 三种截图方式全部超时，页面 DOM 却活着）。版本号更高不等于可用，
    // 而用户显式设过的路径往往是他验证过能用的那个，所以恢复成沿用。
    // 想强制换版本用 WINMETER_WEBVIEW_RUNTIME。
    if let Some(dir) = &inherited {
        if usable(dir) {
            let text = format!(
                "WebView2 runtime: {}\npath: {}\n来源: 环境变量 WEBVIEW2_BROWSER_EXECUTABLE_FOLDER\n",
                version_of_dir(dir).as_ref().map(shown).unwrap_or_else(|| "未知版本".into()),
                dir.display()
            );
            let _ = std::fs::write(&log_path, text);
            return;
        }
    }

    // 走到这里说明环境变量要么没设，要么指向的目录里根本没有
    // `msedgewebview2.exe`——典型情况是装过便携版又卸载了，变量没跟着清。
    // 必须把它删掉：留着的话 WebView2 会照着这个路径去找运行时，直接报
    // 「Could not find the webview runtime」，程序在 setup 阶段就 panic，
    // 用户看到的是双击没反应。删掉之后才会回退到系统注册的 Evergreen 运行时。
    if inherited.is_some() {
        std::env::remove_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER");
    }

    let mut candidates: Vec<(Vec<u32>, PathBuf)> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !usable(&dir) {
                continue;
            }
            // 版本号取自目录名；`current` 这类软链靠解引用拿到真实版本
            let Some(version) = version_of_dir(&dir) else { continue };
            if !candidates.iter().any(|(_, d)| d == &dir) {
                candidates.push((version, dir));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    let Some((version, dir)) = candidates.first() else {
        let _ = std::fs::write(
            &log_path,
            "未找到任何 WebView2 运行时。\n已清除无效的 WEBVIEW2_BROWSER_EXECUTABLE_FOLDER，改由 WebView2 自己找系统运行时。\n",
        );
        return;
    };

    let mut text = format!(
        "WebView2 runtime: {}\npath: {}\n来源: 自动扫描（环境变量未设置或指向无效路径）\n\n候选（按版本从高到低）：\n",
        shown(version),
        dir.display()
    );
    for (v, d) in &candidates {
        text.push_str(&format!("  {}  {}\n", shown(v), d.display()));
    }
    if let Some(old) = &inherited {
        if old != dir {
            text.push_str(&format!(
                "\n注意：环境变量指向的 {} 里没有 msedgewebview2.exe，已忽略。\n",
                old.display()
            ));
        }
    }
    let _ = std::fs::write(&log_path, text);

    std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", dir);
}

pub fn run() {
    // release 构建没有控制台，panic 信息默认直接丢失。先把现场落到文件里，
    // 下次再出问题就能看到是哪一行、什么原因，而不是只有「程序没了」。
    install_panic_log();
    // 必须在任何 WebView 创建之前执行。
    prefer_newest_webview_runtime();
    let owner=match instance::Instance::acquire(){
        Ok(Some(owner))=>owner,Ok(None)=>return,
        Err(error)=>{eprintln!("无法取得 WinMeter 实例锁：{error}");return;}
    };
    let active_owner=owner.clone();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_settings, save_settings, get_system_fonts, taskbar_status, minimize_main, enable_hardware, sensor_service_status, manage_sensor_service, refresh_network, popup::get_popup_snapshot, popup::close_popup, popup::open_main])
        .on_window_event(|window, event| {
            if window.label()=="popup" {
                if matches!(event,tauri::WindowEvent::Focused(false)) {popup::dismiss(window.app_handle());}
                if let tauri::WindowEvent::CloseRequested {api,..}=event {
                    api.prevent_close(); popup::dismiss(window.app_handle());
                }
                return;
            }
            // 窗口事件可能早于 setup 里的 manage()，这里同样不能用 state()。
            let settings = window
                .try_state::<Arc<settings::SettingsStore>>()
                .map(|state| state.get());
            if matches!(event, tauri::WindowEvent::Resized(_))
                && settings.as_ref().is_some_and(|s| s.minimize_to_tray)
                && window.is_minimized().unwrap_or(false) { let _ = window.hide(); }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if settings.as_ref().map_or(true, |s| s.close_to_tray) {
                    api.prevent_close(); let _ = window.hide();
                } else {
                    // The hidden popup is also a Tauri window. Closing only
                    // main would leave an invisible single-instance process.
                    api.prevent_close(); window.app_handle().exit(0);
                }
            }
        })
        .setup(move |app| {
            let store = Arc::new(settings::SettingsStore::load(app.path().config_dir()?.join("WinMeter").join("settings.json")));
            app.manage(store.clone());
            app.manage(popup::PopupState::default());
            let handle = app.handle().clone();
            let activate=handle.clone();active_owner.listen(move||show_main(&activate,"overview"));
            hardware::init(app.path().resource_dir()?,app.path().app_local_data_dir()?.join("sensors"));
            let hud = Arc::new(taskbar::TaskbarHud::spawn(handle.clone(), store.clone()));
            app.manage(hud.clone());
            if store.get().hardware_auto_admin {
                std::thread::spawn(|| { let _ = hardware::enable(); });
            }
            if let Some(window)=app.get_webview_window("main") {window.set_icon(icons::application())?;}
            if std::env::args().any(|arg|arg=="--tray") {
                if let Some(window)=app.get_webview_window("main") {let _=window.hide();}
            }
            use tauri::{menu::{Menu, MenuItem}, tray::{TrayIconBuilder, TrayIconEvent, MouseButton, MouseButtonState}};
            let open = MenuItem::with_id(app, "open", "打开 WinMeter", true, None::<&str>)?;
            let prefs = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
            let exit = MenuItem::with_id(app, "exit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &prefs, &exit])?;
            let tray_dark=taskbar::system_dark();
            let tray = TrayIconBuilder::with_id("main-tray").tooltip("WinMeter · 系统资源监控")
                .icon(icons::tray(tray_dark))
                .menu(&menu).show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main(app, "overview"), "settings" => show_main(app, "settings"),
                    "exit" => app.exit(0), _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, position, .. }=event {
                        popup::toggle(tray.app_handle(),position.x,position.y);
                    }
                });
            tray.build(app)?;
            // 采样耗时计入间隔，避免每次采集后额外再等一个完整周期。
            let sampling=worker::SamplingWorker::spawn(move |stop| {
                let mut collector = metrics::new_collector();
                let mut previous_dark=tray_dark;
                loop {
                    if stop.try_recv().is_ok(){break;}
                    let started = Instant::now();
                    let dark=taskbar::system_dark();
                    let recolor=dark!=previous_dark;
                    let sample = metrics::collect(&mut collector);
                    hud.update(&sample);
                    handle.state::<popup::PopupState>().record(&sample);
                    // Tray APIs wait for the UI thread; never call them from a
                    // worker that the UI thread joins during shutdown.
                    let ui=handle.clone();
                    if handle.run_on_main_thread(move||{
                        if recolor {if let Some(tray)=ui.tray_by_id("main-tray"){let _=tray.set_icon(Some(icons::tray(dark)));}}
                        let _=ui.emit("sample",sample);
                    }).is_err(){break;}
                    previous_dark=dark;
                    let interval = Duration::from_millis(store.get().sample_interval_ms);
                    if stop.recv_timeout(interval.saturating_sub(started.elapsed())).is_ok(){break;}
                }
                if let Err(error)=collector.flush_history(){eprintln!("保存退出历史失败：{error}");}
            });
            app.manage(sampling);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                owner.stop();
                taskbar::shutdown();
                if let Some(worker)=app.try_state::<worker::SamplingWorker>(){worker.shutdown();}
                hardware::shutdown();
            }
        });
}
