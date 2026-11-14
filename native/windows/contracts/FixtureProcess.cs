using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;

namespace Hypermesh.Visor;

// CI-only bootstrap. The production app and its elevation checks are unchanged.
internal static class FixtureProcess
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct Startup
    {
        internal uint Size;
        internal string? Reserved, Desktop, Title;
        internal uint X, Y, Width, Height, XChars, YChars, Fill, Flags;
        internal ushort Show, ReservedSize;
        internal nint ReservedData, Input, Output, Error;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessInfo { internal nint Process, Thread; internal uint Id, ThreadId; }
    [StructLayout(LayoutKind.Sequential)]
    private struct Label { internal nint Sid; internal uint Attributes; }
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool CreateRestrictedToken(nint source, uint flags, uint disabledCount, nint disabled, uint privilegeCount, nint privileges, uint restrictedCount, nint restricted, out nint token);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool GetTokenInformation(nint token, int type, out uint value, uint size, out uint returned);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool SetTokenInformation(nint token, int type, ref Label label, uint size);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateProcessAsUser(nint token, string application, StringBuilder command, nint processSecurity, nint threadSecurity, bool inherit, uint flags, nint environment, string directory, ref Startup startup, out ProcessInfo process);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint WaitForSingleObject(nint handle, uint timeout);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetExitCodeProcess(nint handle, out uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool TerminateProcess(nint handle, uint code);
    [DllImport("kernel32.dll")]
    private static extern bool CloseHandle(nint handle);
    private static void Check(bool success) { if (!success) throw new Win32Exception(Marshal.GetLastWin32Error()); }

    internal static int Run()
    {
        if (Environment.GetEnvironmentVariable("CI") != "true" || string.IsNullOrEmpty(Environment.GetEnvironmentVariable("RUNNER_TEMP")))
            throw new InvalidOperationException("Desktop contracts require CI");
        using WindowsIdentity identity = WindowsIdentity.GetCurrent();
        // LUA_TOKEN filters administrator groups; DISABLE_MAX_PRIVILEGE removes privileges.
        Check(CreateRestrictedToken(identity.Token, 5, 0, 0, 0, 0, 0, 0, out nint token));
        ProcessInfo child = default;
        nint sidMemory = 0;
        try
        {
            SecurityIdentifier medium = new("S-1-16-8192");
            byte[] sid = new byte[medium.BinaryLength]; medium.GetBinaryForm(sid, 0);
            sidMemory = Marshal.AllocHGlobal(sid.Length); Marshal.Copy(sid, 0, sidMemory, sid.Length);
            Label label = new() { Sid = sidMemory, Attributes = 0x20 };
            Check(SetTokenInformation(token, 25, ref label, (uint)(Marshal.SizeOf<Label>() + sid.Length)));
            Check(GetTokenInformation(token, 20, out uint elevated, 4, out uint size));
            if (size != 4 || elevated != 0) throw new InvalidOperationException("CI could not create an unelevated fixture token");
            string executable = Environment.ProcessPath!;
            Startup startup = new() { Size = (uint)Marshal.SizeOf<Startup>(), Desktop = @"winsta0\default" };
            Check(CreateProcessAsUser(token, executable, new StringBuilder('"' + executable + "\" --fixture"), 0, 0, false, 0, 0, AppContext.BaseDirectory, ref startup, out child));
            CloseHandle(child.Thread); child.Thread = 0;
            if (WaitForSingleObject(child.Process, 25000) != 0) throw new TimeoutException("Unelevated desktop fixture timed out");
            Check(GetExitCodeProcess(child.Process, out uint result));
            string log = Path.Combine(AppContext.BaseDirectory, "fixture.log");
            if (File.Exists(log)) Console.Write(File.ReadAllText(log));
            return checked((int)result);
        }
        finally
        {
            if (child.Process != 0) { if (WaitForSingleObject(child.Process, 0) != 0) { TerminateProcess(child.Process, 1); WaitForSingleObject(child.Process, 5000); } CloseHandle(child.Process); }
            if (child.Thread != 0) CloseHandle(child.Thread);
            if (sidMemory != 0) Marshal.FreeHGlobal(sidMemory);
            CloseHandle(token);
        }
    }
}
