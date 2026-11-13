using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace Hypermesh.Visor;

internal sealed class DesktopDriver
{
    private readonly object gate = new();
    private readonly Dictionary<string, Win32.Input> held = [];
    private readonly Func<bool> allowed;
    private readonly Func<long> generation;
    private readonly Action fault;
    private readonly nuint marker;
    private Rectangle? approvedLayout;
    internal bool ApproveLayout() { lock (gate) { approvedLayout = Bounds(); return true; } }
    internal bool LayoutMatches { get { if (!Monitor.TryEnter(gate)) return false; try { return approvedLayout == Bounds(); } catch { return false; } finally { Monitor.Exit(gate); } } }
    internal DesktopDriver(nuint marker, Func<bool> allowed, Func<long> generation, Action fault)
    { this.marker = marker; this.allowed = allowed; this.generation = generation; this.fault = fault; }

    internal object Perform(JsonElement request)
    {
        string action = request.GetProperty("action").GetString() ?? "";
        lock (gate)
        {
            if (action == "release") { Release(); return new { }; }
            if (action == "info")
            {
                Rectangle bounds = Bounds();
                if (!Win32.GetCursorPos(out Win32.Point point)) throw new InvalidOperationException();
                Win32.GetWindowThreadProcessId(Win32.GetForegroundWindow(), out uint pid);
                return new { width = bounds.Width, height = bounds.Height, x = point.X - bounds.X, y = point.Y - bounds.Y, pid };
            }
            long epoch = generation();
            try
            {
                Check(epoch);
                if (action == "view")
                {
                    Rectangle bounds = Bounds();
                    using Bitmap image = new(bounds.Width, bounds.Height, PixelFormat.Format32bppArgb);
                    using (Graphics graphics = Graphics.FromImage(image)) graphics.CopyFromScreen(bounds.Location, Point.Empty, bounds.Size, CopyPixelOperation.SourceCopy);
                    using MemoryStream bytes = new();
                    image.Save(bytes, ImageFormat.Png);
                    if (bytes.Length > 32 * 1024 * 1024) throw new InvalidOperationException();
                    Check(epoch);
                    return new { png = Convert.ToBase64String(bytes.ToArray()) };
                }
                if (action == "pointer")
                {
                    JsonElement steps = request.GetProperty("steps");
                    if (steps.GetArrayLength() > 1024) throw new InvalidOperationException();
                    foreach (JsonElement step in steps.EnumerateArray())
                    {
                        Check(epoch);
                        string kind = step.GetProperty("kind").GetString() ?? "";
                        Win32.Input input = new() { Type = 0 };
                        input.Data.Mouse.Extra = marker;
                        if (kind == "move")
                        {
                            Rectangle bounds = Bounds();
                            int x = step.GetProperty("x").GetInt32(), y = step.GetProperty("y").GetInt32();
                            (input.Data.Mouse.X, input.Data.Mouse.Y) = Normalize(x, y, bounds.Width, bounds.Height);
                            if (!Win32.NormalUserWindow(Win32.WindowFromPoint(new Win32.Point { X = bounds.X + x, Y = bounds.Y + y }))) throw new InvalidOperationException();
                            input.Data.Mouse.Flags = 0x8000 | 0x4000 | 1;
                        }
                        else
                        {
                            if (!Win32.GetCursorPos(out Win32.Point point) || !Win32.NormalUserWindow(Win32.WindowFromPoint(point))) throw new InvalidOperationException();
                            if (kind == "button")
                            {
                                uint flag = step.GetProperty("button").GetInt32() switch { 0 => 2u, 1 => 8u, 2 => 32u, _ => throw new InvalidOperationException() };
                                input.Data.Mouse.Flags = step.GetProperty("down").GetBoolean() ? flag : flag * 2;
                            }
                            else if (kind == "scroll")
                            {
                                string direction = step.GetProperty("direction").GetString() ?? "";
                                input.Data.Mouse.Flags = direction is "left" or "right" ? 0x1000u : 0x800u;
                                input.Data.Mouse.Data = direction switch { "up" or "right" => 120u, "down" or "left" => unchecked((uint)-120), _ => throw new InvalidOperationException() };
                            }
                            else throw new InvalidOperationException();
                        }
                        Send(input, epoch);
                    }
                    return new { };
                }
                if (action == "keyboard")
                {
                    JsonElement strokes = request.GetProperty("strokes");
                    if (strokes.GetArrayLength() > 4096) throw new InvalidOperationException();
                    foreach (JsonElement stroke in strokes.EnumerateArray())
                    {
                        Check(epoch);
                        if (!Win32.NormalUserWindow(Win32.GetForegroundWindow())) throw new InvalidOperationException();
                        bool shortcut = held.Values.Any(IsChordModifier);
                        foreach (Win32.Input input in Keys(stroke.GetProperty("keysym").GetUInt32(), stroke.GetProperty("down").GetBoolean(), shortcut, marker)) Send(input, epoch);
                    }
                    return new { };
                }
                throw new InvalidOperationException();
            }
            catch (OperationCanceledException) { Release(); throw; }
            catch { Release(); fault(); throw; }
        }
    }
    internal static bool IsChordModifier(Win32.Input input) => input.Type == 1 && input.Data.Key.Key is 0xA0 or 0xA1 or 0xA2 or 0xA3 or 0xA4 or 0xA5 or 0x5B or 0x5C;
    internal static (int X, int Y) Normalize(int x, int y, int width, int height)
    {
        if (width < 1 || height < 1 || x < 0 || y < 0 || x >= width || y >= height) throw new ArgumentOutOfRangeException();
        return (width == 1 ? 0 : (int)((long)x * 65535 / (width - 1)), height == 1 ? 0 : (int)((long)y * 65535 / (height - 1)));
    }
    private static Rectangle Bounds()
    {
        if (!Win32.Interactive()) throw new InvalidOperationException();
        Rectangle bounds = SystemInformation.VirtualScreen;
        if (bounds.Width < 1 || bounds.Height < 1 || bounds.Width > 65535 || bounds.Height > 65535 || (long)bounds.Width * bounds.Height > 32_000_000) throw new InvalidOperationException();
        return bounds;
    }
    private void Check(long epoch)
    {
        if (!allowed() || epoch != generation()) throw new OperationCanceledException();
        if (!Win32.Interactive() || approvedLayout != Bounds()) throw new InvalidOperationException();
    }
    private void Send(Win32.Input input, long epoch)
    {
        Check(epoch);
        if (Win32.SendInput(1, [input], Marshal.SizeOf<Win32.Input>()) != 1) throw new InvalidOperationException();
        (string? key, Win32.Input release, bool down) = Held(input);
        if (key != null) { if (down) held[key] = release; else held.Remove(key); }
    }
    internal void Release()
    {
        lock (gate)
        {
            foreach ((string key, Win32.Input input) in held.Reverse().ToArray())
                if (Win32.SendInput(1, [input], Marshal.SizeOf<Win32.Input>()) == 1) held.Remove(key);
        }
    }
    internal bool Released { get { if (!Monitor.TryEnter(gate)) return false; try { return held.Count == 0; } finally { Monitor.Exit(gate); } } }
    internal static (string? Key, Win32.Input Release, bool Down) Held(Win32.Input input)
    {
        if (input.Type == 1)
        {
            string key = $"k-{input.Data.Key.Key}-{input.Data.Key.Scan}-{input.Data.Key.Flags & 5}";
            bool down = (input.Data.Key.Flags & 2) == 0;
            input.Data.Key.Flags |= 2;
            return (key, input, down);
        }
        uint flags = input.Data.Mouse.Flags;
        uint button = flags switch { 2 or 4 => 2u, 8 or 16 => 8u, 32 or 64 => 32u, _ => 0u };
        if (button == 0) return (null, input, false);
        input.Data.Mouse.Flags = button * 2;
        return ($"m-{button}", input, flags == button);
    }
    internal static Win32.Input[] Keys(uint symbol, bool down, bool shortcut, nuint marker)
    {
        ushort key = symbol switch
        {
            0xFF08 => 8, 0xFF09 => 9, 0xFF0D => 13, 0xFF1B => 27, 0xFFFF => 46,
            0xFF50 => 36, 0xFF51 => 37, 0xFF52 => 38, 0xFF53 => 39, 0xFF54 => 40,
            0xFF55 => 33, 0xFF56 => 34, 0xFF57 => 35, 0xFF63 => 45,
            0xFFE1 => 0xA0, 0xFFE2 => 0xA1, 0xFFE3 => 0xA2, 0xFFE4 => 0xA3,
            0xFFE9 => 0xA4, 0xFFEA => 0xA5, 0xFFEB => 0x5B, 0xFFEC => 0x5C,
            >= 0xFFBE and <= 0xFFD5 => (ushort)(0x70 + symbol - 0xFFBE),
            _ => 0
        };
        if (key == 0 && shortcut)
        {
            if (symbol is >= 0x61 and <= 0x7A) key = (ushort)(symbol - 32);
            else if (symbol is >= 0x41 and <= 0x5A or >= 0x30 and <= 0x39) key = (ushort)symbol;
            else throw new InvalidOperationException("Unsupported shortcut key");
        }
        uint flags = down ? 0u : 2u;
        if (key != 0)
        {
            if (key is 33 or 34 or 35 or 36 or 37 or 38 or 39 or 40 or 45 or 46 or 0xA3 or 0xA5 or 0x5B or 0x5C) flags |= 1;
            return [new Win32.Input { Type = 1, Data = new Win32.InputData { Key = new Win32.KeyInput { Key = key, Flags = flags, Extra = marker } } }];
        }
        uint scalar = symbol >= 0x01000000 ? symbol & 0x00FFFFFF : symbol;
        if (!Rune.IsValid((int)scalar) || scalar < 32 || (symbol > 255 && symbol < 0x01000000)) throw new InvalidOperationException();
        return new Rune((int)scalar).ToString().Select(unit => new Win32.Input { Type = 1, Data = new Win32.InputData { Key = new Win32.KeyInput { Scan = unit, Flags = flags | 4, Extra = marker } } }).ToArray();
    }
}
