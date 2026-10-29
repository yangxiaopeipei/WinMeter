use crate::metrics::Sample;
use serde::Serialize;
use std::{collections::VecDeque, sync::Mutex, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use tauri::{Manager, PhysicalPosition, PhysicalSize};

#[derive(Clone, Serialize)]
pub struct Point {
    at: u64, cpu: f64, gpu: f64, memory: u32,
    read: f64, write: f64, down: f64, up: f64,
}
#[derive(Clone, Default, Serialize)]
pub struct Snapshot { sample: Option<Sample>, history: VecDeque<Point> }
#[derive(Default)]
pub struct PopupState {
    snapshot: Mutex<Snapshot>,
    dismissed: Mutex<Option<Instant>>,
}
impl PopupState {
    pub fn record(&self, sample: &Sample) {
        let mut data=self.snapshot.lock().unwrap_or_else(|p|p.into_inner());
        let at=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
        data.history.push_back(Point {at,cpu:sample.cpu_usage,gpu:sample.gpu_usage,memory:sample.mem_load,
            read:sample.disk_read_bps,write:sample.disk_write_bps,down:sample.net_down_bps,up:sample.net_up_bps});
        while data.history.front().is_some_and(|p|at.saturating_sub(p.at)>120_000) || data.history.len()>121 {
            data.history.pop_front();
        }
        data.sample=Some(sample.clone());
    }
}

#[tauri::command]
pub fn get_popup_snapshot(state: tauri::State<'_, PopupState>) -> Snapshot {
    state.snapshot.lock().unwrap_or_else(|p|p.into_inner()).clone()
}

pub fn dismiss(app: &tauri::AppHandle) {
    if let Some(window)=app.get_webview_window("popup") {let _=window.hide();}
    if let Some(state)=app.try_state::<PopupState>() {
        *state.dismissed.lock().unwrap_or_else(|p|p.into_inner())=Some(Instant::now());
    }
}

#[tauri::command]
pub fn close_popup(app: tauri::AppHandle) { dismiss(&app); }

#[tauri::command]
pub fn open_main(app: tauri::AppHandle, page: String) -> Result<(),String> {
    if !["overview","cpu","gpu","mem","disk","net","battery","info","settings"].contains(&page.as_str()) {
        return Err("未知页面".into());
    }
    dismiss(&app);
    crate::show_main(&app,&page);
    Ok(())
}

// Physical coordinates throughout: work area excludes the taskbar, including
// top/side taskbars and monitors to the left of the primary display.
fn placement(work: [i32;4], anchor: [i32;2], scale: f64) -> [i32;4] {
    let margin=(8.0*scale).round() as i32;
    let available_w=(work[2]-work[0]-margin*2).max(1);
    let available_h=(work[3]-work[1]-margin*2).max(1);
    let width=((1280.0*scale).round() as i32).min(available_w);
    let logical_width=width as f64/scale;
    let desired_h=if logical_width>=1050.0 {570.0} else {710.0};
    let height=((desired_h*scale).round() as i32).min(available_h);
    let left=work[0]+margin; let top=work[1]+margin;
    let x=(anchor[0]-width/2).clamp(left,(work[2]-margin-width).max(left));
    let y=if anchor[1] < work[1]+(work[3]-work[1])/2 {top}
        else {(work[3]-margin-height).max(top)};
    [x,y,width,height]
}

pub fn toggle(app: &tauri::AppHandle, x: f64, y: f64) {
    let Some(window)=app.get_webview_window("popup") else {return;};
    if window.is_visible().unwrap_or(false) {dismiss(app);return;}
    // Tray focus loss happens on mouse-down, before its click-up callback.
    if app.state::<PopupState>().dismissed.lock().unwrap_or_else(|p|p.into_inner())
        .is_some_and(|at|at.elapsed()<Duration::from_millis(250)) {return;}
    let monitor=app.monitor_from_point(x,y).ok().flatten()
        .or_else(||window.primary_monitor().ok().flatten());
    if let Some(monitor)=monitor {
        let work=monitor.work_area();
        let r=placement([work.position.x,work.position.y,
            work.position.x+work.size.width as i32,work.position.y+work.size.height as i32],
            [x as i32,y as i32],monitor.scale_factor());
        let _=window.set_size(PhysicalSize::new(r[2] as u32,r[3] as u32));
        let _=window.set_position(PhysicalPosition::new(r[0],r[1]));
    }
    let _=window.show();
    let _=window.set_focus();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn popup_stays_in_work_area_at_screen_edges_and_scaled_negative_monitors() {
        for (work,scale) in [([0,0,1920,1032],1.0),([-2560,-200,0,1192],1.75),([0,48,1280,720],1.25)] {
            for anchor in [[work[0],work[1]-20],[work[2],work[3]+20]] {
                let [x,y,w,h]=placement(work,anchor,scale);
                assert!(x>=work[0] && y>=work[1]);
                assert!(w>0 && h>0 && x+w<=work[2] && y+h<=work[3]);
            }
        }
        assert_eq!(placement([0,48,1920,1080],[1700,24],1.0)[1],56);
        assert_eq!(placement([0,0,1920,1032],[1900,1060],1.0)[1],454);
    }
}
