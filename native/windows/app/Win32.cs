using System.Runtime.InteropServices;
using System.Security.Principal;

namespace Hypermesh.Visor;

internal static class Win32
{
    [StructLayout(LayoutKind.Sequential)] internal struct Point { internal int X, Y; }
    [StructLayout(LayoutKind.Sequential)] internal struct MouseInput { internal int X, Y; internal uint Data, Flags, Time; internal nuint Extra; }
    [StructLayout(LayoutKind.Sequential)] internal struct KeyInput { internal ushort Key, Scan; internal uint Flags, Time; internal nuint Extra; }
    [StructLayout(LayoutKind.Explicit)] internal struct InputData { [FieldOffset(0)] internal MouseInput Mouse; [FieldOffset(0)] internal KeyInput Key; }
    [StructLayout(LayoutKind.Sequential)] internal struct Input { internal uint Type; internal InputData Data; }
    [StructLayout(LayoutKind.Sequential)] internal struct MouseHook { internal Point Point; internal uint Data, Flags, Time; internal nuint Extra; }
    [StructLayout(LayoutKind.Sequential)] internal struct KeyHook { internal uint Key, Scan, Flags, Time; internal nuint Extra; }
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern bool DestroyIcon(nint icon);
    internal delegate nint Hook(int code, nint message, nint data);
    [DllImport("user32.dll", SetLastError = true), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern uint SendInput(uint count, Input[] inputs, int size);
    [DllImport("user32.dll", SetLastError = true), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern nint SetWindowsHookExW(int type, Hook hook, nint module, uint thread);
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern bool UnhookWindowsHookEx(nint hook);
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern nint CallNextHookEx(nint hook, int code, nint message, nint data);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern nint GetModuleHandleW(string? module);
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern nint GetForegroundWindow();
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern nint WindowFromPoint(Point point);
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern uint GetWindowThreadProcessId(nint window, out uint process);
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    internal static extern short GetAsyncKeyState(int key);
    [DllImport("kernel32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern nint OpenProcess(uint access, bool inherit, uint process);
    [DllImport("advapi32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern bool OpenProcessToken(nint process, uint access, out nint token);
    [DllImport("advapi32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern bool GetTokenInformation(nint token, int type, out uint value, uint size, out uint returned);
    [DllImport("kernel32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern bool CloseHandle(nint handle);
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern nint OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll", CharSet = CharSet.Unicode), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern bool GetUserObjectInformationW(nint handle, int index, [Out] char[] value, uint bytes, out uint needed);
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern bool CloseDesktop(nint desktop);

    internal static bool HeldInput()
    {
        for (int key = 1; key < 255; key++) if ((GetAsyncKeyState(key) & 0x8000) != 0) return true;
        return false;
    }
    internal static bool Interactive()
    {
        nint desktop = OpenInputDesktop(0, false, 1);
        if (desktop == 0) return false;
        try
        {
            char[] name = new char[256];
            return GetUserObjectInformationW(desktop, 2, name, 512, out _) && new string(name).TrimEnd('\0') == "Default";
        }
        finally { CloseDesktop(desktop); }
    }
    internal static bool NormalUserWindow(nint window)
    {
        if (window == 0 || GetWindowThreadProcessId(window, out uint pid) == 0 || pid == 0) return false;
        nint process = OpenProcess(0x1000, false, pid);
        if (process == 0) return false;
        try
        {
            if (!OpenProcessToken(process, 8, out nint token)) return false;
            try
            {
                using WindowsIdentity target = new(token);
                using WindowsIdentity current = WindowsIdentity.GetCurrent();
                return target.User != null && current.User != null && target.User.Equals(current.User) && GetTokenInformation(token, 20, out uint elevated, 4, out uint size) && size == 4 && elevated == 0;
            }
            finally { CloseHandle(token); }
        }
        finally { CloseHandle(process); }
    }
}
