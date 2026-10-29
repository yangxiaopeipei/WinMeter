# WinMeter

Windows 系统资源监控工具，基于 Tauri 2、Rust 和原生 HTML / CSS / JavaScript。当前版本：**0.1.0**。

## 功能

- CPU、显卡、内存、磁盘、网络、电池与传感器详情。
- 任务栏显示实时网速、CPU、内存和温度，可调整顺序、字体与间距。
- 托盘资源速览，展示最近两分钟曲线，并可跳转到完整界面。
- 磁盘写入历史、物理磁盘信息与设备提供的 SMART 状态。
- 浅色 / 深色主题、采样间隔、托盘驻留与开机自启设置。
- 可选温度采集服务，安装后无需每次启动重复授权。

## 下载与安装

在 [GitHub Releases](https://github.com/yangxiaopeipei/WinMeter/releases) 中下载 `WinMeter-0.1.0-windows-x64-setup.exe`，运行安装向导，然后从开始菜单启动 WinMeter。

**支持环境：Windows 10 22H2 / Windows 11，x64。** 不提供 Windows 7、32 位或 ARM64 原生安装包。

安装包包含传感器辅助程序、依赖 DLL 和 WebView2 离线安装器。缺少 WebView2 时由安装向导安装，无需用户另行下载；运行 WinMeter 无需安装 Node.js、Rust、Visual Studio 或 .NET SDK。传感器使用 .NET Framework 4.7.2 或更新版本，安装向导会检查该组件。

首次启动以普通权限运行。需要 CPU 温度等底层传感器时，在 **设置 → 硬件传感器** 中安装“自动温度采集服务”，按 Windows 提示授权。部分硬件还需要 [PawnIO 驱动](https://github.com/namazso/PawnIO)。驱动是可选组件，本安装包不自动安装；缺失时基础监控仍可使用，部分温度或风扇读数显示 `—`。不同硬件能提供的传感器不同。

便携版 `WinMeter-0.1.0-windows-x64-portable.zip` 需完整解压后启动 `winmeter.exe`，请保留同级 `hardware/` 文件夹。便携版不安装 WebView2，适用于已具备运行环境的电脑；新电脑优先使用安装版。

## 使用说明

- 首次运行可在设置中启用任务栏监控，点击监控文字或左键点击托盘图标打开速览。
- 默认关闭窗口后驻留托盘，完整退出请使用托盘右键菜单中的“退出”。
- 设置保存在 `%APPDATA%\WinMeter\settings.json`，历史保存在同目录 `diskuse.json`。
- 温度服务名为 `WinMeterSensors`。如已安装，卸载应用前先在设置中卸载该服务，需要管理员授权；服务文件可能保留在 `%ProgramFiles%\WinMeter\Sensors`，供手动清理。
- 应用卸载保留用户设置与历史，不移除共享的 WebView2、.NET Framework 或 PawnIO。

## 数据与限制

- 缺少传感器或 SMART 数据时显示未提供，不以零读数或剩余容量推断设备健康。
- 进程 I/O 包含缓存，不能等同于物理磁盘读写；历史只累计程序运行期间观察到的数据。
- 任务栏监控适配 Windows 11 原生、经典和 StartAllBack 任务栏，暂不支持竖向和副屏任务栏。空间不足时会精简临时显示。
- 指标与历史保存在本机。公网 IP / 地区查询访问 `myip.ipip.net`，代理出口查询访问 `api.ipify.org`，延迟探测使用公共 DNS 或网关；不上传进程列表或历史记录。

## 从源码构建

在 Windows x64 上安装 Node.js 22+、Rust MSVC 工具链、Visual Studio C++ Build Tools（含 Windows SDK）。传感器编译使用系统自带的 .NET Framework C# 编译器。

```powershell
npm ci
npm run dev
```

运行测试与生成发布文件：

```powershell
npm test
npm run test:rust
npm run release
```

`npm run release` 会运行测试、构建安装包和便携包，并生成 SHA256 校验文件，结果在 `dist/`。首次构建需要联网获取锁定依赖、NSIS 工具和微软 WebView2 离线安装器。

## 目录

```text
src/                     前端页面、样式和脚本
src-tauri/src/           Rust 采集器、窗口与任务栏逻辑
src-tauri/hardware/      C# 传感器、固定版本依赖和第三方声明
src-tauri/installer/     Windows 安装器检查
src-tauri/icons/         应用与托盘图标
scripts/                 发布脚本
tests/                   前端测试
docs/                    构建与发布检查说明
```

详见 [构建与发布](docs/RELEASING.md)、[第三方依赖](THIRD_PARTY_NOTICES.md) 和 [更新记录](CHANGELOG.md)。

## 许可证

项目代码采用 [MIT 许可证](LICENSE)。第三方组件按其各自许可证分发，详见 [第三方依赖](THIRD_PARTY_NOTICES.md)。
