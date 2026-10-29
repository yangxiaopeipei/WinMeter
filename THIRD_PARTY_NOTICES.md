# 第三方依赖

WinMeter 的 JavaScript 构建依赖锁定于 `package-lock.json`，Rust 依赖锁定于 `src-tauri/Cargo.lock`。构建分别使用 `npm ci` 与 Cargo `--locked`。

## 硬件采集

`src-tauri/hardware/vendor/` 中的 DLL 与运行配置取自 LibreHardwareMonitor 官方 v0.9.6 发行包，未修改。仅保留构建和运行需要的文件。

- 上游源码：https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/tree/v0.9.6
- 上游发行页：https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases/tag/v0.9.6
- 原始下载包 SHA256：`086D9F1B5A99E643EDC2CFAAAC16051685B551E4C5AC0B32A57C58C0E529C001`
- LibreHardwareMonitor 许可证：MPL-2.0，完整文本见 `src-tauri/hardware/licenses/LibreHardwareMonitor-LICENSE.txt`。
- 上游第三方声明见 `src-tauri/hardware/licenses/THIRD-PARTY-NOTICES.txt`。

构建脚本将上述声明复制到 `hardware/`，随安装版与便携版一同分发。其余上游组件受各自许可证约束，以保留的官方声明为准。

## 运行环境

安装版包含微软 WebView2 Evergreen 离线安装器，由 Tauri 从微软获取，适用微软的运行时许可条款。该文件不进入源码仓库。

PawnIO 是按需安装的独立驱动，本项目不捆绑其安装程序。项目地址：https://github.com/namazso/PawnIO 。
