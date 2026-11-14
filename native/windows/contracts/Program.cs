using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text.Json;

namespace Hypermesh.Visor;

internal static class DesktopContracts
{
    [DllImport("user32.dll"), DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern bool SetCursorPos(int x, int y);
    private static int result = 1;
    private const string Session = "12345678-1234-1234-1234-123456789abc";
    private static void Require(bool condition, string message) { if (!condition) throw new Exception(message); }
    private static JsonElement Request(object value) => JsonSerializer.SerializeToElement(value);
    private static async Task Until(Func<bool> condition, string message)
    {
        for (int i = 0; i < 100; i++) { if (condition()) return; await Task.Delay(20); }
        throw new Exception(message);
    }

    [STAThread]
    private static int Main(string[] args)
    {
        if (args.Length == 0)
        {
            try { return FixtureProcess.Run(); }
            catch (Exception error) { Console.Error.WriteLine(error.Message); return 1; }
        }
        if (args.Length != 1 || args[0] != "--fixture" || Environment.GetEnvironmentVariable("CI") != "true") return 1;
        using StreamWriter log = new(Path.Combine(AppContext.BaseDirectory, "fixture.log")) { AutoFlush = true };
        Console.SetOut(log); Console.SetError(log);
        Application.SetHighDpiMode(HighDpiMode.PerMonitorV2);
        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        using Form fixture = new() { Text = "Visor CI desktop fixture", Width = 420, Height = 280, StartPosition = FormStartPosition.Manual, Location = new Point(60, 60), TopMost = true, BackColor = Color.FromArgb(37, 165, 104) };
        TextBox entry = new() { Location = new Point(20, 20), Width = 340 };
        fixture.Controls.Add(entry);
        fixture.Shown += async (_, _) =>
        {
            try { await Run(fixture, entry); result = 0; }
            catch (Exception error) { Console.Error.WriteLine(error); }
            finally { fixture.Close(); }
        };
        Application.Run(fixture);
        return result;
    }

    private static async Task Run(Form fixture, TextBox entry)
    {
        Require(Win32.Interactive(), "Windows contract requires an interactive Default desktop");
        Require(Win32.NormalUserWindow(fixture.Handle), "Windows contract requires a normal-user process without elevation");
        await Task.Delay(500);
        InputAuthority authority = new();
        nuint marker = (nuint)(BitConverter.ToUInt64(RandomNumberGenerator.GetBytes(8)) | 1UL);
        int interruptions = 0, failures = 0;
        using InputMonitor monitor = new(marker, () => { authority.Revoke(); Interlocked.Increment(ref interruptions); }, () => { authority.Revoke(); Interlocked.Increment(ref failures); });
        DesktopDriver driver = new(marker, session => authority.AllowedFor(session) && monitor.Healthy, () => authority.Generation, () => Interlocked.Increment(ref failures));
        Win32.GetCursorPos(out Win32.Point original);
        try
        {
            Require(monitor.Healthy && driver.ApproveLayout(), "Desktop monitor/layout unavailable");
            authority.Grant(authority.Generation, Session);
            Console.WriteLine("STEP capture");
            JsonElement capture = Request(driver.Perform(Request(new { action = "view", session = Session })));
            using MemoryStream bytes = new(Convert.FromBase64String(capture.GetProperty("png").GetString()!));
            using Bitmap image = new(bytes);
            Rectangle bounds = SystemInformation.VirtualScreen;
            Point sample = fixture.PointToScreen(new Point(150, 120));
            Require(image.Size == bounds.Size, "Capture dimensions differ from the virtual desktop");
            Require(image.GetPixel(sample.X - bounds.X, sample.Y - bounds.Y).ToArgb() == fixture.BackColor.ToArgb(), "Capture did not contain the fixture window");
            bool refused = false;
            Console.WriteLine("STEP cross-session refusal");
            try { driver.Perform(Request(new { action = "view", session = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa" })); }
            catch (OperationCanceledException) { refused = true; }
            Require(refused, "Another session reused desktop approval");

            Point target = entry.PointToScreen(new Point(25, 10));
            Console.WriteLine("STEP pointer move");
            driver.Perform(Request(new { action = "pointer", session = Session, steps = new[] { new { kind = "move", x = target.X - bounds.X, y = target.Y - bounds.Y } } }));
            await Task.Delay(100);
            Console.WriteLine("STEP click");
            driver.Perform(Request(new { action = "pointer", session = Session, steps = new[] { new { kind = "button", button = 0, down = true }, new { kind = "button", button = 0, down = false } } }));
            await Until(() => Win32.GetForegroundWindow() == fixture.Handle && entry.Focused, "Fixture did not receive focus");
            Console.WriteLine("STEP Unicode input");
            driver.Perform(Request(new { action = "keyboard", session = Session, strokes = new[] { new { keysym = 97u, down = true }, new { keysym = 97u, down = false } } }));
            await Until(() => entry.Text == "a", "Unicode input did not reach the fixture");
            Require(authority.AllowedFor(Session) && Volatile.Read(ref interruptions) == 0, "Own marked input paused automation");

            Console.WriteLine("STEP held key");
            driver.Perform(Request(new { action = "keyboard", session = Session, strokes = new[] { new { keysym = 0xFFE3u, down = true } } }));
            await Until(() => (Win32.GetAsyncKeyState(0x11) & 0x8000) != 0, "Held-key fixture did not press Control");
            Win32.Input other = new() { Type = 0 };
            Console.WriteLine("STEP foreign input and release");
            other.Data.Mouse.X = 1; other.Data.Mouse.Flags = 1; other.Data.Mouse.Extra = marker ^ 2;
            Require(Win32.SendInput(1, [other], Marshal.SizeOf<Win32.Input>()) == 1, "Other-input fixture could not send its event");
            await Until(() => Volatile.Read(ref interruptions) > 0 && !authority.AllowedFor(Session), "Input hook did not revoke desktop access");
            driver.Release();
            await Until(() => (Win32.GetAsyncKeyState(0x11) & 0x8000) == 0, "Pause left a synthetic key held");
            refused = false;
            try { driver.Perform(Request(new { action = "view", session = Session })); }
            catch (OperationCanceledException) { refused = true; }
            Require(refused && Volatile.Read(ref failures) == 0, "Pause contract failed or monitor was lost");
            Console.WriteLine("PASS native capture, session binding, focus, Unicode input, hook provenance, pause, and key release");
        }
        finally
        {
            Console.WriteLine($"STATE allowed={authority.AllowedFor(Session)} monitor={monitor.Healthy} interruptions={Volatile.Read(ref interruptions)} failures={Volatile.Read(ref failures)}");
            Console.WriteLine($"INPUT injected={monitor.LastForeignInput.Injected} extra={monitor.LastForeignInput.Extra:x} marker={marker:x}");
            authority.Revoke(); driver.Release();
            // Restore only this fixture's pointer movement; no desktop image is saved.
            SetCursorPos(original.X, original.Y);
        }
    }
}
