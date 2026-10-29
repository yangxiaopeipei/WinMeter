# 硬件采集辅助程序

- `Program.cs`：只读传感器采集；`Service.cs`：可选 Windows 服务。
- `vendor/`：LibreHardwareMonitor v0.9.6 的固定运行依赖及绑定配置，需要纳入源码仓库。
- `licenses/`：随二进制分发的上游许可证与声明，需要纳入源码仓库。
- `runtime/`：由 build.ps1 生成，忽略提交。不要手动向此目录添加依赖。

Rust build.rs 自动运行 build.ps1，并将完整 runtime 复制到构建产物旁的 hardware 目录。Tauri 安装器使用同一 runtime 作为资源。

运行需要 .NET Framework 4.7.2+，不需要 .NET SDK。需要底层温度时可能还需安装官方 PawnIO 驱动。服务操作需要管理员授权，普通界面无需提权。
