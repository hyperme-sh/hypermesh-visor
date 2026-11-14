using System.Diagnostics;
using System.Text.Json;
using Microsoft.Win32;

namespace Hypermesh.Visor;

internal sealed class Companion : Form
{
    private readonly Consent consent = new();
    private readonly DesktopDriver desktop;
    private InputMonitor? monitor;
    private readonly Label status = new() { AutoSize = false, Dock = DockStyle.Fill, Padding = new Padding(12), Text = "Starting Visor…" };
    private readonly Button allow = new() { Text = "Allow desktop access", AutoSize = true, Enabled = false };
    private readonly NotifyIcon tray = new() { Icon = SystemIcons.Application, Text = "Hypermesh Visor", Visible = true };
    private readonly Icon companionIcon = LoadIcon();
    private Icon? statusIcon;
    private string iconKey = "";
    private CompactUsage? responseUsage;
    private readonly PointerOverlay overlay = new();
    private McpTrustWindow? trustWindow;
    private readonly System.Windows.Forms.Timer timer = new() { Interval = 250 };
    private readonly nuint marker = InputMonitor.CreateMarker();
    private readonly InputAuthority authority = new();
    private int inputPending;
    private long lastInput, approvalBaseline = -1;
    private bool polling, approving, resuming, closing, started;
    private string purpose = "";
    private string phase = "off", message = "Desktop access is off", usage = "No response usage yet";
    private long Now => Environment.TickCount64;

    internal Companion()
    {
        tray.Icon = companionIcon;
        Text = "Hypermesh Visor"; Width = 500; Height = 300; MinimumSize = new Size(420, 280);
        desktop = new DesktopDriver(marker, session => authority.AllowedFor(session) && monitor?.Healthy == true,
            () => authority.Generation, Fault);
        FlowLayoutPanel buttons = new() { Dock = DockStyle.Bottom, Height = 48, Padding = new Padding(8), AutoSize = true };
        buttons.Controls.Add(allow);
        Button pause = new() { Text = "Pause", AutoSize = true }, stop = new() { Text = "Stop", AutoSize = true };
        pause.Click += (_, _) => Pause("Paused by you"); stop.Click += (_, _) => Stop();
        allow.Click += async (_, _) => await Approve();
        buttons.Controls.Add(pause); buttons.Controls.Add(stop);
        Controls.Add(status); Controls.Add(buttons);
        ContextMenuStrip menu = new();
        menu.Items.Add("Open controls", null, (_, _) => { Show(); WindowState = FormWindowState.Normal; Activate(); });
        menu.Items.Add("Open session forum", null, async (_, _) => await OpenDestination(false));
        menu.Items.Add("Open workspace in VS Code", null, async (_, _) => await OpenDestination(true));
        menu.Items.Add("Session CLI in PowerShell", null, async (_, _) => await OpenTerminal(false));
        menu.Items.Add("Session CLI in WSL", null, async (_, _) => await OpenTerminal(true));
        menu.Items.Add("MCP discovery and trust", null, (_, _) =>
        {
            if (!started || closing) return;
            Pause("Paused for MCP trust review. Close the review before allowing desktop access again.");
            if (trustWindow == null || trustWindow.IsDisposed)
            {
                trustWindow = new McpTrustWindow();
                trustWindow.FormClosed += (_, _) => { if (!closing) UpdateStatus(); };
            }
            if (!trustWindow.Visible) trustWindow.Show(this);
            trustWindow.Activate();
            UpdateStatus();
        });
        menu.Items.Add("Allow desktop access", null, async (_, _) => await Approve());
        menu.Items.Add("Pause", null, (_, _) => Pause("Paused by you"));
        menu.Items.Add("Stop", null, (_, _) => Stop());
        menu.Items.Add("Quit Visor", null, (_, _) => Close());
        tray.ContextMenuStrip = menu; tray.DoubleClick += (_, _) => { Show(); WindowState = FormWindowState.Normal; Activate(); };
        Resize += (_, _) => { if (WindowState == FormWindowState.Minimized) Hide(); };
        Shown += (_, _) => Start();
        timer.Tick += async (_, _) => await Poll();
        SystemEvents.SessionSwitch += SessionChanged;
        SystemEvents.DisplaySettingsChanged += DisplayChanged;
        FormClosing += CloseCompanion;
    }
    private static Icon LoadIcon()
    {
        string path = Path.Combine(AppContext.BaseDirectory, "companion", "idle_0_128.png");
        if (!File.Exists(path)) return (Icon)SystemIcons.Application.Clone();
        using Bitmap source = new(path);
        using Bitmap bitmap = new(source, new Size(32, 32));
        nint handle = bitmap.GetHicon();
        try { return (Icon)Icon.FromHandle(handle).Clone(); }
        finally { Win32.DestroyIcon(handle); }
    }
    private async Task OpenDestination(bool editor)
    {
        if (!started || closing || consent.Selected is not { } selected) return;
        try
        {
            JsonElement? response = await Engine.Control("destinations", selected);
            if (closing) return;
            string destination = Destinations.Resolve(response, selected, consent.Selected, editor);
            Destinations.Open(destination, editor);
        }
        catch
        {
            if (!closing && consent.Selected == selected)
            {
                message = "Configure this session's destination in companion.toml. Install VS Code to open a workspace.";
                UpdateStatus();
            }
        }
    }
    private async Task OpenTerminal(bool wsl)
    {
        if (!started || closing || consent.Selected is not { } selected) return;
        try
        {
            ProcessStartInfo plan = await SessionTerminal.Plan(wsl, selected);
            if (!closing && consent.Selected == selected) Process.Start(plan);
        }
        catch
        {
            if (!closing && consent.Selected == selected)
            {
                message = "Could not open the session terminal. Check the bundled CLI and your PowerShell or WSL installation.";
                UpdateStatus();
            }
        }
    }
    private void Post(Action action)
    {
        if (closing || IsDisposed || !IsHandleCreated) return;
        try { BeginInvoke(action); } catch (InvalidOperationException) { }
    }
    private void Revoke()
    {
        authority.Revoke();
        overlay.Hide();
        _ = Task.Run(desktop.Release);
    }
    private void Fault()
    {
        authority.Revoke();
        Post(() => Pause("Desktop unavailable: elevated windows and secure desktops cannot be controlled"));
    }
    private void Start()
    {
        try
        {
            if (!Engine.Start(desktop.Perform)) { message = "Visor could not start. Run as your normal user, without elevation."; UpdateStatus(); return; }
            started = true;
            monitor = new InputMonitor(marker, PhysicalInput, () => Post(MonitorLost));
            if (!monitor.Healthy) MonitorLost();
            timer.Start(); UpdateStatus();
        }
        catch { message = "Native engine unavailable; reinstall the complete unsigned bundle."; UpdateStatus(); }
    }
    private void PhysicalInput()
    {
        // Hook callbacks publish cancellation without waiting for UI, IPC or locks.
        authority.Revoke();
        Interlocked.Exchange(ref lastInput, Now);
        if (Interlocked.Exchange(ref inputPending, 1) == 0) Post(async () =>
        {
            Interlocked.Exchange(ref inputPending, 0);
            if (authority.Generation <= approvalBaseline) return;
            var ticket = consent.Input(Interlocked.Read(ref lastInput));
            overlay.Hide(); _ = Task.Run(desktop.Release);
            if (ticket is { } pending)
            {
                message = "Paused for your input. Continue here, or wait two idle minutes.";
                JsonElement? response = await Engine.Control("idle-pause", pending.Session);
                if (Success(response) && Body(response).TryGetProperty("epoch", out JsonElement epoch))
                    consent.RememberIdle(pending.Session, pending.Generation, epoch.GetUInt64());
                UpdateStatus();
            }
        });
    }
    private void MonitorLost()
    {
        Revoke(); consent.LoseMonitor(); message = "Input monitor unavailable. Restart Visor to enable automation.";
        _ = Engine.Control("monitor-lost"); UpdateStatus();
    }
    private void Pause(string reason)
    {
        Revoke(); consent.Pause(); message = reason; _ = Engine.Control("pause"); UpdateStatus();
    }
    private void Stop()
    {
        Revoke(); consent.Stop(); message = "Stopped. Desktop access is off."; _ = Engine.Control("stop"); UpdateStatus();
    }
    private async Task Approve()
    {
        if (!started || approving || trustWindow is { IsDisposed: false } || consent.Selected is not { } session || monitor?.Healthy != true || !Win32.Interactive() || Win32.HeldInput()) return;
        approving = true;
        Revoke();
        long generation = consent.BeginApproval();
        long baseline = authority.Generation; approvalBaseline = baseline;
        try
        {
            await Task.Run(desktop.Release).WaitAsync(TimeSpan.FromSeconds(1));
            if (closing || baseline != authority.Generation || !consent.PendingApproval(session, generation)) return;
            if (!desktop.Released || !desktop.ApproveLayout()) { Pause("Input cleanup is unavailable; restart after returning to your desktop"); return; }
            JsonElement? response = await Engine.Control(phase == "paused" ? "resume" : "start", session);
            if (!closing && Success(response) && desktop.LayoutMatches && baseline == authority.Generation && monitor.Healthy && Win32.Interactive() && !Win32.HeldInput() && consent.Approve(session, generation))
            {
                authority.Grant(baseline, session); message = "Desktop access allowed for this session";
            }
            else Pause("Approval changed or is unavailable. Select the current session and try again.");
        }
        catch { Pause("Desktop access unavailable. Return to your normal desktop and try again."); }
        finally { approving = false; UpdateStatus(); }
    }
    private async Task Poll()
    {
        if (polling || closing || !started) return;
        polling = true;
        try
        {
            if (monitor?.Healthy != true && !consent.MonitorLost) MonitorLost();
            if (!Win32.Interactive() && consent.Allowed) Pause("Desktop session inactive; allow access again after returning");
            JsonElement? response = await Engine.Control("poll");
            if (!Success(response)) { Pause("Engine unavailable. Restart Visor."); return; }
            JsonElement body = Body(response);
            string? session = null;
            if (body.TryGetProperty("session", out JsonElement selected) && selected.ValueKind == JsonValueKind.Object && selected.TryGetProperty("id", out JsonElement id) && Guid.TryParseExact(id.GetString(), "D", out Guid parsed)) session = parsed.ToString();
            if (session != consent.Selected) { Revoke(); consent.Select(session); message = session == null ? "Open a session to begin" : "New session. Allow desktop access to begin."; }
            purpose = selected.ValueKind == JsonValueKind.Object && selected.TryGetProperty("purpose", out JsonElement task) ? task.GetString() ?? "" : "";
            if (purpose.Length > 160) purpose = purpose[..160] + "…";
            phase = body.GetProperty("automation").GetProperty("phase").GetString() ?? "off";
            if (consent.Allowed && phase != "running") Pause("Automation paused; allow access to continue");
            responseUsage = CompactUsage.Read(selected);
            usage = responseUsage?.Summary ?? "No response usage yet";
            if (consent.Allowed && phase == "running" && Win32.GetCursorPos(out Win32.Point point)) overlay.Follow(point.X, point.Y);
            else overlay.Hide();
            if (!resuming && consent.CanResume(Now, Win32.HeldInput()) && Win32.Interactive() && desktop.LayoutMatches && monitor?.Healthy == true && consent.Idle is { } ticket)
            {
                resuming = true;
                long baseline = authority.Generation;
                JsonElement? resumed = await Engine.Control("idle-resume", ticket.Session, ticket.Epoch);
                if (!closing && Success(resumed) && desktop.LayoutMatches && baseline == authority.Generation && monitor.Healthy && Win32.Interactive() && consent.Resume(ticket.Session, ticket.Generation, Now, Win32.HeldInput()))
                { approvalBaseline = baseline; authority.Grant(baseline, ticket.Session); message = "Resumed after two idle minutes"; }
                else Pause("Automatic resume was interrupted. Allow desktop access to continue.");
                resuming = false;
            }
            UpdateStatus();
        }
        catch { Pause("Engine response unavailable. Restart Visor."); }
        finally { polling = false; resuming = false; }
    }
    private static bool Success(JsonElement? response) => response is { } value && value.TryGetProperty("status", out JsonElement status) && status.GetInt32() is >= 200 and < 300;
    private static JsonElement Body(JsonElement? response) => response!.Value.GetProperty("body");
    private void UpdateStatus()
    {
        status.Text = $"{message}\nTask: {purpose}\nScreen capture and mouse/keyboard input require your approval.\n\nSession: {consent.Selected ?? "none"}\n{usage}";
        allow.Enabled = started && !approving && trustWindow is not { IsDisposed: false } && !consent.MonitorLost && consent.Selected != null;
        tray.Text = $"Visor: {phase}\n{usage}"[..Math.Min(63, $"Visor: {phase}\n{usage}".Length)];
        string nextIcon = $"{phase}:{responseUsage}";
        if (nextIcon != iconKey)
        {
            Icon next = CompactUsage.Draw(companionIcon, phase, responseUsage);
            Icon? previous = statusIcon;
            tray.Icon = next; statusIcon = next; iconKey = nextIcon;
            previous?.Dispose();
        }
    }
    private void SessionChanged(object? sender, SessionSwitchEventArgs args) { authority.Revoke(); Post(() => Pause("Desktop session changed; allow access again")); }
    private void DisplayChanged(object? sender, EventArgs args) { authority.Revoke(); Post(() => Pause("Display layout changed; allow access again")); }
    private async void CloseCompanion(object? sender, FormClosingEventArgs args)
    {
        if (closing) return;
        args.Cancel = true; closing = true;
        Revoke(); consent.Stop(); timer.Stop(); tray.Visible = false;
        trustWindow?.Close();
        monitor?.Dispose();
        try { await Task.Run(desktop.Release).WaitAsync(TimeSpan.FromSeconds(1)); } catch { }
        SystemEvents.SessionSwitch -= SessionChanged;
        SystemEvents.DisplaySettingsChanged -= DisplayChanged;
        if (started) await Engine.Control("stop");
        overlay.Dispose(); tray.Dispose(); statusIcon?.Dispose(); companionIcon.Dispose(); timer.Dispose();
        Close();
    }
}

internal sealed class PointerOverlay : Form
{
    internal PointerOverlay()
    {
        FormBorderStyle = FormBorderStyle.None; ShowInTaskbar = false; TopMost = true;
        BackColor = Color.Magenta; TransparencyKey = Color.Magenta; Size = new Size(32, 32);
        string path = Path.Combine(AppContext.BaseDirectory, "companion", "automation_0_128.png");
        if (File.Exists(path)) BackgroundImage = Image.FromFile(path);
        BackgroundImageLayout = ImageLayout.Zoom;
    }
    protected override bool ShowWithoutActivation => true;
    protected override CreateParams CreateParams { get { CreateParams value = base.CreateParams; value.ExStyle |= 0x80000 | 0x20 | 0x80 | 0x08000000; return value; } }
    internal void Follow(int x, int y) { Location = new Point(x + 8, y + 8); if (!Visible) Show(); }
    protected override void Dispose(bool disposing) { if (disposing) BackgroundImage?.Dispose(); base.Dispose(disposing); }
}
