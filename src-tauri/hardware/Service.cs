using System;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.ServiceProcess;
using System.Threading;

internal sealed class SensorService : ServiceBase
{
    internal const string Name = "WinMeterSensors";
    private static readonly string InstallRoot = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "WinMeter");
    private static readonly string InstallDirectory = Path.Combine(InstallRoot, "Sensors");
    private static readonly string DataDirectory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "WinMeterSensors");
    internal static readonly string Output = Path.Combine(DataDirectory, "sensors.json");
    private static readonly string Executable = Path.Combine(InstallDirectory, "WinMeter.Sensors.exe");
    private static readonly string ImagePath = "\"" + Executable + "\" --service";
    private readonly ManualResetEvent stop = new ManualResetEvent(false);
    private Thread worker;

    private SensorService() { ServiceName = Name; AutoLog = false; CanShutdown = true; }
    protected override void OnStart(string[] args) {
        stop.Reset();
        worker = new Thread(() => Program.CollectService(stop)); worker.IsBackground = true; worker.Start();
    }
    protected override void OnStop() {
        stop.Set();
        if (worker != null && !worker.Join(15000)) throw new System.TimeoutException("采集线程未能及时停止");
        if (File.Exists(Output)) File.Delete(Output);
    }
    protected override void OnShutdown() { OnStop(); }

    // All elevated operations have fixed names, paths and arguments. No shell,
    // IPC command execution, passwords, network listeners or user paths.
    internal static int Dispatch(string mode) {
        if (mode == "--service") { ServiceBase.Run(new SensorService()); return 0; }
        if (mode != "--install-service" && mode != "--remove-service" && mode != "--disable-service" && mode != "--resume-service") return 2;
        if (!new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator)) return 5;
        try { Manage(mode); return 0; }
        catch (Exception error) {
            // A fixed protected diagnostic; never write elevated results to a
            // path supplied by an unprivileged caller.
            try { SecureDirectory(DataDirectory); File.WriteAllText(Path.Combine(DataDirectory, "setup-error.txt"), error.ToString()); } catch {}
            return 1;
        }
    }
    private static void RejectLinks(string path) {
        for (var current = new DirectoryInfo(path); current != null; current = current.Parent)
            if (current.Exists && (current.Attributes & FileAttributes.ReparsePoint) != 0) throw new IOException("安装路径不能包含链接");
        if (File.Exists(path) && (File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0) throw new IOException("文件不能是链接");
    }
    private static void SecureDirectory(string path, bool installParent = false) {
        RejectLinks(path); Directory.CreateDirectory(path);
        var security = new DirectorySecurity(); security.SetAccessRuleProtection(true, false);
        security.SetOwner(new SecurityIdentifier(WellKnownSidType.BuiltinAdministratorsSid, null));
        foreach (var sid in new[] { WellKnownSidType.LocalSystemSid, WellKnownSidType.BuiltinAdministratorsSid, WellKnownSidType.BuiltinUsersSid })
            security.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(sid, null),
                sid == WellKnownSidType.BuiltinUsersSid ? FileSystemRights.ReadAndExecute : FileSystemRights.FullControl,
                InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit, PropagationFlags.None, AccessControlType.Allow));
        Directory.SetAccessControl(path, security);
        foreach (var file in Directory.GetFiles(path)) {
            RejectLinks(file);
            var acl = new FileSecurity(); acl.SetAccessRuleProtection(true, false);
            acl.SetOwner(new SecurityIdentifier(WellKnownSidType.BuiltinAdministratorsSid, null));
            foreach (var sid in new[] { WellKnownSidType.LocalSystemSid, WellKnownSidType.BuiltinAdministratorsSid, WellKnownSidType.BuiltinUsersSid })
                acl.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(sid, null), sid == WellKnownSidType.BuiltinUsersSid ? FileSystemRights.ReadAndExecute : FileSystemRights.FullControl, AccessControlType.Allow));
            File.SetAccessControl(file, acl);
        }
        foreach (var directory in Directory.GetDirectories(path))
            if (!installParent || !String.Equals(directory, InstallDirectory, StringComparison.OrdinalIgnoreCase)) throw new IOException("服务目录包含未知子目录");
    }
    private static void StopExisting() {
        using (var service = new ServiceController(Name)) {
            if (service.Status == ServiceControllerStatus.Stopped) return;
            if (service.Status != ServiceControllerStatus.StopPending) service.Stop();
            service.WaitForStatus(ServiceControllerStatus.Stopped, TimeSpan.FromSeconds(30));
        }
    }
    private static void Manage(string mode) {
        IntPtr manager = OpenSCManager(null, null, 3);
        if (manager == IntPtr.Zero) throw new Win32Exception();
        IntPtr service = IntPtr.Zero;
        try {
            service = OpenService(manager, Name, 0xF01FF);
            bool existed = service != IntPtr.Zero;
            if (!existed && Marshal.GetLastWin32Error() != 1060) throw new Win32Exception();
            if (existed) {
                using (var key = Microsoft.Win32.Registry.LocalMachine.OpenSubKey(@"SYSTEM\CurrentControlSet\Services\" + Name))
                    if (key == null || !String.Equals(key.GetValue("ImagePath") as string, ImagePath, StringComparison.OrdinalIgnoreCase))
                        throw new IOException("同名服务不属于 WinMeter 的固定安装目录");
                StopExisting();
            }
            if (mode == "--install-service") {
                // Secure the parent before creating/copying service binaries.
                SecureDirectory(InstallRoot, true); SecureDirectory(InstallDirectory); SecureDirectory(DataDirectory);
                string source = Path.GetDirectoryName(System.Reflection.Assembly.GetExecutingAssembly().Location);
                foreach (var file in Directory.GetFiles(source)) {
                    string name = Path.GetFileName(file);
                    if (name != "WinMeter.Sensors.exe" && name != "WinMeter.Sensors.exe.config" && !name.EndsWith(".dll", StringComparison.OrdinalIgnoreCase) && !name.EndsWith(".txt", StringComparison.OrdinalIgnoreCase)) continue;
                    RejectLinks(file); string destination = Path.Combine(InstallDirectory, name); RejectLinks(destination);
                    if (!String.Equals(file, destination, StringComparison.OrdinalIgnoreCase)) {
                        for (int attempt = 0; ; attempt++) {
                            try { File.Copy(file, destination, true); break; }
                            catch (IOException) { if (attempt >= 10) throw; Thread.Sleep(200); }
                        }
                    }
                }
                SecureDirectory(InstallDirectory);
                if (!existed) {
                    service = CreateService(manager, Name, "WinMeter 温度采集", 0xF01FF, 0x10, 2, 1, ImagePath, null, IntPtr.Zero, null, "LocalSystem", null);
                    if (service == IntPtr.Zero) throw new Win32Exception();
                }
            }
            if (mode == "--remove-service") {
                if (existed && !DeleteService(service)) throw new Win32Exception();
                // Retain protected binaries for recovery; service registration
                // and automatic privileged execution are removed immediately.
                if (File.Exists(Output)) File.Delete(Output);
                return;
            }
            if (service == IntPtr.Zero) throw new IOException("请先安装自动采集服务");
            if (!ChangeServiceConfig(service, 0xFFFFFFFF, mode == "--disable-service" ? 4u : 2u, 0xFFFFFFFF, null, null, IntPtr.Zero, null, null, null, null)) throw new Win32Exception();
            if (mode != "--disable-service") {
                // Restart after an unexpected process crash; initialization
                // and transient sensor failures also retry within the worker.
                using (var recovery = Process.Start(new ProcessStartInfo {
                    FileName = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "sc.exe"),
                    Arguments = "failure WinMeterSensors reset= 86400 actions= restart/5000/restart/15000/restart/60000",
                    UseShellExecute = false, CreateNoWindow = true, WindowStyle = ProcessWindowStyle.Hidden
                })) {
                    if (!recovery.WaitForExit(10000) || recovery.ExitCode != 0) throw new IOException("配置采集服务自动恢复失败");
                }
                using (var controller = new ServiceController(Name)) {
                    controller.Start(); controller.WaitForStatus(ServiceControllerStatus.Running, TimeSpan.FromSeconds(30));
                }
            }
        } finally { if (service != IntPtr.Zero) CloseServiceHandle(service); CloseServiceHandle(manager); }
    }
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern IntPtr OpenSCManager(string machine, string database, uint access);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern IntPtr OpenService(IntPtr manager, string name, uint access);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern IntPtr CreateService(IntPtr manager, string name, string display, uint access, uint type, uint start, uint error, string binary, string group, IntPtr tag, string dependencies, string account, string password);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern bool ChangeServiceConfig(IntPtr service, uint type, uint start, uint error, string binary, string group, IntPtr tag, string dependencies, string account, string password, string display);
    [DllImport("advapi32.dll", SetLastError = true)] private static extern bool DeleteService(IntPtr service);
    [DllImport("advapi32.dll")] private static extern bool CloseServiceHandle(IntPtr handle);
}
