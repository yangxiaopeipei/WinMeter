//! Physical device information from the existing read-only sensor helper.
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct PhysicalDisk {
    pub id: String,
    pub name: String,
    pub number: u32,
    pub kind: String,
    pub bus: String,
    pub firmware: String,
    pub capacity_bytes: u64,
    pub mounts: Vec<String>,
    pub status: String,
    pub temperature_c: Option<f64>,
    pub life_percent: Option<f64>,
    /// DiskInfoToolkit reports lifetime host transfers in GB, not bytes.
    pub host_read_gb: Option<u64>,
    pub host_write_gb: Option<u64>,
    pub power_on_hours: Option<u64>,
}

pub fn parse(tree: Option<&serde_json::Value>) -> Vec<PhysicalDisk> {
    tree.and_then(|v|v["Disks"].as_array()).into_iter().flatten().filter_map(|v| {
        let mut disk:PhysicalDisk=serde_json::from_value(v.clone()).ok()?;
        if disk.id.is_empty() || disk.name.is_empty() {return None;}
        disk.temperature_c=disk.temperature_c.filter(|v|v.is_finite()&&*v>0.0&&*v<125.0);
        disk.life_percent=disk.life_percent.filter(|v|v.is_finite()&&(0.0..=100.0).contains(v));
        Some(disk)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_and_invalid_health_remain_missing_instead_of_healthy() {
        let v=serde_json::json!({"Disks":[{"id":"/storage/0","name":"SSD","life_percent":-1,"temperature_c":0,"host_write_gb":42}]});
        let disks=parse(Some(&v));assert_eq!(disks.len(),1);
        assert_eq!(disks[0].life_percent,None);assert_eq!(disks[0].temperature_c,None);
        assert_eq!(disks[0].host_write_gb,Some(42));assert_eq!(disks[0].power_on_hours,None);
        assert!(parse(None).is_empty());
    }
}
