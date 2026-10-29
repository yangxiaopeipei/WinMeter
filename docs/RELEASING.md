# 构建与发布

## 开发环境

- Windows x64；Node.js 22+；Rust MSVC 工具链。
- Visual Studio C++ Build Tools 和 Windows SDK。
- .NET Framework 4.7.2+；系统 C# 编译器位于 `%WINDIR%\Microsoft.NET\Framework64\v4.0.30319\csc.exe`。
- 首次构建需联网；硬件 DLL 已固定在源码仓库，不需要手动下载或复制。

```powershell
npm ci
npm run release
```

发布脚本验证 package.json、package-lock.json、Cargo.toml、Cargo.lock 与 Tauri 配置版本一致，运行 Node / Rust 测试，通过后构建 NSIS 安装包与便携包。安装包内置完整 WebView2 安装器，因此体积较大。

## 发布内容

将以下文件作为 GitHub `v0.1.0` Release 附件上传，Release 标题使用 `WinMeter 0.1.0`，正文可使用 CHANGELOG.md 中对应内容：

- `dist/WinMeter-0.1.0-windows-x64-setup.exe`
- `dist/WinMeter-0.1.0-windows-x64-portable.zip`
- `dist/SHA256SUMS.txt`

源码仓库只提交 src、src-tauri（排除 target、gen/schemas、hardware/runtime）、scripts、tests、tools 脚本、docs、配置、依赖锁文件与说明。不要提交 dist、node_modules、截图缓存、调试符号或本机助手记录。

校验下载文件：

```powershell
Get-FileHash .\WinMeter-0.1.0-windows-x64-setup.exe -Algorithm SHA256
```

## 新电脑验收

在干净的 Windows 10 22H2 / Windows 11 x64 虚拟机执行；本机编译和单元测试不能替代这些验收：

- 无 Node.js、Rust、VS、额外 .NET SDK 时运行安装包，首次启动正常。
- 缺少 WebView2、断网时安装，确认离线运行时完成安装。
- 普通用户启动、CPU / 内存 / 磁盘 / 网络实时变化、托盘与任务栏操作正常。
- 不安装 PawnIO 时基础监控正常、缺失传感器明确提示；安装官方 PawnIO 后验证该机器支持的温度。
- 安装 / 停用 / 恢复 / 卸载温度服务；取消管理员授权时应用继续工作。
- 从旧版本升级，设置与历史仍在；卸载前移除可选温度服务，卸载后无应用开机自启残留。
- 非 ASCII 安装路径、不同 DPI、任务栏主题切换、重启电脑后启动。

当前未配置代码签名，发布的安装包可能显示未知发布者；如需签名，应使用自己的正式证书在构建流程中配置。不要将私钥提交到仓库。

项目自身采用 MIT 许可证；发布时保留 LICENSE，以及 hardware/ 中的第三方许可证和声明。

## 提交到 GitHub

项目仓库：https://github.com/yangxiaopeipei/WinMeter 。首次上传可参考以下命令；已有远程地址时无需重复添加 origin：

```powershell
git add .
git commit -m "Release 0.1.0"
git remote add origin "https://github.com/yangxiaopeipei/WinMeter.git"
git push -u origin main
git tag v0.1.0
git push origin v0.1.0
```

随后创建 v0.1.0 Release 并上传 dist 中的安装包、便携包和 SHA256SUMS.txt。dist 只作为发布附件，不提交到源码仓库。源码压缩包可作为额外下载项。

本次检查结果和剩余限制见 [VALIDATION.md](VALIDATION.md)。
