using System.Diagnostics;
using System.Runtime.InteropServices;

namespace Hypermesh.Visor;

internal sealed class InputMonitor : IDisposable
{
    private readonly Thread thread;
    private readonly ManualResetEventSlim ready = new();
    private readonly Action input;
    private readonly Action lost;
    private readonly nuint marker;
    private long heartbeat;
    private int failed;
    private long foreignExtra;
    private int foreignInjected;
    internal (bool Injected, nuint Extra) LastForeignInput => (Volatile.Read(ref foreignInjected) != 0, unchecked((nuint)Interlocked.Read(ref foreignExtra)));
    private ApplicationContext? context;
    private WindowsFormsSynchronizationContext? synchronization;
    private Win32.Hook? keyboardCallback, mouseCallback;
    internal bool Healthy => Volatile.Read(ref failed) == 0 && thread.IsAlive && Stopwatch.GetElapsedTime(Interlocked.Read(ref heartbeat)).TotalMilliseconds < 250;

    internal InputMonitor(nuint marker, Action input, Action lost)
    {
        this.marker = marker; this.input = input; this.lost = lost;
        thread = new Thread(Run) { IsBackground = true, Name = "Visor input monitor" };
        thread.SetApartmentState(ApartmentState.STA);
        thread.Start();
        if (!ready.Wait(TimeSpan.FromSeconds(5)) || !Healthy) Fail();
    }
    internal static bool IsOwn(bool injected, nuint extra, nuint marker) => injected && extra == marker;
    private void ForeignInput(bool injected, nuint extra)
    {
        Interlocked.Exchange(ref foreignExtra, unchecked((long)extra));
        Volatile.Write(ref foreignInjected, injected ? 1 : 0);
        input();
    }
    private void Fail()
    {
        if (Interlocked.Exchange(ref failed, 1) == 0) ThreadPool.QueueUserWorkItem(_ => lost());
    }
    private void CheckPump()
    {
        long now = Stopwatch.GetTimestamp();
        long previous = Interlocked.Exchange(ref heartbeat, now);
        if (previous != 0 && Stopwatch.GetElapsedTime(previous, now).TotalMilliseconds >= 250) Fail();
    }
    private void Run()
    {
        nint keyboard = 0, mouse = 0;
        try
        {
            synchronization = new WindowsFormsSynchronizationContext();
            SynchronizationContext.SetSynchronizationContext(synchronization);
            context = new ApplicationContext();
            keyboardCallback = (code, message, data) =>
            {
                if (code >= 0)
                {
                    CheckPump();
                    Win32.KeyHook value = Marshal.PtrToStructure<Win32.KeyHook>(data);
                    if (!IsOwn((value.Flags & 0x10) != 0, value.Extra, marker)) ForeignInput((value.Flags & 0x10) != 0, value.Extra);
                }
                return Win32.CallNextHookEx(0, code, message, data);
            };
            mouseCallback = (code, message, data) =>
            {
                if (code >= 0)
                {
                    CheckPump();
                    Win32.MouseHook value = Marshal.PtrToStructure<Win32.MouseHook>(data);
                    if (!IsOwn((value.Flags & 1) != 0, value.Extra, marker)) ForeignInput((value.Flags & 1) != 0, value.Extra);
                }
                return Win32.CallNextHookEx(0, code, message, data);
            };
            nint module = Win32.GetModuleHandleW(null);
            keyboard = Win32.SetWindowsHookExW(13, keyboardCallback, module, 0);
            mouse = Win32.SetWindowsHookExW(14, mouseCallback, module, 0);
            if (keyboard == 0 || mouse == 0) { Fail(); return; }
            CheckPump();
            using System.Windows.Forms.Timer timer = new() { Interval = 25 };
            timer.Tick += (_, _) => CheckPump();
            timer.Start();
            ready.Set();
            Application.Run(context);
        }
        catch { Fail(); }
        finally
        {
            if (keyboard != 0) Win32.UnhookWindowsHookEx(keyboard);
            if (mouse != 0) Win32.UnhookWindowsHookEx(mouse);
            ready.Set();
        }
    }
    public void Dispose()
    {
        Interlocked.Exchange(ref failed, 1);
        synchronization?.Post(_ => context?.ExitThread(), null);
        if (thread.Join(TimeSpan.FromSeconds(2))) ready.Dispose();
    }
}
