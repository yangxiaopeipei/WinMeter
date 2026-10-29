//! WinMeter 系统指标采集模块。
//! 全部使用免管理员权限的 Windows API：GetSystemTimes / NtQuerySystemInformation / GlobalMemoryStatusEx /
//! GetPerformanceInfo / ToolHelp 进程快照 / GetIfTable2 / GetSystemPowerStatus / PDH 计数器 / 注册表 / WMI(电池)。

use serde::{Deserialize, Serialize};

const GB: f64 = 1024.0 * 1024.0 * 1024.0;
const HOUR: i64 = 3600;
const DAY: i64 = 86400;
static REFRESH_NETWORK:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
pub fn refresh_network(){REFRESH_NETWORK.store(true,std::sync::atomic::Ordering::SeqCst);}

// ---------- 时间工具（磁盘/电池账本共用） ----------

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Howard Hinnant 的 days_from_civil：公历日期 → 1970-01-01 起的天数。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 本机时区偏移（秒）。同一时刻的 GetLocalTime 与 GetSystemTime 相减，四舍五入到 15 分钟。
#[cfg(windows)]
fn local_offset_secs() -> i64 {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    unsafe {
        let l = match st_to_unix(&GetLocalTime()) {
            Some(v) => v,
            None => return 8 * 3600,
        };
        let u = match st_to_unix(&GetSystemTime()) {
            Some(v) => v,
            None => return 8 * 3600,
        };
        let diff = l - u;
        ((diff as f64) / 900.0).round() as i64 * 900
    }
}

#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    8 * 3600
}

/// 某个 unix 秒所在的「本地小时」起点（unix 秒）。
fn hour_key(t: i64, off: i64) -> i64 {
    let l = t + off;
    (l - l.rem_euclid(HOUR)) - off
}

/// 某个 unix 秒所在的「本地日」起点（unix 秒）。
fn day_key(t: i64, off: i64) -> i64 {
    let l = t + off;
    (l - l.rem_euclid(DAY)) - off
}

/// SYSTEMTIME（按 UTC 解释）→ unix 秒
#[cfg(windows)]
fn st_to_unix(st: &windows::Win32::Foundation::SYSTEMTIME) -> Option<i64> {
    if st.wYear < 1970 || st.wMonth == 0 || st.wDay == 0 {
        return None;
    }
    Some(
        days_from_civil(st.wYear as i64, st.wMonth as i64, st.wDay as i64) * DAY
            + st.wHour as i64 * 3600
            + st.wMinute as i64 * 60
            + st.wSecond as i64,
    )
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr::null_mut;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, IP_ADAPTER_ADDRESSES_LH, ICMP_ECHO_REPLY, MIB_IF_TABLE2,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{
        AF_INET, AF_INET6, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6,
    };
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetLogicalDrives, GetVolumeInformationW,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhEnumObjectItemsW,
        PdhGetFormattedCounterValue, PdhOpenQueryW, PdhRemoveCounter, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE,
        PERF_DETAIL_WIZARD,
    };
    use windows::Win32::System::Power::{
        CallNtPowerInformation, GetSystemPowerStatus, SystemBatteryState, SYSTEM_BATTERY_STATE,
        SYSTEM_POWER_STATUS,
    };
    use windows::Win32::System::ProcessStatus::{
        GetPerformanceInfo, GetProcessMemoryInfo, PERFORMANCE_INFORMATION, PROCESS_MEMORY_COUNTERS,
    };
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_QWORD,
        RRF_RT_REG_SZ,
    };
    use windows::Win32::System::SystemInformation::{
        GetSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO,
    };
    use windows::Win32::System::Threading::{
        GetProcessIoCounters, GetProcessTimes, GetSystemTimes, OpenProcess, IO_COUNTERS,
        PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    };
    use windows::Win32::System::WindowsProgramming::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION;
    use windows::Wdk::System::SystemInformation::{
        NtQuerySystemInformation, SystemProcessorPerformanceInformation,
    };

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn filetime_to_u64(ft: &FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
    }

    fn reg_string(subkey: &str, value: &str) -> Option<String> {
        unsafe {
            let sub = to_wide(subkey);
            let val = to_wide(value);
            let mut size = 0u32;
            if RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(sub.as_ptr()),
                PCWSTR(val.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
            .0 != 0
            {
                return None;
            }
            let mut buf = vec![0u16; (size as usize / 2) + 1];
            if RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(sub.as_ptr()),
                PCWSTR(val.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut c_void),
                Some(&mut size),
            )
            .0 != 0
            {
                return None;
            }
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..len]).trim().to_string())
        }
    }

    fn reg_u64(subkey: &str, value: &str, qword: bool) -> Option<u64> {
        unsafe {
            let sub = to_wide(subkey);
            let val = to_wide(value);
            let mut data: u64 = 0;
            let mut size = size_of::<u64>() as u32;
            let flag = if qword { RRF_RT_REG_QWORD } else { RRF_RT_REG_DWORD };
            let r = RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(sub.as_ptr()),
                PCWSTR(val.as_ptr()),
                flag,
                None,
                Some(&mut data as *mut u64 as *mut c_void),
                Some(&mut size),
            );
            if r.0 == 0 { Some(data) } else { None }
        }
    }

    // ---------- PDH ----------

    pub struct Pdh {
        query: isize,
        counters: Vec<(String, isize)>,
    }

    impl Pdh {
        pub fn new() -> Option<Pdh> {
            unsafe {
                let mut query: isize = 0;
                if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                    return None;
                }
                Some(Pdh { query, counters: Vec::new() })
            }
        }

        pub fn add(&mut self, path: &str) -> bool {
            if self.counters.iter().any(|(name, _)| name == path) { return true; }
            unsafe {
                let mut h: isize = 0;
                let p = to_wide(path);
                if PdhAddEnglishCounterW(self.query, PCWSTR(p.as_ptr()), 0, &mut h) == 0 {
                    self.counters.push((path.to_string(), h));
                    true
                } else {
                    false
                }
            }
        }

        fn enum_instances(object: &str) -> Vec<String> {
            unsafe {
                let obj = to_wide(object);
                let mut csize = 0u32;
                let mut isize_ = 0u32;
                PdhEnumObjectItemsW(
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR(obj.as_ptr()),
                    windows::core::PWSTR::null(),
                    &mut csize,
                    windows::core::PWSTR::null(),
                    &mut isize_,
                    PERF_DETAIL_WIZARD,
                    0,
                );
                if isize_ == 0 {
                    return Vec::new();
                }
                // pcchCounterListLength / pcchInstanceListLength 的单位是 TCHAR(=u16)，
                // 不是字节：之前按字节 /2 分配会导致缓冲区越界写，触发访问冲突。
                let mut ibuf = vec![0u16; isize_ as usize + 2];
                let mut cbuf = vec![0u16; csize as usize + 2];
                PdhEnumObjectItemsW(
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR(obj.as_ptr()),
                    windows::core::PWSTR(cbuf.as_mut_ptr()),
                    &mut csize,
                    windows::core::PWSTR(ibuf.as_mut_ptr()),
                    &mut isize_,
                    PERF_DETAIL_WIZARD,
                    0,
                );
                let mut out = Vec::new();
                let mut cur = String::new();
                for &c in &ibuf {
                    if c == 0 {
                        if cur.is_empty() {
                            break;
                        }
                        out.push(std::mem::take(&mut cur));
                    } else {
                        cur.push(char::from_u32(c as u32).unwrap_or('?'));
                    }
                }
                out
            }
        }

        pub fn collect(&mut self) -> bool {
            unsafe { PdhCollectQueryData(self.query) == 0 }
        }

        fn read_double(&self, name: &str) -> Option<f64> {
            for (n, h) in &self.counters {
                if n == name {
                    unsafe {
                        let mut v = PDH_FMT_COUNTERVALUE::default();
                        if PdhGetFormattedCounterValue(*h, PDH_FMT_DOUBLE, None, &mut v) == 0
                            && v.CStatus <= 1
                        {
                            return Some(v.Anonymous.doubleValue);
                        }
                    }
                    return None;
                }
            }
            None
        }
    }

    impl Drop for Pdh {
        fn drop(&mut self) {
            unsafe { PdhCloseQuery(self.query); }
        }
    }

    // ---------- 静态信息 ----------

    #[derive(Clone)]
    pub struct GpuAdapter {
        pub name: String,
        pub luid_high: u32,
        pub luid_low: u32,
        /// 专用显存（独显的独立显存 / 集显那一小段）
        pub dedicated_bytes: u64,
        /// 专用显存 ≥ 1 GB 判为独显（集显一般只有 128 MB）
        pub discrete: bool,
    }

    #[derive(Clone)]
    pub struct StaticInfo {
        pub cpu_name: Option<String>,
        pub cpu_base_mhz: Option<u32>,
        pub core_count: u32,
        pub gpus: Vec<GpuAdapter>,
        #[allow(dead_code)]
        pub mem_total_bytes: u64,
    }

    /// DXGI 枚举物理显卡：名字、LUID（用来和 PDH 的 luid_0x.._0x.. 实例名对上）、专用显存。
    fn dxgi_adapters() -> Vec<GpuAdapter> {
        use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
        let mut out: Vec<GpuAdapter> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        unsafe {
            let factory: IDXGIFactory1 = match CreateDXGIFactory1() {
                Ok(f) => f,
                Err(_) => return out,
            };
            for i in 0..8u32 {
                let adapter = match factory.EnumAdapters1(i) {
                    Ok(a) => a,
                    Err(_) => break,
                };
                let desc = match adapter.GetDesc1() {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                // 0x1414 = Microsoft，只做软件渲染（Basic Render Driver），不是真显卡
                if desc.VendorId == 0x1414 {
                    continue;
                }
                let len = desc
                    .Description
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(desc.Description.len());
                let name = String::from_utf16_lossy(&desc.Description[..len])
                    .trim()
                    .to_string();
                if name.is_empty() || name.contains("Basic Render") {
                    continue;
                }
                // 同一张卡会被 DXGI 枚举出多条（混合显卡笔记本常见），按名字去重
                if !seen.insert(name.clone()) {
                    continue;
                }
                let dedicated_bytes = desc.DedicatedVideoMemory as u64;
                out.push(GpuAdapter {
                    name,
                    luid_high: desc.AdapterLuid.HighPart as u32,
                    luid_low: desc.AdapterLuid.LowPart,
                    dedicated_bytes,
                    discrete: dedicated_bytes >= 1024 * 1024 * 1024,
                });
            }
        }
        out
    }

    /// DXGI 失败时的兜底：从注册表读驱动描述（拿不到 LUID，只能展示名字/显存）
    fn reg_gpus() -> Vec<GpuAdapter> {
        const GPU_CLASS: &str =
            r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
        let mut out = Vec::new();
        for i in 0..16 {
            let sub = format!("{}\\{:04}", GPU_CLASS, i);
            if let Some(desc) = reg_string(&sub, "DriverDesc") {
                if desc.is_empty() || desc.starts_with('@') {
                    continue;
                }
                let mem = reg_u64(&sub, "HardwareInformation.qwMemorySize", true).unwrap_or(0);
                out.push(GpuAdapter {
                    name: desc,
                    luid_high: 0,
                    luid_low: 0,
                    dedicated_bytes: mem,
                    discrete: mem >= 1024 * 1024 * 1024,
                });
            }
        }
        out
    }

    pub fn read_static_info() -> StaticInfo {
        let mut si = SYSTEM_INFO::default();
        unsafe { GetSystemInfo(&mut si) };
        let core_count = si.dwNumberOfProcessors;

        let cpu_name = reg_string(
            r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            "ProcessorNameString",
        );
        let cpu_base_mhz =
            reg_u64(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "~MHz", false).map(|v| v as u32);

        let mut gpus = dxgi_adapters();
        if gpus.is_empty() {
            gpus = reg_gpus();
        }

        let mut ms = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        let mem_total_bytes =
            if unsafe { GlobalMemoryStatusEx(&mut ms) }.is_ok() { ms.ullTotalPhys } else { 0 };

        StaticInfo {
            cpu_name,
            cpu_base_mhz,
            core_count,
            gpus,
            mem_total_bytes,
        }
    }

    // ---------- CPU ----------

    pub struct CpuState {
        pub prev_idle: u64,
        pub prev_total: u64,
        pub prev_core: Vec<(u64, u64)>,
    }

    pub fn sample_cpu(state: &mut CpuState, core_count: u32) -> (f64, Vec<f64>) {
        let mut usage = 0.0f64;
        let mut cores = Vec::new();
        unsafe {
            let mut idle = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_ok() {
                let i = filetime_to_u64(&idle);
                let k = filetime_to_u64(&kernel);
                let u = filetime_to_u64(&user);
                // KernelTime 已包含 IdleTime。
                let total = k + u;
                if state.prev_total > 0 && total > state.prev_total {
                    let dt = total - state.prev_total;
                    let di = i.saturating_sub(state.prev_idle);
                    usage = (dt.saturating_sub(di) as f64 / dt as f64 * 100.0).clamp(0.0, 100.0);
                }
                state.prev_idle = i;
                state.prev_total = total;
            }

            let n = core_count.max(1) as usize;
            let mut buf: Vec<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION> =
                vec![SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION::default(); n + 8];
            let mut ret = 0u32;
            let status = NtQuerySystemInformation(
                SystemProcessorPerformanceInformation,
                buf.as_mut_ptr() as *mut c_void,
                (buf.len() * size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>()) as u32,
                &mut ret,
            );
            if status.is_ok() {
                let cnt = (ret as usize / size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>()).min(n);
                if state.prev_core.len() != cnt {
                    state.prev_core = vec![(0, 0); cnt];
                }
                for idx in 0..cnt {
                    let p = &buf[idx];
                    let idle_t = p.IdleTime.max(0) as u64;
                    let total_t =
                        (p.KernelTime.max(0) + p.UserTime.max(0)) as u64;
                    let mut u = 0.0;
                    if let Some(&(pi, pt)) = state.prev_core.get(idx) {
                        if pt > 0 && total_t > pt {
                            let dt = total_t - pt;
                            let di = idle_t.saturating_sub(pi);
                            u = (dt.saturating_sub(di) as f64 / dt as f64 * 100.0).clamp(0.0, 100.0);
                        }
                    }
                    state.prev_core[idx] = (idle_t, total_t);
                    cores.push(u);
                }
            }
        }
        (usage, cores)
    }

    // ---------- 内存 ----------

    pub fn sample_mem(
        pdh: &Option<Pdh>,
    ) -> (f64, u32, f64, f64, f64, f64, f64, f64, f64, f64, f64) {
        // (total_gb, load, used_gb, in_use_gb, standby_gb, modified_gb, cached_gb, available_gb, commit_gb, commit_limit_gb, pool_paged_gb)
        let mut total = 0.0f64;
        let mut load = 0u32;
        let mut available = 0.0f64;
        let mut commit = 0.0f64;
        let mut commit_limit = 0.0f64;
        let mut pool_paged = 0.0f64;
        unsafe {
            let mut ms = MEMORYSTATUSEX {
                dwLength: size_of::<MEMORYSTATUSEX>() as u32,
                ..Default::default()
            };
            if GlobalMemoryStatusEx(&mut ms).is_ok() {
                total = ms.ullTotalPhys as f64 / GB;
                available = ms.ullAvailPhys as f64 / GB;
                load = ms.dwMemoryLoad;
            }
            let mut pi = PERFORMANCE_INFORMATION::default();
            if GetPerformanceInfo(&mut pi, size_of::<PERFORMANCE_INFORMATION>() as u32).is_ok() {
                let ps = pi.PageSize as f64;
                commit = pi.CommitTotal as f64 * ps / GB;
                commit_limit = pi.CommitLimit as f64 * ps / GB;
                pool_paged = pi.KernelPaged as f64 * ps / GB;
            }
        }
        let mut standby = 0.0f64;
        let mut modified = 0.0f64;
        if let Some(p) = pdh {
            standby = p
                .read_double("\\Memory\\Standby Cache Core Bytes")
                .unwrap_or(0.0)
                + p
                    .read_double("\\Memory\\Standby Cache Normal Priority Bytes")
                    .unwrap_or(0.0)
                + p
                    .read_double("\\Memory\\Standby Cache Reserve Bytes")
                    .unwrap_or(0.0);
            modified = p.read_double("\\Memory\\Modified Page List Bytes").unwrap_or(0.0);
            standby /= GB;
            modified /= GB;
        }
        let cached = standby + modified;
        // Windows 口径：total = in_use + modified + available（available 已含 standby）
        let in_use = (total - available - modified).max(0.0);
        let used = total - available;
        (
            total,
            load,
            used,
            in_use,
            standby,
            modified,
            cached,
            available,
            commit,
            commit_limit,
            pool_paged,
        )
    }

    // ---------- 磁盘 ----------

    pub fn sample_pagefiles() -> Option<(f64, f64)> {
        use windows::Win32::Foundation::BOOL;
        use windows::Win32::System::ProcessStatus::{EnumPageFilesW, ENUM_PAGE_FILE_INFORMATION};
        unsafe extern "system" fn visit(ctx: *mut c_void, info: *mut ENUM_PAGE_FILE_INFORMATION, _: PCWSTR) -> BOOL {
            let totals = &mut *(ctx as *mut (usize, usize));
            totals.0 += (*info).TotalInUse;
            totals.1 += (*info).TotalSize;
            BOOL(1)
        }
        let mut totals = (0usize, 0usize);
        let mut info = SYSTEM_INFO::default();
        unsafe {
            GetSystemInfo(&mut info);
            EnumPageFilesW(Some(visit), &mut totals as *mut _ as *mut c_void).ok()?;
        }
        let page_gb = info.dwPageSize as f64 / GB;
        Some((totals.0 as f64 * page_gb, totals.1 as f64 * page_gb))
    }

    pub struct DiskData {
        pub active: f64,
        pub read_bps: f64,
        pub write_bps: f64,
        pub volumes: Vec<(String, Option<String>, u64, u64)>, // (mount, label, free, total)
    }

    pub fn sample_disk(pdh: &Option<Pdh>) -> DiskData {
        let mut d = DiskData {
            active: 0.0,
            read_bps: 0.0,
            write_bps: 0.0,
            volumes: Vec::new(),
        };
        if let Some(p) = pdh {
            let idle = p.read_double("\\PhysicalDisk(_Total)\\% Idle Time").unwrap_or(0.0);
            d.active = (100.0 - idle).clamp(0.0, 100.0);
            d.read_bps = p
                .read_double("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec")
                .unwrap_or(0.0);
            d.write_bps = p
                .read_double("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec")
                .unwrap_or(0.0);
        }
        unsafe {
            let drives = GetLogicalDrives();
            if drives != 0 {
                for i in 0..26 {
                    if drives & (1u32 << i) == 0 {
                        continue;
                    }
                    let letter = (b'A' + i as u8) as char;
                    let root = format!("{}:\\", letter);
                    let wroot = to_wide(&root);
                    let mut total_bytes = 0u64;
                    let mut free_bytes = 0u64;
                    if GetDiskFreeSpaceExW(
                        PCWSTR(wroot.as_ptr()),
                        None,
                        Some(&mut total_bytes),
                        Some(&mut free_bytes),
                    )
                    .is_ok()
                    {
                        let mut label = vec![0u16; 128];
                        let _ = GetVolumeInformationW(
                            PCWSTR(wroot.as_ptr()),
                            Some(label.as_mut_slice()),
                            None,
                            None,
                            None,
                            None,
                        );
                        let llen = label.iter().position(|&c| c == 0).unwrap_or(0);
                        let label = if llen > 0 {
                            Some(String::from_utf16_lossy(&label[..llen]))
                        } else {
                            None
                        };
                        d.volumes
                            .push((format!("{}:", letter), label, free_bytes, total_bytes));
                    }
                }
            }
        }
        d
    }

    // ---------- 网络 ----------

    fn read_wide(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len]).trim().to_string()
    }

    pub fn sample_net(
        prev: &mut HashMap<u32, (u64, u64)>,
        session: &mut (u64, u64),
    ) -> (f64, f64, Vec<(String, f64, f64)>) {
        let mut down = 0.0f64;
        let mut up = 0.0f64;
        let mut merged: HashMap<String, (f64, f64)> = HashMap::new();
        unsafe {
            let mut table: *mut MIB_IF_TABLE2 = null_mut();
            if GetIfTable2(&mut table).0 == 0 {
                let rows = std::slice::from_raw_parts(
                    (*table).Table.as_ptr(),
                    (*table).NumEntries as usize,
                );
                let has_physical=rows.iter().any(|r|r.InterfaceAndOperStatusFlags._bitfield&0x01!=0 && r.OperStatus==IfOperStatusUp && matches!(r.Type,6|71|243|244));
                for row in rows {
                    // 过滤掉 NDIS 过滤驱动层（WFP / QoS 等）和环回接口：
                    // 它们和物理网卡共用同一份流量，直接显示会把适配器列表撑爆。
                    if row.InterfaceAndOperStatusFlags._bitfield & 0x02 != 0 {
                        continue;
                    }
                    if row.Type == 24 {
                        // IF_TYPE_SOFTWARE_LOOPBACK
                        continue;
                    }
                    let alias = read_wide(&row.Alias);
                    let desc = read_wide(&row.Description);
                    // Keep real TUN/VPN rows for diagnosis; skip infrastructure adapters.
                    let dl = desc.to_lowercase();
                    if ["wi-fi direct", "teredo", "wan miniport", "kernel debug"]
                        .iter()
                        .any(|k| dl.contains(k))
                    {
                        continue;
                    }
                    let name = if !alias.is_empty() {
                        alias
                    } else if !desc.is_empty() {
                        desc.clone()
                    } else {
                        format!("接口 {}", row.InterfaceIndex)
                    };
                    if name.to_lowercase().contains("loopback")
                        || name.to_lowercase().contains("kernel debug")
                    {
                        continue;
                    }
                    let in_o = row.InOctets;
                    let out_o = row.OutOctets;
                    let (mut d, mut u) = (0.0f64, 0.0f64);
                    if let Some(&(pi, po)) = prev.get(&row.InterfaceIndex) {
                        if in_o >= pi {
                            d = (in_o - pi) as f64;
                        }
                        if out_o >= po {
                            u = (out_o - po) as f64;
                        }
                    }
                    prev.insert(row.InterfaceIndex, (in_o, out_o));
                    // 只保留「已连接」或「有过流量」的接口
                    if row.OperStatus.0 != 1 && in_o + out_o == 0 {
                        continue;
                    }
                    // Virtual tunnel counters repeat packets already seen by the
                    // physical NIC. Keep their rows, but count totals only once.
                    let counted=!tunnel_hint(&name,&desc,row.Type)
                        && (!has_physical || row.InterfaceAndOperStatusFlags._bitfield&0x01!=0);
                    if counted {down+=d;up+=u;session.0+=d as u64;session.1+=u as u64;}
                    let e = merged.entry(name).or_insert((0.0, 0.0));
                    e.0 += d;
                    e.1 += u;
                }
                FreeMibTable(table as *const c_void);
            }
        }
        let mut adapters: Vec<(String, f64, f64)> =
            merged.into_iter().map(|(n, (d, u))| (n, d, u)).collect();
        adapters.sort_by(|a, b| {
            (b.1 + b.2)
                .partial_cmp(&(a.1 + a.2))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        (down, up, adapters)
    }

    // ---------- GPU ----------

    /// 从 GPU Engine 实例名（pid_1234_luid_0x..._eng_0_engtype_3D）里取出进程 PID。
    fn pid_from_instance(instance: &str) -> Option<u32> {
        let i = instance.find("pid_")? + 4;
        let rest = &instance[i..];
        let end = rest.find('_').unwrap_or(rest.len());
        rest[..end].parse::<u32>().ok()
    }

    /// 从 GPU 实例名里取出 LUID（luid_0x00000000_0x0000C4B1）
    fn luid_from_instance(instance: &str) -> Option<(u32, u32)> {
        let i = instance.find("luid_")? + 5;
        let mut it = instance[i..].split('_');
        let hi = it.next()?.trim_start_matches("0x");
        let lo = it.next()?.trim_start_matches("0x");
        Some((
            u32::from_str_radix(hi, 16).ok()?,
            u32::from_str_radix(lo, 16).ok()?,
        ))
    }

    pub struct GpuSample {
        /// 所有显卡中最繁忙引擎的使用率
        pub total: f64,
        pub mem_used: u64,
        pub by_pid: HashMap<u32, f64>,
        /// (high, low) → 该卡最繁忙引擎使用率
        pub by_luid: HashMap<(u32, u32), f64>,
        /// (high, low) → 该卡专用显存已用
        pub mem_by_luid: HashMap<(u32, u32), u64>,
    }

    pub fn sample_gpu(pdh: &Option<Pdh>) -> GpuSample {
        let mut s = GpuSample {
            total: 0.0,
            mem_used: 0,
            by_pid: HashMap::new(),
            by_luid: HashMap::new(),
            mem_by_luid: HashMap::new(),
        };
        let mut engines: HashMap<((u32, u32), String), f64> = HashMap::new();
        if let Some(p) = pdh {
            for (name, _) in &p.counters {
                if name.starts_with("\\GPU Engine(") {
                    if let Some(v) = p.read_double(name) {
                        let v = v.max(0.0);
                        // 实例名形如 \GPU Engine(pid_4820_luid_0x00000000_0x..._engtype_3D)\Utilization Percentage
                        if let Some(pid) = pid_from_instance(name) {
                            let usage = s.by_pid.entry(pid).or_insert(0.0);
                            *usage = usage.max(v);
                        }
                        if let Some(luid) = luid_from_instance(name) {
                            if let Some(start) = name.find("_phys_").or_else(|| name.find("_eng_")) {
                                let engine = name[start..].split(')').next().unwrap_or("").to_string();
                                *engines.entry((luid, engine)).or_insert(0.0) += v;
                            }
                        }
                    }
                }
            }
            for ((luid, _), usage) in engines {
                let adapter = s.by_luid.entry(luid).or_insert(0.0);
                *adapter = adapter.max(usage.clamp(0.0, 100.0));
            }
            s.total = s.by_luid.values().copied().fold(0.0, f64::max);
            for (name, _) in &p.counters {
                if name.starts_with("\\GPU Adapter Memory(") {
                    if let Some(v) = p.read_double(name) {
                        let v = v.max(0.0) as u64;
                        s.mem_used += v;
                        if let Some(luid) = luid_from_instance(name) {
                            *s.mem_by_luid.entry(luid).or_insert(0) += v;
                        }
                    }
                }
            }
        }
        for v in s.by_pid.values_mut() {
            *v = v.clamp(0.0, 100.0);
        }
        for v in s.by_luid.values_mut() {
            *v = v.clamp(0.0, 100.0);
        }
        s
    }

    pub fn add_gpu_counters(pdh: &mut Pdh) {
        let mut active = HashSet::new();
        for inst in Pdh::enum_instances("GPU Engine") {
            let path = format!("\\GPU Engine({})\\Utilization Percentage", inst);
            active.insert(path.clone());
            pdh.add(&path);
        }
        for inst in Pdh::enum_instances("GPU Adapter Memory") {
            let path = format!("\\GPU Adapter Memory({})\\Dedicated Usage", inst);
            active.insert(path.clone());
            pdh.add(&path);
        }
        pdh.counters.retain(|(name, counter)| {
            let stale = name.starts_with("\\GPU ") && !active.contains(name);
            if stale { unsafe { PdhRemoveCounter(*counter); } }
            !stale
        });
    }

    // ---------- 温度 ----------

    type PfnInit = unsafe extern "system" fn() -> i32;
    type PfnCount = unsafe extern "system" fn(*mut u32) -> i32;
    type PfnHandle = unsafe extern "system" fn(u32, *mut *mut c_void) -> i32;
    type PfnTemp = unsafe extern "system" fn(*mut c_void, u32, *mut u32) -> i32;
    type PfnName = unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> i32;

    struct Nvml {
        count: PfnCount,
        handle: PfnHandle,
        temp: PfnTemp,
        name: PfnName,
    }

    fn nvml_sym<T: Copy>(lib: windows::Win32::Foundation::HMODULE, name: &str) -> Option<T> {
        use windows::Win32::System::LibraryLoader::GetProcAddress;
        let c = std::ffi::CString::new(name).ok()?;
        unsafe {
            let p = GetProcAddress(lib, windows::core::PCSTR(c.as_ptr() as *const u8))?;
            Some(std::mem::transmute_copy::<unsafe extern "system" fn() -> isize, T>(&p))
        }
    }

    impl Nvml {
        fn load() -> Option<Nvml> {
            use windows::Win32::System::LibraryLoader::LoadLibraryW;
            let wide: Vec<u16> = "nvml.dll\0".encode_utf16().collect();
            let lib = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())).ok()? };
            let init: PfnInit = nvml_sym(lib, "nvmlInit_v2")?;
            let count: PfnCount = nvml_sym(lib, "nvmlDeviceGetCount_v2")?;
            let handle: PfnHandle = nvml_sym(lib, "nvmlDeviceGetHandleByIndex_v2")?;
            let temp: PfnTemp = nvml_sym(lib, "nvmlDeviceGetTemperature")?;
            let name: PfnName = nvml_sym(lib, "nvmlDeviceGetName")?;
            if unsafe { init() } != 0 {
                return None;
            }
            Some(Nvml { count, handle, temp, name })
        }

        /// Match the adapter; never assign an NVIDIA temperature to an Intel GPU.
        fn temp_of(&self, want: &str) -> Option<f64> {
            unsafe {
                let mut n = 0u32;
                if (self.count)(&mut n) != 0 || n == 0 {
                    return None;
                }
                let mut fallback: Option<f64> = None;
                for i in 0..n {
                    let mut dev: *mut c_void = std::ptr::null_mut();
                    if (self.handle)(i, &mut dev) != 0 || dev.is_null() {
                        continue;
                    }
                    let mut buf = [0i8; 96];
                    let ok = (self.name)(dev, buf.as_mut_ptr() as *mut c_void, 96) == 0;
                    let name = if ok {
                        let bytes: Vec<u8> = buf
                            .iter()
                            .take_while(|&&c| c != 0)
                            .map(|&c| c as u8)
                            .collect();
                        String::from_utf8_lossy(&bytes).to_string()
                    } else {
                        String::new()
                    };
                    let mut t = 0u32;
                    if (self.temp)(dev, 0, &mut t) != 0 {
                        continue;
                    }
                    let t = t as f64;
                    if !name.is_empty() && (name == want || want.contains(&name) || name.contains(want)) {
                        return Some(t);
                    }
                    if fallback.is_none() {
                        fallback = Some(t);
                    }
                }
                if want.is_empty() {fallback} else {None}
            }
        }
    }

    /// 磁盘温度：IOCTL_STORAGE_QUERY_PROPERTY / StorageDeviceTemperatureProperty（通常需要管理员）
    fn disk_temp_ioctl() -> Option<f64> {
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
            OPEN_EXISTING,
        };
        use windows::Win32::System::IO::DeviceIoControl;
        const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D1400;
        const STORAGE_DEVICE_TEMPERATURE_PROPERTY: u32 = 15;
        let path: Vec<u16> = "\\\\.\\PhysicalDrive0\0".encode_utf16().collect();
        unsafe {
            let h = CreateFileW(
                PCWSTR(path.as_ptr()),
                FILE_READ_ATTRIBUTES.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .ok()?;
            let mut q = [0u8; 12];
            q[0..4].copy_from_slice(&STORAGE_DEVICE_TEMPERATURE_PROPERTY.to_le_bytes());
            q[4..8].copy_from_slice(&0u32.to_le_bytes()); // PropertyStandardQuery
            let mut out = [0u8; 512];
            let mut ret = 0u32;
            let ok = DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                Some(q.as_ptr() as *const c_void),
                q.len() as u32,
                Some(out.as_mut_ptr() as *mut c_void),
                out.len() as u32,
                Some(&mut ret),
                None,
            );
            let _ = CloseHandle(h);
            ok.ok()?;
            let count = u32::from_le_bytes(out[12..16].try_into().ok()?);
            if count == 0 {
                return None;
            }
            // STORAGE_TEMPERATURE_INFO 从 offset 16 开始，Temperature(u16, 摄氏) 在 +4
            let t = u16::from_le_bytes(out[20..22].try_into().ok()?);
            if t == 0 || t > 150 {
                return None;
            }
            Some(t as f64)
        }
    }

    pub struct Temps {
        nvml: Option<Nvml>,
        pub gpu: Option<f64>,
        pub cpu: Option<f64>,
        pub disk: Option<f64>,
        /// Storage IOCTL fallback; CPU data must come from an identified sensor.
        disk_raw: Option<f64>,
        disk_next: i64,
    }

    impl Temps {
        pub fn new() -> Temps {
            Temps {
                nvml: Nvml::load(),
                gpu: None,
                cpu: None,
                disk: None,
                disk_raw: None,
                disk_next: 0,
            }
        }

        pub fn sample(&mut self, gpu_name: &str, now: i64, lhm: &LhmData) {
            self.gpu = lhm.gpu_by_name.get(gpu_name).copied()
                .or_else(||if lhm.gpu_by_name.is_empty(){lhm.gpu}else{None})
                .or_else(||self.gpu_temperature(gpu_name));
            if now >= self.disk_next {
                self.disk_raw = disk_temp_ioctl();
                self.disk_next = now + if self.disk_raw.is_some() {10} else {60};
            }
            // A thermal zone is not an identified CPU sensor. Missing CPU data
            // remains missing, including after the helper stops or loses access.
            self.cpu = lhm.cpu;
            self.disk = lhm.disk.or(self.disk_raw);
        }
        pub fn gpu_temperature(&self, name: &str) -> Option<f64> {
            self.nvml.as_ref().and_then(|n| n.temp_of(name))
        }
    }

    // ---------- 网络详情：本地 IP / 网关 / DNS / MAC / 链路速率 / Wi-Fi ----------

    fn sockaddr_ip(a: *const SOCKADDR) -> Option<String> {
        if a.is_null() {
            return None;
        }
        unsafe {
            match (*a).sa_family {
                AF_INET => {
                    let sin = a as *const SOCKADDR_IN;
                    let b = (*sin).sin_addr.S_un.S_addr.to_ne_bytes();
                    Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
                }
                AF_INET6 => {
                    let sin6 = a as *const SOCKADDR_IN6;
                    let bytes = (*sin6).sin6_addr.u.Byte;
                    // 跳过 fe80:: 链路本地
                    if u16::from_be_bytes([bytes[0], bytes[1]]) & 0xffc0 == 0xfe80 {
                        return None;
                    }
                    Some(std::net::Ipv6Addr::from(bytes).to_string())
                }
                _ => None,
            }
        }
    }

    type IfSnapshot = NetInterfaceJson;

    fn tunnel_hint(name: &str, description: &str, kind: u32) -> bool {
        let text=format!("{name} {description}").to_lowercase();
        matches!(kind,23|131) || ["tunnel","wintun","wireguard","sing-tun","sing-box","mihomo","clash","tap-windows","openvpn","tailscale","zerotier"]
            .iter().any(|s|text.contains(s)) || name.to_lowercase().starts_with("tun")
    }

    fn route_to(target: &str, interfaces: &[IfSnapshot]) -> Option<NetRouteJson> {
        use windows::Win32::{NetworkManagement::IpHelper::{GetBestRoute2,MIB_IPFORWARD_ROW2},Networking::WinSock::SOCKADDR_INET};
        let ip:std::net::Ipv4Addr=target.parse().ok()?;
        unsafe {
            let mut destination=SOCKADDR_INET::default();
            destination.Ipv4.sin_family=AF_INET;
            destination.Ipv4.sin_addr.S_un.S_addr=u32::from_ne_bytes(ip.octets());
            let mut route=MIB_IPFORWARD_ROW2::default(); let mut source=SOCKADDR_INET::default();
            if GetBestRoute2(None,0,None,&destination,0,&mut route,&mut source).0!=0 {return None;}
            let nic=interfaces.iter().find(|i|i.index==route.InterfaceIndex);
            Some(NetRouteJson {target:target.into(),interface_index:route.InterfaceIndex,
                if_name:nic.map(|i|i.name.clone()).unwrap_or_else(||format!("接口 {}",route.InterfaceIndex)),
                next_hop:sockaddr_ip(&route.NextHop as *const _ as *const SOCKADDR),
                source:sockaddr_ip(&source as *const _ as *const SOCKADDR),
                prefix_len:route.DestinationPrefix.PrefixLength,is_tun:nic.is_some_and(|i|i.is_tun)})
        }
    }

    fn collect_interfaces() -> Vec<IfSnapshot> {
        use windows::Win32::NetworkManagement::IpHelper::{
            GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST,
            GAA_FLAG_SKIP_MULTICAST,
        };
        use windows::Win32::Networking::WinSock::AF_UNSPEC;

        let mut out = Vec::new();
        unsafe {
            let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
            let mut size = 0u32;
            let _ = GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, None, &mut size);
            if size == 0 {
                return out;
            }
            let mut buf = vec![0u8; size as usize + 256];
            let head = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
            if GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(head), &mut size) != 0 {
                return out;
            }
            let mut cur = head;
            while !cur.is_null() {
                let a = &*cur;
                cur = a.Next;
                if a.IfType==24 {continue;}

                let name = read_wide_pwstr(a.FriendlyName);

                let mut ipv4 = None;
                let mut ipv6 = None;
                let mut u = a.FirstUnicastAddress;
                while !u.is_null() {
                    if let Some(ip) = sockaddr_ip((*u).Address.lpSockaddr) {
                        if ip.contains(':') {
                            if ipv6.is_none() {
                                ipv6 = Some(ip);
                            }
                        } else if ipv4.is_none() && !ip.starts_with("169.254.") {
                            ipv4 = Some(ip);
                        }
                    }
                    u = (*u).Next;
                }

                let mut gateways = Vec::new();
                let mut g = a.FirstGatewayAddress;
                while !g.is_null() {
                    if let Some(ip) = sockaddr_ip((*g).Address.lpSockaddr) {
                        if !gateways.contains(&ip) {
                            gateways.push(ip);
                        }
                    }
                    g = (*g).Next;
                }

                let mut dns = Vec::new();
                let mut d = a.FirstDnsServerAddress;
                while !d.is_null() {
                    if let Some(ip) = sockaddr_ip((*d).Address.lpSockaddr) {
                        if !dns.contains(&ip) {
                            dns.push(ip);
                        }
                    }
                    d = (*d).Next;
                }

                let mac = {
                    let n = (a.PhysicalAddressLength as usize).min(8);
                    if n >= 6 && a.PhysicalAddress[..n].iter().any(|&b| b != 0) {
                        Some(
                            a.PhysicalAddress[..n]
                                .iter()
                                .map(|b| format!("{:02X}", b))
                                .collect::<Vec<_>>()
                                .join(":"),
                        )
                    } else {
                        None
                    }
                };

                if ipv4.is_none() && ipv6.is_none() && gateways.is_empty() {
                    continue;
                }

                let kind = match a.IfType {
                    6 => "以太网",
                    71 => "Wi-Fi",
                    243 | 244 => "蜂窝",
                    _ => "其他",
                };
                let mbps = if a.TransmitLinkSpeed > 0 {
                    Some(a.TransmitLinkSpeed as f64 / 1_000_000.0)
                } else {
                    None
                };

                out.push(IfSnapshot {
                    index:a.Anonymous1.Anonymous.IfIndex,
                    description:read_wide_pwstr(a.Description),mtu:a.Mtu,
                    is_tun:tunnel_hint(&name,&read_wide_pwstr(a.Description),a.IfType),
                    name,
                    mac,
                    ipv4,
                    ipv6,
                    gateways,
                    dns,
                    link_mbps: mbps,
                    kind:kind.into(),
                    up: a.OperStatus == IfOperStatusUp,
                });
            }
        }
        out
    }

    fn read_wide_pwstr(p: windows::core::PWSTR) -> String {
        if p.is_null() {
            return String::new();
        }
        unsafe {
            let mut n = 0usize;
            while *p.0.add(n) != 0 && n < 512 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p.0, n))
                .trim()
                .to_string()
        }
    }

    /// 当前连着的 Wi-Fi：SSID / 信号质量（0-100）/ 频段
    pub fn wifi_state() -> (Option<String>, Option<u32>, Option<f64>) {
        use windows::Win32::NetworkManagement::WiFi::{
            wlan_intf_opcode_current_connection, WlanCloseHandle, WlanEnumInterfaces,
            WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, WLAN_CONNECTION_ATTRIBUTES,
            WLAN_INTERFACE_INFO_LIST,
        };
        unsafe {
            let mut ver = 0u32;
            let mut handle = HANDLE::default();
            if WlanOpenHandle(2, None, &mut ver, &mut handle) != 0 {
                return (None, None, None);
            }
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = null_mut();
            if WlanEnumInterfaces(handle, None, &mut list) != 0 {
                let _ = WlanCloseHandle(handle, None);
                return (None, None, None);
            }
            let mut res = (None, None, None);
            let count = (*list).dwNumberOfItems as usize;
            for i in 0..count {
                let info = &(*list).InterfaceInfo[i];
                let mut data: *mut c_void = null_mut();
                let mut size = 0u32;
                let opcode = wlan_intf_opcode_current_connection;
                if WlanQueryInterface(
                    handle,
                    &info.InterfaceGuid,
                    opcode,
                    None,
                    &mut size,
                    &mut data,
                    None,
                ) != 0
                    || data.is_null()
                {
                    continue;
                }
                let conn = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                let ssid = &conn.wlanAssociationAttributes.dot11Ssid;
                let n = (ssid.uSSIDLength as usize).min(32);
                let name = String::from_utf8_lossy(&ssid.ucSSID[..n]).trim().to_string();
                let quality = conn.wlanAssociationAttributes.wlanSignalQuality;
                // ulRxRate / ulTxRate 单位是 kbps，转成 Mbps 作为「协商速率」
                let rate = conn.wlanAssociationAttributes.ulRxRate as f64 / 1000.0;
                if !name.is_empty() {
                    res = (Some(name), Some(quality), Some(rate));
                }
                WlanFreeMemory(data);
                if res.0.is_some() {
                    break;
                }
            }
            WlanFreeMemory(list as *const c_void);
            let _ = WlanCloseHandle(handle, None);
            res
        }
    }

    /// 系统代理（HKCU\...\Internet Settings）
    pub fn system_proxy() -> (bool, Option<String>) {
        use windows::Win32::System::Registry::{RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
        unsafe {
            let sub = to_wide(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings");
            let mut on: u32 = 0;
            let mut sz = size_of::<u32>() as u32;
            let en = to_wide("ProxyEnable");
            let ok = RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                PCWSTR(en.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut on as *mut u32 as *mut c_void),
                Some(&mut sz),
            )
            .0 == 0;
            if !ok || on == 0 {
                return (false, None);
            }
            // 值可能是 "127.0.0.1:7890" 或 "http=..;https=.."
            let mut sz2 = 0u32;
            let srv = to_wide("ProxyServer");
            if RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                PCWSTR(srv.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut sz2),
            )
            .0 != 0
            {
                return (true, None);
            }
            let mut b = vec![0u16; (sz2 as usize / 2) + 1];
            if RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                PCWSTR(srv.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(b.as_mut_ptr() as *mut c_void),
                Some(&mut sz2),
            )
            .0 != 0
            {
                return (true, None);
            }
            let len = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            let raw = String::from_utf16_lossy(&b[..len]);
            let picked = raw
                .split(';')
                .find_map(|p| p.strip_prefix("https="))
                .or_else(|| raw.split(';').find_map(|p| p.strip_prefix("http=")))
                .unwrap_or(&raw)
                .to_string();
            (true, Some(picked))
        }
    }

    pub struct NetInfo {
        pub interfaces: Vec<NetInterfaceJson>,
        pub routes: Vec<NetRouteJson>,
        pub pac_url: Option<String>,
        pub local_ip: Option<String>,
        pub ipv6: Option<String>,
        pub if_name: Option<String>,
        pub mac: Option<String>,
        pub link_mbps: Option<f64>,
        pub kind: Option<String>,
        pub gateways: Vec<String>,
        pub dns: Vec<String>,
        pub wifi_ssid: Option<String>,
        pub wifi_signal: Option<u32>,
        pub wifi_rate: Option<f64>,
        pub proxy_enabled: bool,
        pub proxy_server: Option<String>,
        next: i64,
    }

    impl NetInfo {
        pub fn new() -> NetInfo {
            let mut n = NetInfo {
                interfaces:Vec::new(),routes:Vec::new(),pac_url:None,
                local_ip: None,
                ipv6: None,
                if_name: None,
                mac: None,
                link_mbps: None,
                kind: None,
                gateways: Vec::new(),
                dns: Vec::new(),
                wifi_ssid: None,
                wifi_signal: None,
                wifi_rate: None,
                proxy_enabled: false,
                proxy_server: None,
                next: 0,
            };
            n.refresh();
            n
        }

        /// 5 秒刷一次：IP / 网关 / DNS 变化不频繁，但切网要跟上
        pub fn sample(&mut self, now: i64) {
            if now >= self.next {
                self.next = now + 5;
                self.refresh();
            }
        }

        fn refresh(&mut self) {
            let ifs = collect_interfaces();
            self.routes=["223.5.5.5","1.1.1.1"].iter().filter_map(|ip|route_to(ip,&ifs)).collect();
            let preferred=self.routes.iter().find(|r|!r.is_tun).map(|r|r.interface_index);
            // 优先：已连接 + 有 IPv4 + 有默认网关 + 链路速率高
            let best = ifs
                .iter()
                .filter(|i| i.up && !i.is_tun && (i.ipv4.is_some() || i.ipv6.is_some()))
                .max_by(|a, b| {
                    let score = |x: &IfSnapshot| {
                        (x.gateways.iter().any(|g| !g.contains(':')) as u8) * 4
                            + (x.kind == "Wi-Fi" || x.kind == "以太网") as u8 * 2
                            + (Some(x.index)==preferred) as u8 * 8
                    };
                    score(a).cmp(&score(b)).then(
                        a.link_mbps
                            .unwrap_or(0.0)
                            .partial_cmp(&b.link_mbps.unwrap_or(0.0))
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
                })
                ;
            self.local_ip=None;self.ipv6=None;self.if_name=None;self.mac=None;
            self.link_mbps=None;self.kind=None;self.gateways.clear();self.dns.clear();
            if let Some(b) = best {
                self.local_ip = b.ipv4.clone();
                self.ipv6 = b.ipv6.clone();
                self.if_name = Some(b.name.clone());
                self.mac = b.mac.clone();
                self.link_mbps = b.link_mbps;
                self.kind = Some(b.kind.to_string());
                self.gateways = b.gateways.clone();
                self.dns = b.dns.clone();
            }
            let (s, q, bd) = wifi_state();
            self.wifi_ssid = s;
            self.wifi_signal = q;
            self.wifi_rate = bd;
            let (pe, ps) = system_proxy();
            self.proxy_enabled = pe;
            self.proxy_server = ps;
            self.pac_url=system_pac();
            self.interfaces=ifs;
        }
    }

    fn system_pac() -> Option<String> {
        unsafe {
            let mut buf=[0u16;2048];let mut size=(buf.len()*2) as u32;
            if RegGetValueW(HKEY_CURRENT_USER,PCWSTR(to_wide(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings").as_ptr()),
                PCWSTR(to_wide("AutoConfigURL").as_ptr()),RRF_RT_REG_SZ,None,Some(buf.as_mut_ptr() as *mut c_void),Some(&mut size)).0!=0 {return None;}
            let value=read_wide(&buf);(!value.is_empty()).then_some(value)
        }
    }

    // ---------- 公网 / 代理出口（独立探测，禁止代理失败时回退直连） ----------

    pub struct Egress {
        pub ip: Option<String>,
        pub region: Option<String>,
        pub isp: Option<String>,
        pub via_proxy: bool,
        pub at: i64,
        pub rtt_ms: Option<f64>,
        pub rtt_target: Option<String>,
        pub error:Option<String>,pub proxy_ip:Option<String>,pub proxy_at:i64,
        pub proxy_error:Option<String>,pub proxy_server:Option<String>,pub proxy_path:Option<String>,
    }

    pub struct NetProbe {
        pub egress: std::sync::Arc<std::sync::Mutex<Egress>>,
    }

    fn curl_bin() -> String {
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let p = format!(r"{}\System32\curl.exe", sys);
        if std::path::Path::new(&p).exists() {
            p
        } else {
            "curl".to_string()
        }
    }

    fn curl_json(url:&str, proxy:Option<&str>) -> Result<serde_json::Value,String> {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new(curl_bin());
        cmd.args(["--disable","--silent","--fail","--connect-timeout","4","--max-time","8","--max-filesize","65536","--proto","=https"]);
        if let Some(server)=proxy {
            let p=if server.contains("://"){server.to_string()}else{format!("http://{server}")};
            cmd.args(["--proxy",&p,"--noproxy",""]);
        } else {
            cmd.args(["--proxy","","--noproxy","*"]);
        }
        let out=cmd.arg(url).creation_flags(0x0800_0000).output().map_err(|_|"无法启动系统网络查询".to_string())?;
        if !out.status.success() {return Err("查询失败或超时，稍后自动重试".into());}
        if out.stdout.len()>65536 {return Err("查询响应过大".into());}
        serde_json::from_slice(&out.stdout).map_err(|_|"服务返回了无效数据".into())
    }
    fn parse_ipip(v:&serde_json::Value) -> Option<(String,Option<String>,Option<String>)> {
        if v["ret"].as_str()!=Some("ok"){return None;}
        let ip=v["data"]["ip"].as_str()?.parse::<std::net::IpAddr>().ok()?.to_string();
        let location=v["data"]["location"].as_array()?;
        let parts:Vec<_>=location.iter().take(3).filter_map(|v|v.as_str()).filter(|s|!s.is_empty()).collect();
        let region=(!parts.is_empty()).then(||parts.join(" "));
        let isp=location.get(4).and_then(|v|v.as_str()).filter(|s|!s.is_empty()).map(str::to_string);
        Some((ip,region,isp))
    }
    fn fetch_egress() -> Result<(String, Option<String>, Option<String>),String> {
        let v=curl_json("https://myip.ipip.net/json",None)?;
        parse_ipip(&v).ok_or_else(||"IPIP 返回了无效地址或归属地".into())
    }
    fn fetch_proxy(server:Option<&str>) -> Result<String,String> {
        let v=curl_json("https://api.ipify.org?format=json",server)?;
        v["ip"].as_str().and_then(|ip|ip.parse::<std::net::IpAddr>().ok()).map(|ip|ip.to_string())
            .ok_or_else(||"代理回显服务返回了无效地址".into())
    }

    #[cfg(test)]
    mod network_tests {
        use super::*;
        #[test]
        fn parses_domestic_provider_and_rejects_invalid_addresses() {
            let v=serde_json::json!({"ret":"ok","data":{"ip":"203.0.113.9","location":["中国","广东","广州","","电信"]}});
            assert_eq!(parse_ipip(&v),Some(("203.0.113.9".into(),Some("中国 广东 广州".into()),Some("电信".into()))));
            let mut bad=v.clone();bad["data"]["ip"]=serde_json::json!("<script>");assert!(parse_ipip(&bad).is_none());
            bad=v;bad["ret"]=serde_json::json!("error");assert!(parse_ipip(&bad).is_none());
        }
        #[test]
        fn tunnel_detection_does_not_confuse_physical_adapters() {
            assert!(tunnel_hint("Meta","Meta Tunnel",53));
            assert!(tunnel_hint("VPN","WireGuard Tunnel",6));
            assert!(!tunnel_hint("Wi-Fi","Intel Wi-Fi 7",71));
            assert!(!tunnel_hint("以太网","Realtek PCIe GbE",6));
        }
        #[test]
        #[ignore="queries local interfaces, routes, and public IP providers"]
        fn inspect_network() {
            let n=NetInfo::new();
            println!("interfaces={}\nroutes={}",serde_json::to_string(&n.interfaces).unwrap(),serde_json::to_string(&n.routes).unwrap());
            println!("public={:?}",fetch_egress());
            if n.proxy_enabled || n.interfaces.iter().any(|i|i.up&&i.is_tun) {
                println!("proxy_or_tun={:?}",fetch_proxy(if n.proxy_enabled{n.proxy_server.as_deref()}else{None}));
            }
        }
    }

    fn ping_once(target: &str) -> Option<f64> {
        use windows::Win32::NetworkManagement::IpHelper::{
            IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho,
        };
        let addr: std::net::Ipv4Addr = target.parse().ok()?;
        let dst = u32::from_ne_bytes(addr.octets());
        unsafe {
            let h = IcmpCreateFile().ok()?;
            let payload = [0u8; 8];
            let mut reply = vec![0u8; 64 + 8 + 8];
            let n = IcmpSendEcho(
                h,
                dst,
                payload.as_ptr() as *const c_void,
                payload.len() as u16,
                None,
                reply.as_mut_ptr() as *mut c_void,
                reply.len() as u32,
                900,
            );
            let ok = n > 0;
            let rtt = if ok {
                let r = &*(reply.as_ptr() as *const ICMP_ECHO_REPLY);
                Some(r.RoundTripTime as f64)
            } else {
                None
            };
            let _ = IcmpCloseHandle(h);
            rtt
        }
    }

    impl NetProbe {
        pub fn spawn() -> NetProbe {
            let egress = std::sync::Arc::new(std::sync::Mutex::new(Egress {
                ip: None,
                region: None,
                isp: None,
                via_proxy: false,
                at: 0,
                rtt_ms: None,
                rtt_target: None,
                error:None,proxy_ip:None,proxy_at:0,proxy_error:None,proxy_server:None,proxy_path:None,
            }));
            let shared = egress.clone();
            std::thread::spawn(move || {
                let mut last_ip = 0i64;
                let mut previous_network=String::new();
                loop {
                    let now = now_unix();
                    if REFRESH_NETWORK.swap(false,std::sync::atomic::Ordering::SeqCst){last_ip=0;}
                    let ifs=collect_interfaces();
                    let (proxy_enabled,proxy_server)=system_proxy();
                    let proxy=if proxy_enabled {proxy_server.clone().filter(|s|!s.contains('=')&&!s.trim().is_empty())}else{None};
                    let via_tun=proxy.is_none()&&ifs.iter().any(|i|i.up&&i.is_tun);
                    let signature=format!("{:?}|{:?}|{:?}",proxy,system_pac(),ifs.iter().map(|i|(&i.name,&i.ipv4,&i.ipv6,i.up)).collect::<Vec<_>>());
                    if signature!=previous_network {
                        previous_network=signature;last_ip=0;
                        if let Ok(mut e)=shared.lock(){e.ip=None;e.region=None;e.isp=None;e.at=0;e.error=None;e.proxy_ip=None;e.proxy_at=0;e.proxy_error=None;}
                    }
                    // 延迟：优先打国内公共 DNS（真实外网链路），失败再退回默认网关。
                    // 网关可能是 TUN 假地址（198.18.0.0/15），ping 出来恒为 0，没意义。
                    let gw = {
                        ifs.iter()
                            .filter(|i| i.up && !i.is_tun)
                            .flat_map(|i| i.gateways.iter())
                            .find(|g| !g.contains(':') && !g.starts_with("198.18.") && !g.starts_with("198.19."))
                            .cloned()
                    };
                    let mut rtt = ping_once("223.5.5.5").map(|r| (r, "223.5.5.5".to_string()));
                    if rtt.is_none() {
                        if let Some(g) = &gw {
                            rtt = ping_once(g).map(|r| (r, g.clone()));
                        }
                    }
                    // HTTP 查询可能等待数秒，不要在持有采样快照锁时联网。
                    let checked=now-last_ip>=120;
                    let (ip_result,proxy_result)=if checked {
                        // Independent requests: one failed path must not hide the other.
                        let proxy_task=if proxy.is_some() || via_tun {
                            let p=proxy.clone();Some(std::thread::spawn(move||fetch_proxy(p.as_deref())))
                        }else{None};
                        let direct=fetch_egress();
                        let proxied=proxy_task.map(|t|t.join().unwrap_or_else(|_|Err("代理查询中断".into())));
                        last_ip=if direct.is_ok() && proxied.as_ref().is_none_or(|r|r.is_ok()){now}else{now-90};
                        (Some(direct),proxied)
                    }else{(None,None)};
                    {
                        if let Ok(mut e) = shared.lock() {
                            match rtt {
                                Some((ms, t)) => {
                                    e.rtt_ms = Some(ms);
                                    e.rtt_target = Some(t);
                                }
                                None => {
                                    e.rtt_ms = None;
                                    e.rtt_target = gw.clone();
                                }
                            }
                            e.proxy_server=proxy.clone();
                            e.proxy_path=if proxy.is_some(){Some("system".into())}else if via_tun{Some("tun".into())}else{None};
                            if let Some(result)=ip_result {match result {
                                Ok((ip, region, isp)) => {
                                    e.ip = Some(ip);
                                    e.region = region;
                                    e.isp = isp;
                                    e.via_proxy = false;
                                    e.at = now;
                                    e.error=None;
                                },
                                Err(error)=>{e.ip=None;e.region=None;e.isp=None;e.at=0;e.error=Some(error);},
                            }}
                            if proxy.is_none()&&!via_tun {e.proxy_ip=None;e.proxy_at=0;e.proxy_error=None;}
                            if let Some(result)=proxy_result {match result {
                                Ok(ip)=>{e.proxy_ip=Some(ip);e.proxy_at=now;e.proxy_error=None;},
                                Err(error)=>{e.proxy_ip=None;e.proxy_at=0;e.proxy_error=Some(error);},
                            }}
                        }
                    }
                    for _ in 0..15 {
                        if REFRESH_NETWORK.load(std::sync::atomic::Ordering::SeqCst){break;}
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                }
            });
            NetProbe { egress }
        }
    }

    // ---------- 外部传感器：LibreHardwareMonitor ----------
    // 内置 LHM 采集库负责传感器识别，PawnIO 负责所需的底层驱动访问。
    // 从本机独立采集进程的受限会话文件读取，不启动 HTTP 服务。

    #[derive(Clone, Default)]
    pub struct LhmData {
        pub online: bool,
        pub cpu: Option<f64>,
        pub gpu: Option<f64>,
        pub gpu_by_name: std::collections::HashMap<String,f64>,
        pub mainboard: Option<f64>,
        pub disk: Option<f64>,
        pub fan_rpm: Option<f64>,
    }

    #[derive(Default)]
    struct LhmAcc {
        cpu_pkg: Option<f64>,
        cpu_core_max: Option<f64>,
        gpu: Option<f64>,
        gpu_by_name: std::collections::HashMap<String,f64>,
        mainboard: Option<f64>,
        disk: Option<f64>,
        disk_primary: Option<f64>,
        fan: Option<f64>,
    }

    /// LHM 的 Value 是带单位的字符串，如 "55.0 °C" / "1200 RPM"
    fn lhm_num(s: &str) -> Option<f64> {
        let t = s.trim().split_whitespace().next()?;
        t.replace(',', ".").parse::<f64>().ok().filter(|v|v.is_finite())
    }

    fn walk_lhm(node: &serde_json::Value, acc: &mut LhmAcc) {
        // LHM uses a numeric tree id; the hardware path is SensorId.
        let id = node.get("SensorId").or_else(||node.get("Identifier"))
            .or_else(||node.get("id")).and_then(|x|x.as_str()).unwrap_or("");
        let text = node.get("Text").and_then(|x| x.as_str()).unwrap_or("");
        let val = node.get("Value").and_then(|x| x.as_str()).unwrap_or("");
        // 设备节点（/nvme/0 这类）没有 Value，下面按单位匹配时自然跳过
        let temperature=node.get("Type").and_then(|v|v.as_str())==Some("Temperature")
            || val.contains("°C") || val.contains('℃') || val.contains("°F");
        if temperature {
            let raw=node.get("RawValue").and_then(|v|v.as_f64());
            let t=raw.or_else(||lhm_num(val).map(|v|if val.contains("°F") {(v-32.0)/1.8} else {v}));
            if let Some(t) = t.filter(|v|v.is_finite() && (-20.0..150.0).contains(v)
                && !text.contains("Warning") && !text.contains("Critical") && !text.contains("Distance")) {
                if id.starts_with("/intelcpu/") || id.starts_with("/amdcpu/") {
                    if matches!(text,"CPU Package" | "Core (Tctl/Tdie)" | "Core (Tdie)" | "CPU (Tctl/Tdie)") {
                        acc.cpu_pkg = Some(t);
                    } else if !text.contains("Distance") {
                        acc.cpu_core_max = Some(acc.cpu_core_max.map_or(t, |m: f64| m.max(t)));
                    }
                } else if id.starts_with("/lpc/") || id.starts_with("/motherboard/") {
                    if text == "Motherboard" || text.contains("System") {
                        acc.mainboard = Some(acc.mainboard.map_or(t, |m: f64| m.max(t)));
                    }
                } else if id.starts_with("/nvme/") || id.starts_with("/hdd/") || id.starts_with("/ssd/") {
                    if matches!(text,"Composite Temperature"|"Temperature"|"Drive Temperature") {
                        acc.disk_primary=Some(acc.disk_primary.map_or(t,|previous|previous.max(t)));
                    } else if text.starts_with("Temperature #") {
                        acc.disk=Some(acc.disk.map_or(t,|previous|previous.max(t)));
                    }
                } else if id.starts_with("/gpu-") && (text == "GPU Core" || text.contains("Temperature")) {
                    acc.gpu = Some(acc.gpu.map_or(t, |m: f64| m.max(t)));
                    if let Some(name)=node.get("Hardware").and_then(|n|n.as_str()) {acc.gpu_by_name.insert(name.to_string(),t);}
                }
            }
        } else if val.contains("RPM") && (text.contains("Fan") || text.contains("风扇")) {
            if let Some(r) = lhm_num(val) {
                acc.fan = Some(acc.fan.map_or(r, |m: f64| m.max(r)));
            }
        }
        if let Some(ch) = node.get("Children").and_then(|c| c.as_array()) {
            for c in ch {
                walk_lhm(c, acc);
            }
        }
    }

    #[cfg(test)]
    mod sensor_tests {
        use super::*;
        #[test]
        fn power_requires_absolute_units_and_consistent_driver_state() {
            assert_eq!(reported_battery_power(22000,4,false),Some(22.0));
            assert_eq!(reported_battery_power(-18000,2,false),Some(-18.0));
            assert_eq!(reported_battery_power(0,1,false),Some(0.0));
            assert_eq!(reported_battery_power(0,4,false),None);
            assert_eq!(reported_battery_power(i32::MIN,2,false),None);
            assert_eq!(reported_battery_power(18000,2,false),None);
            assert_eq!(reported_battery_power(-18000,4,false),None);
            assert_eq!(reported_battery_power(18000,4,true),None);
        }
        #[test]
        fn reads_real_lhm_sensor_ids_and_raw_celsius() {
            let tree=serde_json::json!({"id":0,"Children":[
                {"id":11,"SensorId":"/intelcpu/0/temperature/0","Text":"CPU Core #1","Type":"Temperature","RawValue":81.0,"Value":"177.8 °F"},
                {"id":12,"SensorId":"/intelcpu/0/temperature/1","Text":"CPU Package","Type":"Temperature","RawValue":67.5,"Value":"153.5 °F"},
                {"id":13,"SensorId":"/gpu-nvidia/0/temperature/0","Text":"GPU Core","Value":"52.0 °C"},
                {"id":14,"SensorId":"/nvme/0/temperature/0","Text":"Temperature","Value":"41,5 °C"}
            ]});
            let mut acc=LhmAcc::default(); walk_lhm(&tree,&mut acc);
            assert_eq!(acc.cpu_pkg,Some(67.5)); assert_eq!(acc.cpu_core_max,Some(81.0));
            assert_eq!(acc.gpu,Some(52.0)); assert_eq!(acc.disk_primary,Some(41.5));
        }
        #[test]
        fn reads_amd_and_legacy_ids_and_rejects_missing_values() {
            let mut acc=LhmAcc::default();
            walk_lhm(&serde_json::json!({"SensorId":"/amdcpu/0/temperature/0","Text":"Core (Tctl/Tdie)","Type":"Temperature","Value":"140 °F"}),&mut acc);
            assert!((acc.cpu_pkg.unwrap()-60.0).abs()<0.001);
            walk_lhm(&serde_json::json!({"id":"/intelcpu/0/temperature/0","Text":"CPU Package","Value":"— °C"}),&mut acc);
            assert!((acc.cpu_pkg.unwrap()-60.0).abs()<0.001);
            assert_eq!(lhm_num("NaN °C"),None);
        }
        #[test]
        fn disk_temperature_ignores_warning_thresholds_and_prefers_composite() {
            let tree=serde_json::json!({"Children":[
                {"SensorId":"/nvme/0/temperature/0","Text":"Composite Temperature","Type":"Temperature","RawValue":48.0},
                {"SensorId":"/nvme/0/temperature/1","Text":"Temperature #1","Type":"Temperature","RawValue":44.0},
                {"SensorId":"/nvme/0/temperature/2","Text":"Warning Temperature","Type":"Temperature","RawValue":79.0},
                {"SensorId":"/nvme/0/temperature/3","Text":"Critical Temperature","Type":"Temperature","RawValue":84.0}
            ]});
            assert_eq!(parse_sensor_tree(&tree).disk,Some(48.0));
        }
    }

    pub fn parse_sensor_tree(v:&serde_json::Value)->LhmData {
        let mut acc = LhmAcc::default();
        walk_lhm(v, &mut acc);
        LhmData {
            online: true,
            cpu: acc.cpu_pkg.or(acc.cpu_core_max),
            gpu: acc.gpu,
            gpu_by_name: acc.gpu_by_name,
            mainboard: acc.mainboard,
            disk: acc.disk_primary.or(acc.disk),
            fan_rpm: acc.fan,
        }
    }

    // ---------- 本机信息 ----------

    /// 读取 SMBIOS 类型 16/17，得到插槽数和逐条内存的品牌、频率。
    /// 走固件表而不是 WMI：普通权限可读，不依赖 WMI 服务和 COM 初始化，
    /// 也不需要 PawnIO 驱动或管理员授权。
    pub fn read_memory_modules() -> (Vec<MemModuleJson>, Option<u32>) {
        use windows::Win32::System::SystemInformation::{GetSystemFirmwareTable, FIRMWARE_TABLE_PROVIDER};
        const RSMB: u32 = 0x5253_4D42; // 'RSMB'，即原始 SMBIOS 结构表

        let size = unsafe { GetSystemFirmwareTable(FIRMWARE_TABLE_PROVIDER(RSMB), 0, None) };
        if size < 12 {
            return (Vec::new(), None);
        }
        let mut buf = vec![0u8; size as usize];
        let got = unsafe { GetSystemFirmwareTable(FIRMWARE_TABLE_PROVIDER(RSMB), 0, Some(&mut buf)) };
        if got == 0 || got > size {
            return (Vec::new(), None);
        }
        buf.truncate(got as usize);
        // RawSMBIOSData：4 字节版本头 + 4 字节长度，表体从偏移 8 开始。
        let length = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
        let end = buf.len().min(8 + length);
        if end <= 8 {
            return (Vec::new(), None);
        }
        parse_smbios_memory(&buf[8..end])
    }

    fn smbios_strings(raw: &[u8]) -> Vec<String> {
        raw.split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).trim().to_string())
            .collect()
    }

    fn memory_type_name(kind: u8) -> Option<String> {
        let name = match kind {
            0x03 => "DRAM",
            0x0F => "SDRAM",
            0x12 => "DDR",
            0x13 => "DDR2",
            0x14 => "DDR2",
            0x18 => "DDR3",
            0x1A => "DDR4",
            0x1B => "LPDDR",
            0x1C => "LPDDR2",
            0x1D => "LPDDR3",
            0x1E => "LPDDR4",
            0x20 => "HBM",
            0x21 => "HBM2",
            0x22 => "DDR5",
            0x23 => "LPDDR5",
            0x24 => "HBM3",
            _ => return None,
        };
        Some(name.to_string())
    }

    fn memory_form_name(form: u8) -> Option<String> {
        let name = match form {
            0x09 => "DIMM",
            0x0C => "RIMM",
            0x0D => "SODIMM",
            0x0F => "FB-DIMM",
            _ => return None,
        };
        Some(name.to_string())
    }

    /// 固件里的厂商名往往带一长串公司后缀，直接显示会挤掉频率。
    /// 先削后缀，再按已知品牌归一化；认不出来就返回 None，界面留空。
    pub(super) fn clean_brand(raw: &str) -> Option<String> {
        let text = raw.trim();
        if text.is_empty() {
            return None;
        }
        let lower_raw = text.to_ascii_lowercase();
        if matches!(lower_raw.as_str(), "unknown" | "not specified" | "undefined" | "none" | "0000" | "00000000" | "0") {
            return None;
        }
        if text.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }

        let mut name = text.to_string();
        for suffix in [
            " Technology", " Technologies", " Semiconductor", " Semiconductors",
            " Inc.", " Inc", " Corp.", " Corp", " Corporation", " Company",
            " Co., Ltd.", " Co.,Ltd.", " Co Ltd", " Co.,Ltd", " Ltd.", " Ltd",
        ] {
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(&suffix.to_ascii_lowercase()) && name.len() > suffix.len() {
                name.truncate(name.len() - suffix.len());
            }
        }
        let name = name.trim().to_string();
        if name.is_empty() {
            return None;
        }

        let lower = name.to_ascii_lowercase();
        let alias = [
            ("sk hynix", "SK hynix"),
            ("hynix", "SK hynix"),
            ("samsung", "Samsung"),
            ("micron", "Micron"),
            ("crucial", "Crucial"),
            ("kingston", "Kingston"),
            ("a-data", "ADATA"),
            ("adata", "ADATA"),
            ("g.skill", "G.Skill"),
            ("ramaxel", "Ramaxel"),
            ("nanya", "Nanya"),
            ("transcend", "Transcend"),
            ("team group", "Team Group"),
            ("teamgroup", "Team Group"),
            ("corsair", "Corsair"),
            ("elpida", "Elpida"),
            ("patriot", "Patriot"),
            ("winbond", "Winbond"),
            ("pny", "PNY"),
        ];
        if let Some((_, display)) = alias.iter().find(|(key, _)| lower.starts_with(key)) {
            return Some((*display).to_string());
        }
        // 没见过的厂商：太长就截断，避免把频率挤出可视区。
        // 断在词中间不好看，能退到上一个空格就退。
        if name.chars().count() > 16 {
            let cut: String = name.chars().take(16).collect();
            let trimmed = cut.trim_end();
            let kept = match trimmed.rfind(' ') {
                Some(pos) if pos >= 8 => &trimmed[..pos],
                _ => trimmed,
            };
            return Some(kept.to_string() + "…");
        }
        Some(name)
    }

    pub(super) fn parse_smbios_memory(table: &[u8]) -> (Vec<MemModuleJson>, Option<u32>) {
        let mut modules = Vec::new();
        let mut slots = None;
        let mut i = 0usize;
        while i + 4 <= table.len() {
            let kind = table[i];
            let len = table[i + 1] as usize;
            if len < 4 || i + len > table.len() {
                break;
            }
            let fields = &table[i..i + len];
            if kind == 127 {
                break;
            }
            // 格式化区之后是字符串集，以连续两个 0 结束。
            let mut next = i + len;
            while next + 1 < table.len() && !(table[next] == 0 && table[next + 1] == 0) {
                next += 1;
            }
            let stop = (next + 2).min(table.len());
            let strings = smbios_strings(&table[(i + len).min(stop)..stop]);
            let text = |idx: u8| -> Option<String> {
                if idx == 0 {
                    return None;
                }
                strings.get(idx as usize - 1).cloned().filter(|v| !v.is_empty())
            };

            match kind {
                16 if len >= 0x0F => {
                    let count = u16::from_le_bytes([fields[0x0D], fields[0x0E]]);
                    if count > 0 {
                        slots = Some(count as u32);
                    }
                }
                17 if len >= 0x15 => {
                    let size_mb = u16::from_le_bytes([fields[0x0C], fields[0x0D]]);
                    let extended = if len >= 0x20 {
                        u32::from_le_bytes([fields[0x1C], fields[0x1D], fields[0x1E], fields[0x1F]])
                    } else {
                        0
                    };
                    // 0 = 空插槽，0xFFFF = 未知，0x7FFF = 看扩展字段（单位 MB）。
                    let capacity_gb = if size_mb == 0x7FFF && extended > 0 {
                        (extended & 0x7FFF_FFFF) as f64 / 1024.0
                    } else if size_mb == 0 || size_mb == 0xFFFF {
                        0.0
                    } else {
                        size_mb as f64 / 1024.0
                    };
                    let rated = u16::from_le_bytes([fields[0x15], fields[0x16]]);
                    let configured = if len >= 0x22 {
                        u16::from_le_bytes([fields[0x20], fields[0x21]])
                    } else {
                        0
                    };
                    let speed = if rated > 0 { rated } else { configured };
                    modules.push(MemModuleJson {
                        brand: text(fields[0x17]).as_deref().and_then(clean_brand),
                        part: text(fields[0x1A]),
                        slot: text(fields[0x10]),
                        capacity_gb,
                        speed_mts: if speed > 0 { Some(speed as u32) } else { None },
                        kind: memory_type_name(fields[0x12]),
                        form: memory_form_name(fields[0x0E]),
                    });
                }
                _ => {}
            }
            if stop <= i {
                break;
            }
            i = stop;
        }
        (modules, slots)
    }

    pub fn read_sys_info() -> SysJson {
        use wmi::{COMLibrary, WMIConnection};
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Board {
            manufacturer: Option<String>,
            product: Option<String>,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Bios {
            manufacturer: Option<String>,
            #[serde(rename = "SMBIOSBIOSVersion")]
            version: Option<String>,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Processor {
            number_of_cores: Option<u32>,
            number_of_logical_processors: Option<u32>,
        }

        let mut si = SysJson::default();
        si.hostname = std::env::var("COMPUTERNAME").ok();
        si.user = std::env::var("USERNAME").ok();
        si.arch = std::env::var("PROCESSOR_ARCHITECTURE").ok();
        let (mem_modules, mem_slots) = read_memory_modules();
        si.mem_modules = mem_modules;
        si.mem_slots = mem_slots;

        let nt = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let product = reg_string(nt, "ProductName");
        let edition = reg_string(nt, "DisplayVersion");
        si.os_name = product.clone();
        si.os_version = edition.clone();
        let build = reg_string(nt, "CurrentBuild");
        let ubr = reg_u64(nt, "UBR", false);
        si.build = build.map(|b| match ubr {
            Some(u) => format!("{}.{}", b, u),
            None => b,
        });
        // 注册表里 Win11 仍写着 Windows 10，按 build ≥ 22000 纠正显示名
        if let (Some(name), Some(b)) = (&si.os_name, reg_string(nt, "CurrentBuild")) {
            if b.parse::<u32>().map(|v| v >= 22000).unwrap_or(false) {
                si.os_name = Some(name.replace("Windows 10", "Windows 11"));
            }
        }

        if let Ok(com) = COMLibrary::new() {
            if let Ok(conn) = WMIConnection::new(com) {
                if let Ok(rows) = conn.raw_query::<Board>("SELECT Manufacturer, Product FROM Win32_BaseBoard") {
                    if let Some(b) = rows.into_iter().next() {
                        let parts: Vec<String> = [b.manufacturer, b.product].into_iter().flatten().collect();
                        if !parts.is_empty() {
                            si.board = Some(parts.join(" "));
                        }
                    }
                }
                if let Ok(rows) = conn.raw_query::<Bios>("SELECT Manufacturer, SMBIOSBIOSVersion FROM Win32_BIOS") {
                    if let Some(b) = rows.into_iter().next() {
                        let parts: Vec<String> = [b.manufacturer, b.version].into_iter().flatten().collect();
                        if !parts.is_empty() {
                            si.bios = Some(parts.join(" "));
                        }
                    }
                }
                if let Ok(rows) = conn.raw_query::<Processor>("SELECT NumberOfCores, NumberOfLogicalProcessors FROM Win32_Processor") {
                    if let Some(p) = rows.into_iter().next() {
                        si.cpu_physical_cores = p.number_of_cores;
                        si.cpu_logical_cores = p.number_of_logical_processors;
                    }
                }
            }
        }
        si
    }

    // ---------- 电池 ----------

    #[derive(Clone)]
    pub struct BatteryData {
        pub present: bool,
        pub charging: bool,
        pub ac_line: bool,
        pub percent: Option<f64>,
        pub remaining_sec: Option<u32>,
        pub rate_w: Option<f64>,
        /// 电池端净功率（W），仅驱动直报；正充电，负放电。
        pub power_w: Option<f64>,
        /// true = 驱动直报；false = 无有效直报数据
        pub power_sensor: bool,
        /// 驱动报告的当前电量（mWh）
        pub capacity_mwh: Option<u32>,
        /// true 表示 remaining_sec 是「容量 ÷ 功率」倒推的，不是系统给的
        pub remain_est: bool,
        pub full_capacity_mwh: Option<u32>,
    }

    pub fn sample_battery() -> BatteryData {
        let mut b = BatteryData {
            present: false,
            charging: false,
            ac_line: false,
            percent: None,
            remaining_sec: None,
            rate_w: None,
            power_w: None,
            power_sensor: false,
            capacity_mwh: None,
            remain_est: false,
            full_capacity_mwh: None,
        };
        unsafe {
            let mut sps = SYSTEM_POWER_STATUS::default();
            if GetSystemPowerStatus(&mut sps).is_ok() {
                b.ac_line = sps.ACLineStatus == 1;
                if sps.BatteryLifePercent != 255 {
                    b.percent = Some(sps.BatteryLifePercent as f64);
                }
                if sps.BatteryLifeTime > 0 && sps.BatteryLifeTime != u32::MAX && !b.ac_line {
                    b.remaining_sec = Some(sps.BatteryLifeTime);
                }
                b.present = sps.BatteryFlag != 255 && sps.BatteryFlag & 128 == 0;
                b.charging = b.present && sps.BatteryFlag & 8 != 0;
            }
            let mut st = SYSTEM_BATTERY_STATE::default();
            let r = CallNtPowerInformation(
                SystemBatteryState,
                None,
                0,
                Some(&mut st as *mut SYSTEM_BATTERY_STATE as *mut c_void),
                size_of::<SYSTEM_BATTERY_STATE>() as u32,
            );
            if r.is_ok() && st.BatteryPresent.0 != 0 {
                b.present = true;
                b.charging = st.Charging.0 != 0;
                b.ac_line = st.AcOnLine.0 != 0;
                if st.MaxCapacity > 0 && st.MaxCapacity != u32::MAX {
                    b.full_capacity_mwh = Some(st.MaxCapacity);
                }
                if st.RemainingCapacity != u32::MAX {
                    b.capacity_mwh = Some(st.RemainingCapacity);
                }
                if st.EstimatedTime > 0 && st.EstimatedTime != u32::MAX && st.Discharging.0 != 0 {
                    b.remaining_sec = Some(st.EstimatedTime);
                }
            }
        }
        let device = battery_device_data();
        if device.relative {
            // 相对单位不能标成 mWh / W，也不能用于容量差分。
            b.capacity_mwh = None;
            b.full_capacity_mwh = None;
            b.power_w = None;
            b.rate_w = None;
            b.power_sensor = false;
        } else if let Some(w) = device.power_w {
            b.present = true;
            b.power_w = Some(w);
            b.rate_w = Some(w.abs());
            b.power_sensor = true;
            b.charging = w > 0.0;
        }
        b
    }

    fn battery_chemistry(value:Option<u16>)->Option<String>{
        match value {
            Some(1)=>Some("其他"),Some(3)=>Some("铅酸"),Some(4)=>Some("镍镉"),
            Some(5)=>Some("镍氢"),Some(6)=>Some("锂离子"),Some(7)=>Some("锌空气"),Some(8)=>Some("锂聚合物"),
            _=>None,
        }.map(str::to_string)
    }
    #[cfg(test)]
    mod reviewed_metrics_tests {
        use super::*;
        #[test]
        fn missing_cpu_sensor_clears_old_value_instead_of_using_a_thermal_zone(){
            let mut temperatures=Temps {nvml:None,cpu:Some(72.0),gpu:None,disk:None,disk_raw:None,disk_next:i64::MAX};
            temperatures.sample("",1,&LhmData::default());assert_eq!(temperatures.cpu,None);
            temperatures.sample("",2,&LhmData{cpu:Some(68.0),..Default::default()});assert_eq!(temperatures.cpu,Some(68.0));
            temperatures.sample("",3,&LhmData::default());assert_eq!(temperatures.cpu,None);
        }
        #[test]
        fn chemistry_uses_windows_enumeration_and_keeps_unknown_missing(){
            for (code,label) in [(1,"其他"),(3,"铅酸"),(4,"镍镉"),(5,"镍氢"),(6,"锂离子"),(7,"锌空气"),(8,"锂聚合物")] {
                assert_eq!(battery_chemistry(Some(code)).as_deref(),Some(label));
            }
            for value in [None,Some(0),Some(2),Some(9)]{assert_eq!(battery_chemistry(value),None);}
        }
    }
    pub fn battery_wmi_details() -> (Option<String>, Option<String>, Option<u32>, Option<u32>) {
        use wmi::{COMLibrary, WMIConnection};
        #[derive(serde::Deserialize)]
        #[serde(rename = "Win32_Battery")]
        #[serde(rename_all = "PascalCase")]
        struct Win32Battery {
            name: Option<String>,
            chemistry: Option<u16>,
            design_capacity: Option<u32>,
            full_charge_capacity: Option<u32>,
        }
        let com = match COMLibrary::new() {
            Ok(c) => c,
            Err(_) => return (None, None, None, None),
        };
        let conn = match WMIConnection::new(com) {
            Ok(c) => c,
            Err(_) => return (None, None, None, None),
        };
        let results: Vec<Win32Battery> = match conn.query() {
            Ok(r) => r,
            Err(_) => return (None, None, None, None),
        };
        if let Some(b) = results.into_iter().next() {
            let chem = battery_chemistry(b.chemistry);
            (b.name, chem, b.design_capacity, b.full_charge_capacity)
        } else {
            (None, None, None, None)
        }
    }

    /// 电池设计容量 / 满充容量 / 循环次数。
    /// WMI 的 Win32_Battery.DesignCapacity 在多数机器上是空的，
    /// 只有 IOCTL_BATTERY_QUERY_INFORMATION 能稳定拿到（powercfg /batteryreport 走的就是这条路）。
    pub fn battery_health() -> (Option<u32>, Option<u32>, Option<u32>) {
        battery_device_data().health
    }

    #[derive(Default)]
    struct BatteryDeviceData {
        health: (Option<u32>, Option<u32>, Option<u32>),
        power_w: Option<f64>,
        relative: bool,
    }

    pub fn reported_battery_power(rate:i32,state:u32,relative:bool)->Option<f64> {
        if relative || rate==i32::MIN {return None;}
        match state & 6 {
            4 if rate>0=>Some(rate as f64/1000.0),
            2 if rate<0=>Some(rate as f64/1000.0),
            0 if rate==0=>Some(0.0),
            _=>None,
        }
    }

    fn battery_device_data() -> BatteryDeviceData {
        use windows::Win32::Devices::DeviceAndDriverInstallation::{
            SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
            SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
            GUID_DEVCLASS_BATTERY, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
        };
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows::Win32::System::IO::DeviceIoControl;
        use windows::Win32::System::Power::{
            BatteryInformation, BATTERY_INFORMATION, BATTERY_QUERY_INFORMATION,
            BATTERY_STATUS, BATTERY_WAIT_STATUS, BATTERY_CAPACITY_RELATIVE,
            IOCTL_BATTERY_QUERY_INFORMATION, IOCTL_BATTERY_QUERY_TAG, IOCTL_BATTERY_QUERY_STATUS,
        };
        use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HWND};
        unsafe {
            let hdev = match SetupDiGetClassDevsW(
                Some(&GUID_DEVCLASS_BATTERY),
                PCWSTR::null(),
                HWND::default(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            ) {
                Ok(h) => h,
                Err(_) => return BatteryDeviceData::default(),
            };
            let mut res = BatteryDeviceData::default();
            let mut count = 0;
            let mut valid_rates = 0;
            let mut total_w = 0.0;
            let mut did = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            for i in 0..8u32 {
                if SetupDiEnumDeviceInterfaces(hdev, None, &GUID_DEVCLASS_BATTERY, i, &mut did)
                    .is_err()
                {
                    break;
                }
                count += 1;
                let mut need = 0u32;
                let _ = SetupDiGetDeviceInterfaceDetailW(hdev, &did, None, 0, Some(&mut need), None);
                if need == 0 {
                    continue;
                }
                let mut buf = vec![0u16; (need as usize + 1) / 2];
                let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(hdev, &did, Some(detail), need, None, None)
                    .is_err()
                {
                    continue;
                }
                let h = match CreateFileW(
                    PCWSTR((*detail).DevicePath.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                ) {
                    Ok(h) => h,
                    Err(_) => continue,
                };
                // 先取 battery tag；超时写 0 表示不等待
                let mut timeout = 0u32;
                let mut tag = 0u32;
                let mut ret = 0u32;
                let got = DeviceIoControl(
                    h,
                    IOCTL_BATTERY_QUERY_TAG,
                    Some(&mut timeout as *mut u32 as *const c_void),
                    size_of::<u32>() as u32,
                    Some(&mut tag as *mut u32 as *mut c_void),
                    size_of::<u32>() as u32,
                    Some(&mut ret),
                    None,
                );
                if got.is_ok() && tag != 0 {
                    let q = BATTERY_QUERY_INFORMATION {
                        BatteryTag: tag,
                        InformationLevel: BatteryInformation,
                        AtRate: 0,
                    };
                    let mut info = BATTERY_INFORMATION::default();
                    let got2 = DeviceIoControl(
                        h,
                        IOCTL_BATTERY_QUERY_INFORMATION,
                        Some(&q as *const _ as *const c_void),
                        size_of::<BATTERY_QUERY_INFORMATION>() as u32,
                        Some(&mut info as *mut _ as *mut c_void),
                        size_of::<BATTERY_INFORMATION>() as u32,
                        Some(&mut ret),
                        None,
                    );
                    if got2.is_ok() {
                        res.relative |= info.Capabilities & BATTERY_CAPACITY_RELATIVE != 0;
                        let d = info.DesignedCapacity;
                        let f = info.FullChargedCapacity;
                        res.health = (
                            if d > 0 && d != u32::MAX { Some(d) } else { None },
                            if f > 0 && f != u32::MAX { Some(f) } else { None },
                            if info.CycleCount > 0 { Some(info.CycleCount) } else { None },
                        );
                        let wait = BATTERY_WAIT_STATUS {
                            BatteryTag: tag,
                            Timeout: 0,
                            ..Default::default()
                        };
                        let mut status = BATTERY_STATUS::default();
                        if DeviceIoControl(
                            h,
                            IOCTL_BATTERY_QUERY_STATUS,
                            Some(&wait as *const _ as *const c_void),
                            size_of::<BATTERY_WAIT_STATUS>() as u32,
                            Some(&mut status as *mut _ as *mut c_void),
                            size_of::<BATTERY_STATUS>() as u32,
                            Some(&mut ret),
                            None,
                        ).is_ok() && ret as usize >= size_of::<BATTERY_STATUS>()
                            && reported_battery_power(status.Rate,status.PowerState,res.relative).is_some()
                        {
                            // 驱动接口的 Rate 是有符号 mW；0 也是有效的停充读数。
                            total_w += reported_battery_power(status.Rate,status.PowerState,res.relative).unwrap();
                            valid_rates += 1;
                        }
                    }
                }
                let _ = CloseHandle(h);
            }
            let _ = SetupDiDestroyDeviceInfoList(hdev);
            // 多电池只在全部可读时报告总功率，避免显示部分电池的数值。
            if count > 0 && valid_rates == count && !res.relative {
                res.power_w = Some(total_w);
            }
            if res.relative {
                res.health = (None, None, None);
            }
            res
        }
    }

    /// 当前电源计划。标准方案直接查表（GUID 固定），自定义方案读系统给的显示名。
    pub fn power_plan() -> Option<String> {
        use windows::core::GUID;
        use windows::Win32::Foundation::{HLOCAL, LocalFree, WIN32_ERROR};
        use windows::Win32::System::Power::{PowerGetActiveScheme, PowerReadFriendlyName};
        unsafe {
            let mut guid: *mut GUID = null_mut();
            if PowerGetActiveScheme(None, &mut guid) != WIN32_ERROR(0) || guid.is_null() {
                return None;
            }
            let g = *guid;
            let _ = LocalFree(HLOCAL(guid as *mut c_void));
            let key = format!(
                "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                g.data1,
                g.data2,
                g.data3,
                g.data4[0],
                g.data4[1],
                g.data4[2],
                g.data4[3],
                g.data4[4],
                g.data4[5],
                g.data4[6],
                g.data4[7]
            );
            let known = |k: &str| -> Option<&'static str> {
                match k {
                    "381b4222-f694-41f0-9685-ff5bb260df2e" => Some("平衡"),
                    "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c" => Some("高性能"),
                    "a1841308-3541-4fab-bc81-f71556f20b4a" => Some("节能"),
                    "e9a42b02-d5df-448d-aa00-03f14749eb61" => Some("卓越性能"),
                    _ => None,
                }
            };
            if let Some(n) = known(&key) {
                return Some(n.to_string());
            }
            let mut size = 0u32;
            let _ = PowerReadFriendlyName(None, Some(&g), None, None, None, &mut size);
            if size == 0 {
                return Some(key);
            }
            let mut buf = vec![0u8; size as usize];
            if PowerReadFriendlyName(None, Some(&g), None, None, Some(buf.as_mut_ptr()), &mut size)
                .is_ok()
            {
                let u16s: Vec<u16> = buf
                    .chunks_exact(2)
                    .map(|c| u16::from_ne_bytes([c[0], c[1]]))
                    .collect();
                let len = u16s.iter().position(|&c| c == 0).unwrap_or(u16s.len());
                let name = String::from_utf16_lossy(&u16s[..len]).trim().to_string();
                if !name.is_empty() && !name.starts_with('@') {
                    return Some(name);
                }
                // @%SystemRoot%\...\powrprof.dll,-15,Balanced → 取最后一段
                if let Some(last) = name.rsplit(',').next() {
                    if !last.trim().is_empty() {
                        return Some(last.trim().to_string());
                    }
                }
            }
            Some(key)
        }
    }

    // ---------- 进程 ----------

    pub struct PrevProc {
        created:u64, cpu:u64, system:u64, io:Option<(u64,u64,u64)>, path:Option<String>,
    }
    pub struct ProcReading {
        pub process:ProcJson, pub path:Option<String>, pub read:u64, pub write:u64,
    }
    pub fn sample_procs(
        prev: &mut HashMap<u32, PrevProc>, sys_total_now:u64, elapsed_secs:f64,
    ) -> (Vec<ProcReading>, u32) {
        let mut out = Vec::new();
        let mut count = 0u32;
        unsafe {
            let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
                Ok(h) => h,
                Err(_) => {prev.clear();return (out,count);},
            };
            let mut pe = PROCESSENTRY32W {
                dwSize: size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap, &mut pe).is_ok() {
                loop {
                    count += 1;
                    let name = {
                        let len = pe.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                        String::from_utf16_lossy(&pe.szExeFile[..len])
                    };
                    let pid = pe.th32ProcessID;
                    let mut cpu: Option<f64> = None;
                    let mut mem: Option<u64> = None;
                    let mut dr: Option<f64> = None;
                    let mut dw: Option<f64> = None;
                    let mut ops: Option<f64> = None;
                    let mut path=None;let mut delta_read=0;let mut delta_write=0;
                    let mut observed=false;

                    if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                        let mut ct = FILETIME::default();
                        let mut et = FILETIME::default();
                        let mut kt = FILETIME::default();
                        let mut ut = FILETIME::default();
                        let mut io = IO_COUNTERS::default();
                        if GetProcessTimes(h, &mut ct, &mut et, &mut kt, &mut ut).is_ok() {
                            observed=true;
                            let cpu_time = filetime_to_u64(&kt) + filetime_to_u64(&ut);
                            let created=filetime_to_u64(&ct);
                            let old=prev.get(&pid).filter(|p|p.created==created);
                            if let Some(p) = old {
                                if sys_total_now > p.system {
                                    let wall = (sys_total_now - p.system) as f64;
                                    let dproc = cpu_time.saturating_sub(p.cpu) as f64;
                                    if wall > 0.0 {
                                        cpu = Some(dproc / wall * 100.0);
                                    }
                                }
                            }
                            let got_io = GetProcessIoCounters(h, &mut io).is_ok();
                            if got_io {
                                let rd = io.ReadTransferCount;
                                let wr = io.WriteTransferCount;
                                let op = io.ReadOperationCount + io.WriteOperationCount;
                                if let Some((r,w))=crate::app_io::delta(old.and_then(|p|p.io.map(|io|(p.created,io.0,io.1))),created,rd,wr) {
                                    delta_read=r;delta_write=w;
                                    dr=Some(r as f64/elapsed_secs);dw=Some(w as f64/elapsed_secs);
                                    ops=old.and_then(|p|p.io).map(|v|op.saturating_sub(v.2) as f64/elapsed_secs);
                                }
                            }
                            path=old.and_then(|p|p.path.clone());
                            if path.is_none() {
                                let mut text=vec![0u16;32768];let mut len=text.len() as u32;
                                if QueryFullProcessImageNameW(h,PROCESS_NAME_WIN32,windows::core::PWSTR(text.as_mut_ptr()),&mut len).is_ok(){
                                    path=Some(String::from_utf16_lossy(&text[..len as usize]));
                                }
                            }
                            prev.insert(pid,PrevProc{created,cpu:cpu_time,system:sys_total_now,
                                io:got_io.then_some((io.ReadTransferCount,io.WriteTransferCount,io.ReadOperationCount+io.WriteOperationCount)),path:path.clone()});
                            let mut pmc = PROCESS_MEMORY_COUNTERS::default();
                            if GetProcessMemoryInfo(
                                h,
                                &mut pmc,
                                size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                            )
                            .is_ok()
                            {
                                mem = Some(pmc.WorkingSetSize as u64);
                            }
                        }
                        let _ = CloseHandle(h);
                    }
                    if !observed {prev.remove(&pid);}
                    out.push(ProcReading{process:ProcJson{name,pid,cpu,mem_bytes:mem,disk_read_bps:dr,disk_write_bps:dw,disk_ops:ops,gpu_percent:None},path,read:delta_read,write:delta_write});
                    if Process32NextW(snap, &mut pe).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        let alive: HashSet<u32> = out.iter().map(|p| p.process.pid).collect();
        prev.retain(|k, _| alive.contains(k));
        (out, count)
    }

    // ---------- 蓝牙 ----------

    /// 蓝牙 Class of Device → 中文类型（跟设计稿的「鼠标 (HID)」「音频」口径一致）
    pub fn bt_kind(cod: u32) -> String {
        let major = (cod >> 8) & 0x1f;
        let minor = cod & 0xff;
        match major {
            1 => "电脑".to_string(),
            2 => "手机".to_string(),
            3 => "网络设备".to_string(),
            4 => match (minor >> 2) & 0x3f {
                1 | 2 | 6 => "音频 · 耳机".to_string(),
                3 | 4 | 5 => "音频 · 音箱".to_string(),
                _ => "音频".to_string(),
            },
            5 => match (minor >> 4) & 0x0f {
                1 => "键盘 · HID".to_string(),
                2 => "鼠标 · HID".to_string(),
                3 => "键盘/鼠标 · HID".to_string(),
                _ => "外设 · HID".to_string(),
            },
            6 => "图像设备".to_string(),
            7 => "穿戴设备".to_string(),
            _ => "其他".to_string(),
        }
    }

    /// 蓝牙适配器型号（设备管理器里的名字，如 MediaTek Bluetooth Adapter）。
    /// BluetoothGetRadioInfo 给的是无线电名称，默认就是计算机名，看不出硬件型号；
    /// 这里的适配器设备通常没有 FriendlyName，只有 DeviceDesc。
    pub fn bt_adapter_model() -> Option<String> {
        use windows::Win32::Devices::DeviceAndDriverInstallation::{
            SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
            SetupDiGetDeviceRegistryPropertyW, DIGCF_PRESENT, GUID_DEVCLASS_BLUETOOTH,
            SP_DEVINFO_DATA, SPDRP_DEVICEDESC, SPDRP_FRIENDLYNAME,
        };
        use windows::Win32::Foundation::HWND;
        unsafe {
            let h = SetupDiGetClassDevsW(
                Some(&GUID_DEVCLASS_BLUETOOTH),
                PCWSTR::null(),
                HWND::default(),
                DIGCF_PRESENT,
            )
            .ok()?;
            let read_prop = |did: *const SP_DEVINFO_DATA, prop| -> Option<String> {
                let mut buf = [0u8; 512];
                let mut t = 0u32;
                let mut need = 0u32;
                if SetupDiGetDeviceRegistryPropertyW(
                    h,
                    did,
                    prop,
                    Some(&mut t),
                    Some(&mut buf),
                    Some(&mut need),
                )
                .is_err()
                {
                    return None;
                }
                let u: Vec<u16> = buf
                    .chunks_exact(2)
                    .map(|c| u16::from_ne_bytes([c[0], c[1]]))
                    .collect();
                let n = u.iter().position(|&c| c == 0).unwrap_or(u.len());
                let s = String::from_utf16_lossy(&u[..n]).trim().to_string();
                if s.is_empty() { None } else { Some(s) }
            };
            let mut res = None;
            for i in 0..64u32 {
                let mut did = SP_DEVINFO_DATA {
                    cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInfo(h, i, &mut did).is_err() {
                    break;
                }
                let name = read_prop(&did, SPDRP_FRIENDLYNAME)
                    .or_else(|| read_prop(&did, SPDRP_DEVICEDESC));
                let name = match name {
                    Some(n) => n,
                    None => continue,
                };
                let low = name.to_lowercase();
                // 排除枚举器、协议服务、已配对设备
                let is_noise = low.contains("枚举器")
                    || low.contains("enumerator")
                    || low.contains("service")
                    || low.contains("服务")
                    || low.contains("rfcomm")
                    || low.contains("avrcp")
                    || low.contains("nap")
                    || low.contains("pse")
                    || low.contains("device (")
                    || low.starts_with("bluetooth 设备")
                    || low.contains("蓝牙设备");
                if is_noise {
                    continue;
                }
                if low.contains("adapter") || low.contains("适配器") {
                    res = Some(name);
                    break;
                }
                if res.is_none() && low.contains("bluetooth") {
                    res = Some(name);
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(h);
            res
        }
    }

    /// 蓝牙适配器名称（第一个 radio）
    pub fn radio_name() -> Option<String> {
        use windows::Win32::Devices::Bluetooth::{
            BluetoothFindFirstRadio, BluetoothFindRadioClose, BluetoothGetRadioInfo,
            BLUETOOTH_FIND_RADIO_PARAMS, BLUETOOTH_RADIO_INFO,
        };
        unsafe {
            let params = BLUETOOTH_FIND_RADIO_PARAMS {
                dwSize: size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
            };
            let mut radio = HANDLE::default();
            let find = BluetoothFindFirstRadio(&params, &mut radio).ok()?;
            let mut info = BLUETOOTH_RADIO_INFO {
                dwSize: size_of::<BLUETOOTH_RADIO_INFO>() as u32,
                ..Default::default()
            };
            let ok = BluetoothGetRadioInfo(radio, &mut info) == 0;
            let _ = BluetoothFindRadioClose(find);
            let _ = CloseHandle(radio);
            if !ok {
                return None;
            }
            let len = info.szName.iter().position(|&c| c == 0).unwrap_or(0);
            if len == 0 {
                None
            } else {
                Some(String::from_utf16_lossy(&info.szName[..len]))
            }
        }
    }

    #[allow(clippy::type_complexity)]
    pub fn sample_bluetooth() -> (
        u32,
        u32,
        Vec<(String, bool, bool, String, Option<i64>)>,
        Option<String>,
    ) {
        use windows::Win32::Devices::Bluetooth::{
            BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindNextDevice,
            BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS,
        };
        let mut connected = 0u32;
        let mut remembered = 0u32;
        let mut devices = Vec::new();
        let now = now_unix();
        unsafe {
            let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
                dwSize: size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
                fReturnAuthenticated: true.into(),
                fReturnRemembered: true.into(),
                fReturnUnknown: true.into(),
                fReturnConnected: true.into(),
                fIssueInquiry: false.into(),
                cTimeoutMultiplier: 0,
                hRadio: HANDLE::default(),
            };
            let mut dev = BLUETOOTH_DEVICE_INFO {
                dwSize: size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
                ..Default::default()
            };
            let find = match BluetoothFindFirstDevice(&params, &mut dev) {
                Ok(f) => f,
                Err(_) => return (connected, remembered, devices, radio_name()),
            };
            loop {
                let is_conn = dev.fConnected.as_bool();
                let is_rem = dev.fRemembered.as_bool();
                if is_conn {
                    connected += 1;
                }
                if is_rem {
                    remembered += 1;
                }
                let len = dev.szName.iter().position(|&c| c == 0).unwrap_or(0);
                let addr = dev.Address.Anonymous.ullLong;
                let name = if len > 0 {
                    String::from_utf16_lossy(&dev.szName[..len])
                } else {
                    format!(
                        "设备 {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                        (addr >> 40) as u8 & 0xff,
                        (addr >> 32) as u8 & 0xff,
                        (addr >> 24) as u8 & 0xff,
                        (addr >> 16) as u8 & 0xff,
                        (addr >> 8) as u8 & 0xff,
                        addr as u8 & 0xff,
                    )
                };
                devices.push((
                    name,
                    is_conn,
                    is_rem,
                    bt_kind(dev.ulClassofDevice),
                    st_to_unix(&dev.stLastUsed).map(|t| (now - t).max(0)),
                ));
                dev.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
                if BluetoothFindNextDevice(find, &mut dev).is_err() {
                    break;
                }
            }
            let _ = BluetoothFindDeviceClose(find);
        }
        (connected, remembered, devices, radio_name())
    }

    // ---------- Collector 状态 ----------

    pub struct CollectorState {
        pub pdh: Option<Pdh>,
        pub cpu: CpuState,
        pub procs: HashMap<u32, PrevProc>,
        pub net: HashMap<u32, (u64, u64)>,
        pub session: (u64, u64),
        pub temps: Temps,
        pub netinfo: NetInfo,
    }
}

// ---------- 磁盘 / 电池账本（持久化到 %APPDATA%\WinMeter\diskuse.json） ----------

#[derive(Serialize, Deserialize, Clone, Copy, Default)]
pub struct DiskBucket {
    pub ts: i64,
    pub read: u64,
    pub write: u64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default)]
pub struct BatPoint {
    pub ts: i64,
    pub pct: f64,
    pub ac: bool,
}

#[derive(Serialize, Deserialize, Default)]
pub struct LedgerFile {
    #[serde(default)]
    pub hours: Vec<DiskBucket>,
    #[serde(default)]
    pub days: Vec<DiskBucket>,
    #[serde(default)]
    pub battery: Vec<BatPoint>,
    #[serde(default)]
    pub apps: crate::app_io::History,
}

fn ledger_path() -> Option<std::path::PathBuf> {
    let base = std::env::var("APPDATA").ok()?;
    Some(std::path::PathBuf::from(base).join("WinMeter").join("diskuse.json"))
}

pub struct DiskLedger {
    pub data: LedgerFile,
    offset: i64,
    pub session_read: u64,
    pub session_write: u64,
    last_save: i64,
}

impl DiskLedger {
    pub fn load() -> DiskLedger {
        let now = now_unix();
        let mut data = LedgerFile::default();
        if let Some(p) = ledger_path() {
            if let Ok(txt) = std::fs::read_to_string(&p) {
                if let Ok(v) = serde_json::from_str::<LedgerFile>(&txt) {
                    data = v;
                }
            }
        }
        let mut l = DiskLedger {
            data,
            offset: local_offset_secs(),
            session_read: 0,
            session_write: 0,
            last_save: now,
        };
        l.prune(now);
        l
    }

    fn bump(list: &mut Vec<DiskBucket>, key: i64, r: u64, w: u64) {
        if let Some(b) = list.iter_mut().find(|b| b.ts == key) {
            b.read += r;
            b.write += w;
        } else {
            list.push(DiskBucket { ts: key, read: r, write: w });
            list.sort_by_key(|b| b.ts);
        }
    }

    pub fn add_disk(&mut self, read: u64, write: u64, now: i64) {
        self.session_read += read;
        self.session_write += write;
        let hk = hour_key(now, self.offset);
        let dk = day_key(now, self.offset);
        Self::bump(&mut self.data.hours, hk, read, write);
        Self::bump(&mut self.data.days, dk, read, write);
    }

    /// 电量点：同一分钟内重复采样只更新最后一点，保持曲线密度但不撑爆文件。
    pub fn add_battery(&mut self, pct: f64, ac: bool, now: i64) {
        if let Some(last) = self.data.battery.last_mut() {
            if now - last.ts < 30 {
                last.pct = pct;
                last.ac = ac;
                return;
            }
            if now < last.ts {
                return;
            }
        }
        self.data.battery.push(BatPoint { ts: now, pct, ac });
    }

    fn prune(&mut self, now: i64) {
        let h_cut = hour_key(now, self.offset) - 36 * HOUR;
        let d_cut = day_key(now, self.offset) - 60 * DAY;
        let b_cut = now - 12 * HOUR;
        self.data.hours.retain(|b| b.ts >= h_cut);
        self.data.days.retain(|b| b.ts >= d_cut);
        self.data.battery.retain(|b| b.ts >= b_cut);
        self.data.apps.prune(day_key(now,self.offset));
    }

    pub fn maybe_save(&mut self, now: i64) {
        self.prune(now);
        if now - self.last_save >= 60 {
            if self.save().is_ok(){self.last_save=now;}
        }
    }

    pub fn save(&self) -> Result<(),String> {
        self.save_to(&ledger_path().ok_or("历史目录不可用")?)
    }
    fn save_to(&self,p:&std::path::Path)->Result<(),String>{
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e|e.to_string())?;
        }
        let txt=serde_json::to_vec(&self.data).map_err(|e|e.to_string())?;
        let tmp=p.with_extension("json.tmp");
        std::fs::write(&tmp,txt).map_err(|e|e.to_string())?;
        std::fs::rename(&tmp,p).map_err(|e|e.to_string())
    }

    pub fn today(&self, now: i64) -> (u64, u64) {
        let dk = day_key(now, self.offset);
        self.data
            .days
            .iter()
            .find(|b| b.ts == dk)
            .map(|b| (b.read, b.write))
            .unwrap_or((0, 0))
    }

    pub fn recent_hours(&self, now: i64) -> Vec<DiskBucket> {
        let h = hour_key(now, self.offset);
        self.data
            .hours
            .iter()
            .filter(|b| b.ts > h - 24 * HOUR && b.ts <= h)
            .copied()
            .collect()
    }

    pub fn recent_days(&self, now: i64) -> Vec<DiskBucket> {
        let d = day_key(now, self.offset);
        self.data
            .days
            .iter()
            .filter(|b| b.ts > d - 30 * DAY && b.ts <= d)
            .copied()
            .collect()
    }


}

// ---------- 对外统一的 Sample ----------

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct Sample {
    pub cpu_usage: f64,
    pub cores: Vec<f64>,
    pub core_count: u32,
    pub cpu_name: Option<String>,
    pub cpu_base_mhz: Option<u32>,

    pub gpu_usage: f64,
    pub gpu_name: Option<String>,
    pub gpu_mem_used_gb: f64,
    pub gpu_mem_total_gb: f64,
    /// 每张显卡一条（集显 / 独显各算各的）
    pub gpus: Vec<GpuJson>,

    pub mem_total_gb: f64,
    pub mem_used_gb: f64,
    pub mem_load: u32,
    pub mem_in_use_gb: f64,
    pub mem_standby_gb: f64,
    pub mem_modified_gb: f64,
    pub mem_cached_gb: f64,
    pub mem_available_gb: f64,
    pub commit_gb: f64,
    pub commit_limit_gb: f64,
    pub pool_paged_gb: f64,

    pub net_down_bps: f64,
    pub net_up_bps: f64,
    pub net_session_down_bytes: u64,
    pub net_session_up_bytes: u64,
    pub adapters: Vec<AdapterInfo>,
    /// 本地 IP / 网卡 / 网关 / DNS / Wi-Fi / 系统代理 / 出口 IP / 延迟
    pub net: NetInfoJson,

    pub disk_active: f64,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub volumes: Vec<VolumeInfo>,

    /// 磁盘账本：今日读写 / 24h 与 30 天桶
    pub disk_today_read: u64,
    pub disk_today_write: u64,
    pub disk_hours: Vec<DiskBucket>,
    pub disk_days: Vec<DiskBucket>,

    pub battery: BatteryJson,
    pub battery_history: Vec<BatPoint>,
    pub bluetooth: BtJson,

    /// CPU / GPU / 系统盘温度（℃）；拿不到就是 null
    pub cpu_temp_c: Option<f64>,
    pub gpu_temp_c: Option<f64>,
    pub cpu_temp_source: Option<String>,
    pub disk_temp_c: Option<f64>,
    /// 主板 / 风扇只有外部传感器（LHM）能给
    pub mb_temp_c: Option<f64>,
    pub fan_rpm: Option<f64>,
    /// 温度来源：integrated = 内置 LHM，builtin = IOCTL / NVML 回退
    pub sensor_src: Option<String>,
    pub hardware_status: crate::hardware::Status,

    /// 当前电源计划（平衡 / 高性能 / 节能 / 卓越性能，或系统给的自定义名）
    pub power_plan: Option<String>,
    /// 页面文件（交换）已用 / 上限，单位 GB
    pub swap_used_gb: Option<f64>,
    pub swap_total_gb: Option<f64>,

    /// 本机信息（静态，每秒一起带过来）
    pub sys: SysJson,

    pub procs: Vec<ProcJson>,
    pub app_io:crate::app_io::Summary,
    pub physical_disks:Vec<crate::storage::PhysicalDisk>,
    pub uptime_sec: u64,
    pub proc_count: u32,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "snake_case")]
pub struct SysJson {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub build: Option<String>,
    pub arch: Option<String>,
    pub board: Option<String>,
    pub bios: Option<String>,
    pub cpu_physical_cores: Option<u32>,
    pub cpu_logical_cores: Option<u32>,
    /// 开机时刻（unix 秒）
    pub boot_ts: Option<i64>,
    /// 主板上的内存插槽数量；固件没给就是 None。
    pub mem_slots: Option<u32>,
    /// 逐条物理内存。空插槽也会出现，容量为 0。
    pub mem_modules: Vec<MemModuleJson>,
}

/// 一条物理内存的固件描述。品牌和频率来自 SMBIOS 类型 17，
/// 与 Windows 报告的可用容量可能不同（核显预留、硬件保留），
/// 所以这里只描述「装了什么」，不参与任何占用率计算。
#[derive(Serialize, Clone, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct MemModuleJson {
    pub brand: Option<String>,
    pub part: Option<String>,
    pub slot: Option<String>,
    /// 装机容量，单位 GB；0 表示插槽是空的。
    pub capacity_gb: f64,
    /// 标称或实际运行频率，单位 MT/s。
    pub speed_mts: Option<u32>,
    /// 归一化后的类型名，如 DDR5、LPDDR5。
    pub kind: Option<String>,
    /// DIMM / SODIMM 这类封装形式。
    pub form: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct NetInterfaceJson {
    pub index:u32,pub name:String,pub description:String,pub mtu:u32,pub is_tun:bool,
    pub mac:Option<String>,pub ipv4:Option<String>,pub ipv6:Option<String>,
    pub gateways:Vec<String>,pub dns:Vec<String>,pub link_mbps:Option<f64>,pub kind:String,pub up:bool,
}
#[derive(Serialize, Clone)]
pub struct NetRouteJson {
    pub target:String,pub interface_index:u32,pub if_name:String,pub next_hop:Option<String>,
    pub source:Option<String>,pub prefix_len:u8,pub is_tun:bool,
}
#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct NetInfoJson {
    pub interfaces:Vec<NetInterfaceJson>,pub routes:Vec<NetRouteJson>,pub pac_url:Option<String>,
    pub proxy_ip:Option<String>,pub proxy_at:i64,pub proxy_error:Option<String>,pub egress_error:Option<String>,pub proxy_path:Option<String>,
    /// 内网 IPv4（主网卡）
    pub local_ip: Option<String>,
    pub ipv6: Option<String>,
    pub if_name: Option<String>,
    pub mac: Option<String>,
    pub link_mbps: Option<f64>,
    /// Wi-Fi / 以太网 / 蜂窝 / 其他
    pub kind: Option<String>,
    pub gateways: Vec<String>,
    pub dns: Vec<String>,
    pub wifi_ssid: Option<String>,
    pub wifi_signal: Option<u32>,
    pub wifi_rate: Option<f64>,
    pub proxy_enabled: bool,
    pub proxy_server: Option<String>,
    /// 国内回显服务，显式禁用 HTTP 代理；系统 TUN 仍可能接管
    pub egress_ip: Option<String>,
    pub egress_region: Option<String>,
    pub egress_isp: Option<String>,
    pub egress_via_proxy: bool,
    pub egress_at: i64,
    /// 到网关（或 223.5.5.5）的往返延迟
    pub rtt_ms: Option<f64>,
    pub rtt_target: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct GpuJson {
    pub name: String,
    pub temp_c: Option<f64>,
    pub usage: f64,
    pub mem_used_gb: f64,
    pub mem_total_gb: f64,
    pub discrete: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct AdapterInfo {
    pub name: String,
    pub down_bps: f64,
    pub up_bps: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct VolumeInfo {
    pub mount: String,
    pub label: Option<String>,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct BatteryJson {
    pub present: bool,
    pub charging: bool,
    pub ac_line: bool,
    pub percent: Option<f64>,
    pub remaining_sec: Option<u32>,
    pub rate_w: Option<f64>,
    /// 带符号：正 = 充入电池，负 = 电池放电
    pub power_w: Option<f64>,
    /// 仅为 true 时 power_w 可作为有效的驱动报告值
    pub power_sensor: bool,
    /// true 表示 remaining_sec 由「容量 ÷ 功率」倒推，不是系统报告值
    pub remain_est: bool,
    pub design_capacity_mwh: Option<u32>,
    pub full_capacity_mwh: Option<u32>,
    pub cycle_count: Option<u32>,
    pub name: Option<String>,
    pub chemistry: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct BtJson {
    pub connected: u32,
    pub remembered: u32,
    /// 适配器型号（设备管理器里的名字）
    pub adapter: Option<String>,
    /// 无线电名称（Windows 蓝牙设置里可改，默认等于计算机名）
    pub radio: Option<String>,
    pub devices: Vec<BtDeviceJson>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct BtDeviceJson {
    pub name: String,
    pub connected: bool,
    pub remembered: bool,
    pub kind: String,
    pub last_used_sec: Option<i64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct ProcJson {
    pub name: String,
    pub pid: u32,
    pub cpu: Option<f64>,
    pub mem_bytes: Option<u64>,
    pub disk_read_bps: Option<f64>,
    pub disk_write_bps: Option<f64>,
    pub disk_ops: Option<f64>,
    pub gpu_percent: Option<f64>,
}

/// 采集器：维护跨采样的增量状态。
#[cfg(windows)]
pub struct Collector {
    imp: imp::CollectorState,
    static_info: imp::StaticInfo,
    battery_details: (Option<String>, Option<String>, Option<u32>, Option<u32>),
    battery_health: (Option<u32>, Option<u32>, Option<u32>),
    bt_model: Option<String>,
    sys: SysJson,
    probe: imp::NetProbe,
    ledger: DiskLedger,
    last_tick_ms: u64,
    last_bat_push: i64,
    last_gpu_refresh: i64,
}

#[cfg(windows)]
impl Collector {pub fn flush_history(&self)->Result<(),String>{self.ledger.save()}}

#[cfg(windows)]
pub fn new_collector() -> Collector {
    let static_info = imp::read_static_info();
    let mut pdh = imp::Pdh::new();
    if let Some(p) = pdh.as_mut() {
        p.add("\\PhysicalDisk(_Total)\\% Idle Time");
        p.add("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec");
        p.add("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec");
        p.add("\\Memory\\Standby Cache Core Bytes");
        p.add("\\Memory\\Standby Cache Normal Priority Bytes");
        p.add("\\Memory\\Standby Cache Reserve Bytes");
        p.add("\\Memory\\Modified Page List Bytes");
        imp::add_gpu_counters(p);
        let _ = p.collect();
    }
    let battery_details = imp::battery_wmi_details();
    let sys = imp::read_sys_info();
    Collector {
        imp: imp::CollectorState {
            pdh,
            cpu: imp::CpuState { prev_idle: 0, prev_total: 0, prev_core: Vec::new() },
            procs: std::collections::HashMap::new(),
            net: std::collections::HashMap::new(),
            session: (0, 0),
            temps: imp::Temps::new(),
            netinfo: imp::NetInfo::new(),
        },
        static_info,
        battery_details,
        battery_health: imp::battery_health(),
        bt_model: imp::bt_adapter_model(),
        sys,
        probe: imp::NetProbe::spawn(),
        ledger: DiskLedger::load(),
        last_tick_ms: 0,
        last_bat_push: 0,
        last_gpu_refresh: now_unix(),
    }
}

#[cfg(windows)]
pub fn collect(c: &mut Collector) -> Sample {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::SystemInformation::GetTickCount64;
    use windows::Win32::System::Threading::GetSystemTimes;

    let st = &c.static_info;
    if now_unix() - c.last_gpu_refresh >= 30 {
        if let Some(pdh) = c.imp.pdh.as_mut() { imp::add_gpu_counters(pdh); }
        c.last_gpu_refresh = now_unix();
    }
    let tick = unsafe { GetTickCount64() };
    let dt = if c.last_tick_ms > 0 && tick > c.last_tick_ms {
        (tick - c.last_tick_ms) as f64 / 1000.0
    } else { 1.0 };
    c.last_tick_ms = tick;

    // 系统总 CPU 时间（100ns 单位，用于进程 CPU% 归一化）
    let sys_total = unsafe {
        let mut idle = FILETIME::default();
        let mut k = FILETIME::default();
        let mut u = FILETIME::default();
        if GetSystemTimes(Some(&mut idle), Some(&mut k), Some(&mut u)).is_ok() {
            let f = |x: &FILETIME| -> u64 { ((x.dwHighDateTime as u64) << 32) | x.dwLowDateTime as u64 };
            f(&k) + f(&u)
        } else {
            0
        }
    };

    let (cpu_usage, cores) = imp::sample_cpu(&mut c.imp.cpu, st.core_count);
    let (
        mem_total_gb,
        mem_load,
        mem_used_gb,
        mem_in_use_gb,
        mem_standby_gb,
        mem_modified_gb,
        mem_cached_gb,
        mem_available_gb,
        commit_gb,
        commit_limit_gb,
        pool_paged_gb,
    ) = imp::sample_mem(&c.imp.pdh);
    let disk = imp::sample_disk(&c.imp.pdh);
    let (net_down_bytes, net_up_bytes, adapters) = imp::sample_net(&mut c.imp.net, &mut c.imp.session);
    let net_down_bps = net_down_bytes / dt;
    let net_up_bps = net_up_bytes / dt;
    let gpu = imp::sample_gpu(&c.imp.pdh);

    // 每张卡的用量按 LUID 归位；万一 PDH 实例名对不上 LUID，就把合计记到主卡上
    let mut gpus: Vec<GpuJson> = st
        .gpus
        .iter()
        .map(|a| {
            let key = (a.luid_high, a.luid_low);
            GpuJson {
                name: a.name.clone(),
                temp_c: None,
                usage: gpu.by_luid.get(&key).copied().unwrap_or(0.0),
                mem_used_gb: gpu.mem_by_luid.get(&key).copied().unwrap_or(0) as f64 / GB,
                mem_total_gb: a.dedicated_bytes as f64 / GB,
                discrete: a.discrete,
            }
        })
        .collect();
    // 主卡：优先独显（专用显存最大的那张），否则取第一张
    let primary = gpus
        .iter()
        .enumerate()
        .filter(|(_, g)| g.discrete)
        .max_by(|a, b| {
            a.1.mem_total_gb
                .partial_cmp(&b.1.mem_total_gb)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0);
    if gpu.by_luid.is_empty() && gpu.total > 0.0 {
        if let Some(g) = gpus.get_mut(primary) {
            g.usage = gpu.total;
            g.mem_used_gb = gpu.mem_used as f64 / GB;
        }
    }
    let prim = gpus.get(primary);
    let gpu_usage = prim.map(|g| g.usage).unwrap_or(gpu.total);
    let gpu_name = prim.map(|g| g.name.clone());
    let gpu_mem_used = prim.map(|g| g.mem_used_gb * GB as f64).unwrap_or(gpu.mem_used as f64);
    let gpu_mem_total = prim.map(|g| g.mem_total_gb * GB as f64).unwrap_or(0.0);

    // 温度：CPU 只用已识别的内置传感器；GPU / 磁盘可回退到 NVML / storage 属性。
    // 优先使用 WinMeter 内置采集进程；没有数据时回落到系统接口。
    let (native_tree,hardware_status)=crate::hardware::snapshot();
    let native=native_tree.as_ref().map(imp::parse_sensor_tree).unwrap_or_default();
    let lhm=native.clone();
    let primary_name = gpu_name.clone().unwrap_or_default();
    c.imp.temps.sample(&primary_name, now_unix(), &lhm);
    for (index,g) in gpus.iter_mut().enumerate() {
        g.temp_c=lhm.gpu_by_name.get(&g.name).copied().or_else(||if index==primary {c.imp.temps.gpu} else {c.imp.temps.gpu_temperature(&g.name)});
    }
    // 本机 IP / 网关 / DNS / Wi-Fi / 系统代理：5 秒刷一次
    c.imp.netinfo.sample(now_unix());

    // 磁盘账本：用采样间隔 × 速率 累加到小时桶 / 日桶
    let now = now_unix();
    c.ledger.add_disk(
        (disk.read_bps * dt) as u64,
        (disk.write_bps * dt) as u64,
        now,
    );

    if let Some(p) = c.imp.pdh.as_mut() {
        let _ = p.collect();
    }

    let (procs, proc_count) = imp::sample_procs(&mut c.imp.procs, sys_total, dt);
    for p in &procs {c.ledger.data.apps.add(day_key(now,c.ledger.offset),&p.process.name,p.path.as_deref(),p.read,p.write);}
    let battery = imp::sample_battery();
    let (bt_connected, bt_remembered, bt_devices, bt_radio) = imp::sample_bluetooth();
    let bt_adapter = c.bt_model.clone().or_else(|| bt_radio.clone());

    // 电量曲线：最多每 30s 落一个点
    if let Some(pct) = battery.percent {
        if c.last_bat_push == 0 || now - c.last_bat_push >= 30 {
            c.ledger.add_battery(pct, battery.ac_line, now);
            c.last_bat_push = now;
        }
    }
    c.ledger.maybe_save(now);
    let (today_read, today_write) = c.ledger.today(now);

    let (bname, bchem, bdesign, bfull) = c.battery_details.clone();
    let (hdesign, hfull, hcycles) = c.battery_health;
    let design = bdesign.or(hdesign);
    let full = hfull.or(bfull).or(battery.full_capacity_mwh);

    let pagefiles = imp::sample_pagefiles();
    Sample {
        cpu_usage,
        cores,
        core_count: st.core_count,
        cpu_name: st.cpu_name.clone(),
        cpu_base_mhz: st.cpu_base_mhz,

        gpu_usage,
        gpu_name,
        gpu_mem_used_gb: gpu_mem_used / GB as f64,
        gpu_mem_total_gb: gpu_mem_total / GB as f64,
        gpus,

        mem_total_gb,
        mem_used_gb,
        mem_load,
        mem_in_use_gb,
        mem_standby_gb,
        mem_modified_gb,
        mem_cached_gb,
        mem_available_gb,
        commit_gb,
        commit_limit_gb,
        pool_paged_gb,

        net_down_bps,
        net_up_bps,
        net_session_down_bytes: c.imp.session.0,
        net_session_up_bytes: c.imp.session.1,
        adapters: adapters
            .into_iter()
            .map(|(name, down, up)| AdapterInfo { name, down_bps: down / dt, up_bps: up / dt })
            .collect(),

        net: {
            let g = c.probe.egress.lock().unwrap_or_else(|p| p.into_inner());
            let n = &c.imp.netinfo;
            NetInfoJson {
                interfaces:n.interfaces.clone(),routes:n.routes.clone(),pac_url:n.pac_url.clone(),
                proxy_ip:if (n.proxy_enabled || n.interfaces.iter().any(|i|i.up&&i.is_tun)) && n.proxy_server==g.proxy_server {g.proxy_ip.clone()}else{None},
                proxy_path:g.proxy_path.clone(),
                proxy_at:g.proxy_at,proxy_error:g.proxy_error.clone(),egress_error:g.error.clone(),
                local_ip: n.local_ip.clone(),
                ipv6: n.ipv6.clone(),
                if_name: n.if_name.clone(),
                mac: n.mac.clone(),
                link_mbps: n.link_mbps,
                kind: n.kind.clone(),
                gateways: n.gateways.clone(),
                dns: n.dns.clone(),
                wifi_ssid: n.wifi_ssid.clone(),
                wifi_signal: n.wifi_signal,
                wifi_rate: n.wifi_rate,
                proxy_enabled: n.proxy_enabled,
                proxy_server: n.proxy_server.clone(),
                egress_ip: g.ip.clone(),
                egress_region: g.region.clone(),
                egress_isp: g.isp.clone(),
                egress_via_proxy: g.via_proxy,
                egress_at: g.at,
                rtt_ms: g.rtt_ms,
                rtt_target: g.rtt_target.clone(),
            }
        },

        disk_active: disk.active,
        disk_read_bps: disk.read_bps,
        disk_write_bps: disk.write_bps,
        volumes: disk
            .volumes
            .into_iter()
            .map(|(mount, label, free_bytes, total_bytes)| VolumeInfo {
                mount,
                label,
                free_bytes,
                total_bytes,
            })
            .collect(),

        disk_today_read: today_read,
        disk_today_write: today_write,
        disk_hours: c.ledger.recent_hours(now),
        disk_days: c.ledger.recent_days(now),

        battery: BatteryJson {
            present: battery.present,
            charging: battery.charging,
            ac_line: battery.ac_line,
            percent: battery.percent,
            remaining_sec: battery.remaining_sec,
            rate_w: battery.rate_w,
            power_w: battery.power_w,
            power_sensor: battery.power_sensor,
            remain_est: battery.remain_est,
            design_capacity_mwh: design,
            full_capacity_mwh: full,
            cycle_count: hcycles,
            name: bname,
            chemistry: bchem,
        },
        battery_history: c.ledger.data.battery.clone(),
        bluetooth: BtJson {
            connected: bt_connected,
            remembered: bt_remembered,
            adapter: bt_adapter,
            radio: bt_radio,
            devices: bt_devices
                .into_iter()
                .map(|(name, connected, remembered, kind, last_used_sec)| BtDeviceJson {
                    name,
                    connected,
                    remembered,
                    kind,
                    last_used_sec,
                })
                .collect(),
        },

        cpu_temp_c: c.imp.temps.cpu,
        cpu_temp_source: if native.cpu.is_some() {Some("WinMeter 内置硬件采集 · PawnIO".into())} else {None},
        gpu_temp_c: c.imp.temps.gpu,
        disk_temp_c: c.imp.temps.disk,
        mb_temp_c: lhm.mainboard,
        fan_rpm: lhm.fan_rpm,
        sensor_src: Some(if native.online {"integrated"} else { "builtin" }.to_string()),
        hardware_status,

        power_plan: imp::power_plan(),
        swap_used_gb: pagefiles.map(|p| p.0),
        swap_total_gb: pagefiles.map(|p| p.1),

        sys: {
            let mut sj = c.sys.clone();
            // 开机时刻 = 现在 - 已运行秒数
            sj.boot_ts = Some(now - (unsafe { GetTickCount64() } / 1000) as i64);
            sj
        },

        app_io:c.ledger.data.apps.summary(day_key(now,c.ledger.offset)),
        physical_disks:crate::storage::parse(native_tree.as_ref()),
        procs:procs.into_iter().map(|mut p|{p.process.gpu_percent=gpu.by_pid.get(&p.process.pid).copied();p.process}).collect(),

        uptime_sec: unsafe { GetTickCount64() / 1000 },
        proc_count,
    }
}


#[cfg(all(test, windows))]
mod probe {
    use super::*;
    #[test]
    fn stages() {
        eprintln!("stage: static");
        let si = imp::read_static_info();
        eprintln!("stage: pdh new");
        let mut pdh = imp::Pdh::new();
        if let Some(p) = pdh.as_mut() {
            p.add("\\PhysicalDisk(_Total)\\% Idle Time");
            p.add("\\Memory\\Standby Cache Core Bytes");
            imp::add_gpu_counters(p);
            let _ = p.collect();
        }
        eprintln!("stage: battery wmi");
        let _ = imp::battery_wmi_details();
        eprintln!("stage: cpu");
        let mut cs = imp::CpuState { prev_idle: 0, prev_total: 0, prev_core: Vec::new() };
        let _ = imp::sample_cpu(&mut cs, si.core_count);
        eprintln!("stage: mem");
        let _ = imp::sample_mem(&pdh);
        eprintln!("stage: disk");
        let _ = imp::sample_disk(&pdh);
        eprintln!("stage: net");
        let mut net = std::collections::HashMap::new();
        let mut sess = (0u64, 0u64);
        let _ = imp::sample_net(&mut net, &mut sess);
        eprintln!("stage: gpu");
        let _ = imp::sample_gpu(&pdh);
        eprintln!("stage: procs");
        let mut prev = std::collections::HashMap::new();
        let _ = imp::sample_procs(&mut prev, 1000, 1.0);
        eprintln!("stage: battery");
        let _ = imp::sample_battery();
        eprintln!("stage: bt");
        let _ = imp::sample_bluetooth();
        eprintln!("stage: done");
    }
}

/// 调试用：`cargo test dump_sample -- --nocapture` 打印一次真实采样，
/// 方便核对前端字段名和取值是否对得上。
#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn smbios_memory_reads_brand_speed_and_empty_slots(){
        // 按 SMBIOS 规范手工拼一张最小固件表：一个内存阵列 + 一条已装 +
        // 一个空插槽，用来固定偏移量和字符串索引的解析行为。
        fn block(kind:u8,body:Vec<u8>,strings:&[&str])->Vec<u8>{
            let mut v=vec![kind,(4+body.len()) as u8,0,0];
            v.extend_from_slice(&body);
            if strings.is_empty(){v.extend_from_slice(&[0,0]);}
            else{for s in strings{v.extend_from_slice(s.as_bytes());v.push(0);}v.push(0);}
            v
        }
        // 类型 16：NumberOfMemoryDevices 在偏移 0x0D
        let mut array=vec![0u8;0x13];
        array[0x0D-4..0x0F-4].copy_from_slice(&2u16.to_le_bytes());
        // 类型 17：16 GB / 6400 MT/s / DDR5 / SODIMM
        let mut dimm=vec![0u8;0x24];
        dimm[0x0C-4..0x0E-4].copy_from_slice(&0x4000u16.to_le_bytes());
        dimm[0x0E-4]=0x0D;
        dimm[0x10-4]=1;
        dimm[0x12-4]=0x22;
        dimm[0x15-4..0x17-4].copy_from_slice(&6400u16.to_le_bytes());
        dimm[0x17-4]=2;
        dimm[0x1A-4]=3;
        // 类型 17：空插槽，只有槽位名
        let mut empty=vec![0u8;0x24];
        empty[0x0E-4]=0x0D;
        empty[0x10-4]=1;
        empty[0x12-4]=0x22;

        let mut table=block(16,array,&[]);
        table.extend(block(17,dimm,&["Controller0-ChannelA-DIMM1","Micron Technology","MTC8C1084S1VC64BD1 B"]));
        table.extend(block(17,empty,&["Controller0-ChannelB-DIMM1"]));
        table.extend(block(127,vec![],&[]));

        let (modules,slots)=super::imp::parse_smbios_memory(&table);
        assert_eq!(slots,Some(2));
        assert_eq!(modules.len(),2);
        assert_eq!(modules[0].capacity_gb,16.0);
        assert_eq!(modules[0].brand.as_deref(),Some("Micron"));
        assert_eq!(modules[0].speed_mts,Some(6400));
        assert_eq!(modules[0].kind.as_deref(),Some("DDR5"));
        assert_eq!(modules[0].form.as_deref(),Some("SODIMM"));
        assert_eq!(modules[0].slot.as_deref(),Some("Controller0-ChannelA-DIMM1"));
        assert_eq!(modules[1].capacity_gb,0.0);
        assert_eq!(modules[1].brand,None);
        assert_eq!(modules[1].slot.as_deref(),Some("Controller0-ChannelB-DIMM1"));
    }

    #[test]
    fn memory_brand_drops_vendor_suffixes_and_unknown_placeholders(){
        use super::imp::clean_brand;
        assert_eq!(clean_brand("Micron Technology").as_deref(),Some("Micron"));
        assert_eq!(clean_brand("Kingston Technology").as_deref(),Some("Kingston"));
        assert_eq!(clean_brand("SK Hynix Semiconductor").as_deref(),Some("SK hynix"));
        assert_eq!(clean_brand("Samsung Electronics").as_deref(),Some("Samsung"));
        assert_eq!(clean_brand("A-DATA Technology").as_deref(),Some("ADATA"));
        assert_eq!(clean_brand("Unknown"),None);
        assert_eq!(clean_brand(""),None);
        assert_eq!(clean_brand("12345678"),None);
        // 没见过的厂商保留原名，太长才截断
        assert_eq!(clean_brand("Fancy Memory Lab").as_deref(),Some("Fancy Memory Lab"));
        assert_eq!(clean_brand("Some Very Long Vendor Name Here").as_deref(),Some("Some Very Long…"));
    }

    #[test]
    fn final_save_persists_short_session_and_replaces_existing_history(){
        use super::*;
        let directory=std::env::temp_dir().join(format!("winmeter-final-flush-{}",std::process::id()));
        let path=directory.join("history.json");
        let now=now_unix();
        let mut ledger=DiskLedger{data:LedgerFile::default(),offset:0,session_read:0,session_write:0,last_save:now};
        ledger.add_disk(1024,2048,now);ledger.add_battery(81.0,true,now);
        ledger.data.apps.add(day_key(now,0),"writer.exe",Some("C:\\writer.exe"),100,200);
        assert_eq!(ledger.last_save,now);
        ledger.save_to(&path).unwrap();
        let first:LedgerFile=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!((first.days[0].read,first.days[0].write),(1024,2048));assert_eq!(first.battery[0].pct,81.0);
        assert_eq!(first.apps.summary(day_key(now,0)).today[0].write,200);
        ledger.add_disk(10,20,now+1);ledger.save_to(&path).unwrap();
        let second:LedgerFile=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!((second.days[0].read,second.days[0].write),(1034,2068));
        std::fs::remove_file(path).unwrap();std::fs::remove_dir(directory).unwrap();
    }
    #[test]
    fn application_io_observes_a_known_write_and_resolves_its_executable(){
        let mut previous=std::collections::HashMap::new();
        let _=super::imp::sample_procs(&mut previous,1,1.0);
        let path=std::env::temp_dir().join(format!("winmeter-io-test-{}",std::process::id()));
        std::fs::write(&path,vec![5u8;65536]).unwrap();
        let (readings,_)=super::imp::sample_procs(&mut previous,2,1.0);
        let this=readings.iter().find(|p|p.process.pid==std::process::id()).unwrap();
        assert!(this.write>=65536);assert!(this.path.as_ref().is_some_and(|p|p.ends_with(".exe")));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn previous_history_files_load_without_application_records(){
        let ledger:super::LedgerFile=serde_json::from_str(r#"{"hours":[],"days":[],"battery":[]}"#).unwrap();
        assert!(ledger.apps.days.is_empty());
    }
    #[test]
    fn dump_sample() {
        let mut c = super::new_collector();
        let mut s = super::collect(&mut c);
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(1100));
            s = super::collect(&mut c);
        }
        let j = serde_json::to_string_pretty(&s).unwrap();
        println!("{}", j);
        assert!(j.contains("\"cpu_usage\""));
        assert!(j.contains("\"disk_hours\""));
    }
}
