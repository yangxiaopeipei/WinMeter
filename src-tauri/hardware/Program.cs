using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Security.Principal;
using System.Threading;
using System.Web.Script.Serialization;
using LibreHardwareMonitor.Hardware;

// Read-only sensors. Interactive mode follows its owner; service mode uses
// fixed protected paths and accepts no caller-supplied collection commands.
internal static class Program
{
    private static readonly JavaScriptSerializer Json = new JavaScriptSerializer();
    private static readonly List<Dictionary<string, object>> Nodes = new List<Dictionary<string, object>>();
    private static readonly List<Dictionary<string, object>> Disks = new List<Dictionary<string, object>>();
    private static void Read(IHardware hardware)
    {
        hardware.Update();
        var disk = hardware as LibreHardwareMonitor.Hardware.Storage.StorageDevice;
        if (disk != null)
        {
            var storage = disk.Storage;
            var smart = storage.Smart;
            // The library clears nullable values to zero after failed SMART reads.
            // Only publish health counters when actual attributes were returned.
            bool valid = smart.SmartAttributes.Count > 0;
            var mounts = new List<string>();
            foreach (var partition in storage.Partitions)
                if (partition.DriveLetter.HasValue) mounts.Add(partition.DriveLetter.Value + ":");
            Disks.Add(new Dictionary<string, object> {
                {"id", hardware.Identifier.ToString()}, {"name", storage.Model}, {"number", storage.DriveNumber},
                {"kind", storage.IsNVMe ? "NVMe SSD" : storage.IsSSD ? "SSD" : "HDD"},
                {"bus", storage.BusType.ToString()}, {"firmware", storage.Firmware}, {"capacity_bytes", storage.TotalSize},
                {"mounts", mounts}, {"status", valid ? smart.DiskStatus.ToString() : "Unknown"},
                {"temperature_c", valid ? (object)smart.Temperature : null},
                {"life_percent", valid && (storage.IsSSD || storage.IsNVMe) ? (object)smart.Life : null},
                {"host_read_gb", valid ? (object)smart.HostReads : null}, {"host_write_gb", valid ? (object)smart.HostWrites : null},
                {"power_on_hours", valid ? (object)Math.Max(smart.MeasuredPowerOnHours, smart.DetectedPowerOnHours) : null}
            });
        }
        foreach (ISensor sensor in hardware.Sensors)
        {
            if (sensor.SensorType != SensorType.Temperature && sensor.SensorType != SensorType.Fan) continue;
            Nodes.Add(new Dictionary<string, object> {
                {"SensorId", sensor.Identifier.ToString()}, {"Text", sensor.Name}, {"Hardware", hardware.Name},
                {"Type", sensor.SensorType.ToString()}, {"RawValue", sensor.Value},
                {"Value", sensor.Value.HasValue ? sensor.Value.Value.ToString(System.Globalization.CultureInfo.InvariantCulture) + (sensor.SensorType == SensorType.Fan ? " RPM" : " °C") : ""}
            });
        }
        foreach (IHardware child in hardware.SubHardware) Read(child);
    }
    internal static void Write(string output, int parent, string error, bool admin, bool terminal = false)
    {
        var value = new Dictionary<string, object> {
            {"Version", 1}, {"Parent", parent}, {"Stamp", DateTimeOffset.UtcNow.ToUnixTimeSeconds()},
            {"Admin", admin}, {"Error", error}, {"Terminal", terminal}, {"Children", Nodes}, {"Disks", Disks}
        };
        string temporary = output + ".tmp";
        File.WriteAllText(temporary, Json.Serialize(value), new System.Text.UTF8Encoding(false));
        if (File.Exists(output)) File.Replace(temporary, output, null); else File.Move(temporary, output);
    }
    private static bool Alive(int parent)
    {
        try {using (Process process = Process.GetProcessById(parent)) {return !process.HasExited;}} catch {return false;}
    }
    public static int Main(string[] args)
    {
        if (args.Length == 1 && args[0].EndsWith("-service", StringComparison.Ordinal))
            return SensorService.Dispatch(args[0]);
        if (args.Length != 3 || (args[0] != "--watch" && args[0] != "--once")) return 2;
        int parent;
        if (!int.TryParse(args[1], out parent) || parent <= 0) return 2;
        string output = Path.GetFullPath(args[2]);
        bool admin = new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator);
        Computer computer = null;
        bool failed = false;
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(output));
            computer = new Computer { IsCpuEnabled = true, IsGpuEnabled = true, IsStorageEnabled = true, IsMotherboardEnabled = true };
            computer.Open();
            do
            {
                Nodes.Clear(); Disks.Clear(); string error = "";
                try {foreach (IHardware hardware in computer.Hardware) Read(hardware);}
                catch (Exception e) {error = e.Message;}
                Write(output, parent, error, admin);
                if (args[0] == "--once") break;
                for (int i = 0; i < 20 && Alive(parent) && !File.Exists(output + ".stop"); i++) Thread.Sleep(100);
            } while (Alive(parent) && !File.Exists(output + ".stop"));
            return 0;
        }
        catch (Exception e)
        {
            failed = true;
            try {Nodes.Clear(); Disks.Clear(); Write(output, parent, e.Message, admin, true);} catch {}
            return 1;
        }
        finally {
            try {if (computer != null) computer.Close();} catch {}
            if (args[0] == "--watch" && !failed) {try {File.Delete(output);File.Delete(output+".stop");} catch {}}
        }
    }

    internal static void CollectService(WaitHandle stop)
    {
        // Retry a driver/sensor initialization failure without asking the user
        // for a new elevation. Never reuse old readings after a failed cycle.
        do {
            Computer computer = null;
            try {
                computer = new Computer { IsCpuEnabled = true, IsGpuEnabled = true, IsStorageEnabled = true, IsMotherboardEnabled = true };
                computer.Open();
                do {
                    Nodes.Clear(); Disks.Clear();
                    foreach (IHardware hardware in computer.Hardware) Read(hardware);
                    Write(SensorService.Output, 0, "", true);
                } while (!stop.WaitOne(2000));
                return;
            } catch (Exception error) {
                Nodes.Clear(); Disks.Clear();
                try { Write(SensorService.Output, 0, error.Message, true, true); } catch {}
            } finally { try { if (computer != null) computer.Close(); } catch {} }
        } while (!stop.WaitOne(10000));
    }
}
