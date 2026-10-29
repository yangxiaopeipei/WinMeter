use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Mutex};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub font_family: String,
    pub font_size: u32,
    pub sample_interval_ms: u64,
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
    pub launch_at_startup: bool,
    pub hardware_auto_admin: bool,
    pub taskbar_enabled: bool,
    pub taskbar_network: bool,
    pub taskbar_cpu: bool,
    pub taskbar_memory: bool,
    pub taskbar_cpu_temp: bool,
    pub taskbar_gpu_temp: bool,
    pub taskbar_disk_temp: bool,
    pub taskbar_text_align: String,
    pub taskbar_font: String,
    pub taskbar_font_size: u32,
    pub taskbar_position: String,
    pub taskbar_offset: i32,
    pub taskbar_theme: String,
    pub taskbar_gap: u32,
    pub taskbar_order: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(), font_family: "Segoe UI".into(), font_size: 13,
            sample_interval_ms: 1000, close_to_tray: true, minimize_to_tray: true, launch_at_startup: false, hardware_auto_admin: false,
            taskbar_enabled: false, taskbar_network: true, taskbar_cpu: true,
            taskbar_memory: true, taskbar_cpu_temp: false, taskbar_gpu_temp: false,
            taskbar_disk_temp: false, taskbar_text_align: "left".into(), taskbar_font: "Segoe UI".into(), taskbar_font_size: 12,
            taskbar_position: "right".into(), taskbar_offset: 0, taskbar_theme: "system".into(),
            taskbar_gap: 4, taskbar_order: ["network", "cpu", "memory", "cpu_temp", "gpu_temp", "disk_temp"].map(String::from).to_vec(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if !["system", "light", "dark"].contains(&self.theme.as_str())
            || !["system", "light", "dark"].contains(&self.taskbar_theme.as_str()) {
            return Err("不支持的主题".into());
        }
        if !(11..=18).contains(&self.font_size) || !(9..=18).contains(&self.taskbar_font_size) {
            return Err("主界面字号应为 11–18，任务栏字号应为 9–18".into());
        }
        for font in [&self.font_family, &self.taskbar_font] {
            if font.trim().is_empty() || font.encode_utf16().count() > 31 || font.chars().any(char::is_control) {
                return Err("请输入有效字体名称（最多 31 个字符）".into());
            }
        }
        if ![1000, 2000, 5000].contains(&self.sample_interval_ms) {
            return Err("采样间隔应为 1、2 或 5 秒".into());
        }
        if !["left", "right"].contains(&self.taskbar_position.as_str()) || !(0..=2000).contains(&self.taskbar_offset) {
            return Err("任务栏位置或偏移无效".into());
        }
        if self.taskbar_enabled && !self.taskbar_network && !self.taskbar_cpu && !self.taskbar_memory
            && !self.taskbar_cpu_temp && !self.taskbar_gpu_temp && !self.taskbar_disk_temp {
            return Err("请至少选择一个任务栏指标".into());
        }
        let mut order = self.taskbar_order.clone(); order.sort();
        if order != ["cpu", "cpu_temp", "disk_temp", "gpu_temp", "memory", "network"] || self.taskbar_gap > 24 {
            return Err("任务栏顺序必须包含全部六个指标各一次，间距应为 0–24".into());
        }
        if !["left", "center"].contains(&self.taskbar_text_align.as_str()) {
            return Err("文字对齐方式无效".into());
        }
        Ok(())
    }

    fn migrate_order(&mut self) {
        // Preserve the previous three-item ordering and all other preferences.
        let mut previous = self.taskbar_order.clone(); previous.sort();
        if previous == ["cpu", "memory", "network"] {
            self.taskbar_order.extend(["cpu_temp", "gpu_temp", "disk_temp"].map(String::from));
        }
    }
}

pub struct SettingsStore {
    value: Mutex<Settings>,
    pub path: PathBuf,
}

impl SettingsStore {
    pub fn load(path: PathBuf) -> Self {
        let value = std::fs::read(&path).ok()
            .and_then(|bytes| {
                let mut settings = serde_json::from_slice::<Settings>(&bytes).ok()?;
                // One-time migration from the original absolute left-positioned HUD.
                let old = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
                if old.get("taskbar_order").is_none() {
                    settings.taskbar_position = "right".into(); settings.taskbar_offset = 0;
                }
                settings.migrate_order();
                Some(settings)
            })
            .filter(|s| s.validate().is_ok()).unwrap_or_default();
        Self { value: Mutex::new(value), path }
    }

    pub fn get(&self) -> Settings {
        self.value.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn save(&self, settings: Settings) -> Result<Settings, String> {
        settings.validate()?;
        let mut current = self.value.lock().map_err(|e| e.to_string())?;
        self.persist(&settings)?;
        *current = settings.clone();
        Ok(settings)
    }

    fn persist(&self,settings:&Settings)->Result<(),String>{
        let dir = self.path.parent().ok_or("配置目录无效")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upgrades_existing_order_without_resetting_preferences() {
        let mut s = Settings::default();
        s.taskbar_order = ["memory", "network", "cpu"].map(String::from).to_vec();
        s.taskbar_gap = 11;
        s.migrate_order(); s.migrate_order();
        assert_eq!(&s.taskbar_order[..3], &["memory", "network", "cpu"]);
        assert_eq!(s.taskbar_order.len(), 6);
        assert_eq!(s.taskbar_gap, 11);
        assert!(s.validate().is_ok());
        s.taskbar_order[5] = "cpu".into();
        assert!(s.validate().is_err());
    }
    #[test]
    fn accepts_temperature_only_and_centered_text() {
        let mut s = Settings::default();
        s.taskbar_enabled = true;
        s.taskbar_network = false; s.taskbar_cpu = false; s.taskbar_memory = false;
        s.taskbar_cpu_temp = true; s.taskbar_text_align = "center".into();
        assert!(s.validate().is_ok());
        s.taskbar_text_align = "unsupported".into();
        assert!(s.validate().is_err());
    }
    #[test]
    fn rejects_invalid_settings_and_preserves_defaults_for_missing_fields() {
        let mut s: Settings = serde_json::from_str("{}").unwrap();
        assert!(s.validate().is_ok());
        s.taskbar_enabled = true;
        s.taskbar_network = false; s.taskbar_cpu = false; s.taskbar_memory = false;
        assert!(s.validate().is_err());
        s = Settings::default(); s.font_size = 99;
        assert!(s.validate().is_err());
        s = Settings::default(); s.sample_interval_ms = 0;
        assert!(s.validate().is_err());
    }
    #[test]
    fn saves_and_reloads_configuration() {
        let dir = std::env::temp_dir().join(format!("winmeter-settings-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        let store = SettingsStore::load(path.clone());
        let mut s = store.get(); s.font_family = "Microsoft YaHei UI".into(); s.font_size = 15;
        store.save(s.clone()).unwrap();
        assert_eq!(SettingsStore::load(path).get(), s);
        std::fs::remove_file(dir.join("settings.json")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn old_disk_order_is_ignored_without_resetting_other_preferences(){
        let s:Settings=serde_json::from_str(r#"{"font_size":15,"disk_order":["volumes","apps"]}"#).unwrap();
        assert_eq!(s.font_size,15);assert!(s.validate().is_ok());
    }
}
