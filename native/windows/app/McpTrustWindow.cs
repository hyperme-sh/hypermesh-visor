using System.Text;
using System.Text.Json;

namespace Hypermesh.Visor;

internal sealed class McpTrustWindow : Form
{
    private sealed record Profile(string Name) { public override string ToString() => Safe(Name); }
    private sealed record Source(string Label, string? Id = null, bool File = false) { public override string ToString() => Safe(Label); }
    private readonly ComboBox profiles = new() { DropDownStyle = ComboBoxStyle.DropDownList, Width = 220 };
    private readonly ComboBox sources = new() { DropDownStyle = ComboBoxStyle.DropDownList, Width = 420 };
    private readonly TextBox details = new() { Multiline = true, ReadOnly = true, WordWrap = false, ScrollBars = ScrollBars.Both, Dock = DockStyle.Fill };
    private readonly TextBox confirmation = new() { Width = 100, MaxLength = 8 };
    private readonly Button review = new() { Text = "Preview source", AutoSize = true };
    private readonly Button approve = new() { Text = "Approve snapshot", AutoSize = true };
    private readonly Button revoke = new() { Text = "Revoke profile", AutoSize = true };
    private readonly Button refresh = new() { Text = "Reload", AutoSize = true };
    private readonly Label message = new() { AutoSize = false, Dock = DockStyle.Top, Height = 75, Padding = new Padding(8) };
    private readonly Dictionary<string, string> grants = new(StringComparer.Ordinal);
    private string? pending;
    private bool busy, closing, configurationUnavailable;

    internal McpTrustWindow()
    {
        Text = "MCP discovery and trust"; Width = 900; Height = 650; MinimumSize = new Size(720, 500);
        FlowLayoutPanel selectors = new() { Dock = DockStyle.Top, Height = 75, Padding = new Padding(8), AutoSize = true };
        selectors.Controls.AddRange([new Label { Text = "Profile", AutoSize = true }, profiles, sources, review, refresh]);
        FlowLayoutPanel decisions = new() { Dock = DockStyle.Bottom, Height = 75, Padding = new Padding(8), AutoSize = true };
        decisions.Controls.AddRange([new Label { Text = "Type TRUST to approve or REVOKE to revoke:", AutoSize = true }, confirmation, approve, revoke]);
        Controls.Add(details); Controls.Add(message); Controls.Add(selectors); Controls.Add(decisions);
        profiles.SelectedIndexChanged += (_, _) => InvalidateReview();
        sources.SelectedIndexChanged += (_, _) => InvalidateReview();
        confirmation.TextChanged += (_, _) => UpdateButtons();
        review.Click += async (_, _) => await Run(Preview);
        approve.Click += async (_, _) => await Run(Approve);
        revoke.Click += async (_, _) => await Run(Revoke);
        refresh.Click += async (_, _) => await Run(Reload);
        Shown += async (_, _) => await Run(Reload);
        FormClosed += (_, _) => { closing = true; InvalidateReview(); };
        UpdateButtons();
    }

    internal static string Safe(string text) => string.Concat(text.Select(c => char.IsControl(c) || char.GetUnicodeCategory(c) == System.Globalization.UnicodeCategory.Format ? $"\\u{(int)c:x4}" : c.ToString()));
    private void UpdateButtons()
    {
        if (closing) return;
        profiles.Enabled = sources.Enabled = refresh.Enabled = confirmation.Enabled = !busy;
        review.Enabled = !busy && profiles.SelectedItem is Profile && sources.SelectedItem is Source;
        approve.Enabled = !busy && pending != null && confirmation.Text == "TRUST";
        revoke.Enabled = !busy && profiles.SelectedItem is Profile profile && grants.ContainsKey(profile.Name) && confirmation.Text == "REVOKE";
    }
    private void InvalidateReview()
    {
        string? previous = pending; pending = null;
        if (previous != null) _ = Engine.Control("mcp-decision", decision: new { action = "cancel", review_id = previous });
        if (closing) return;
        confirmation.Clear(); details.Clear();
        string state = profiles.SelectedItem is Profile profile && grants.TryGetValue(profile.Name, out string? status) ? Safe(status) : "not approved";
        message.Text = $"Current trust: {state}. Preview reads source metadata without starting servers.\nApproval applies to future sessions. Existing sessions retain their attachments; close them to stop those servers.";
        if (configurationUnavailable) message.Text += " Profile configuration or sources are unavailable; existing approvals can still be revoked.";
        UpdateButtons();
    }
    private async Task Run(Func<Task> operation)
    {
        if (busy || closing) return;
        busy = true; UpdateButtons();
        try { await operation(); }
        catch { if (!closing) { InvalidateReview(); message.Text = "MCP request failed or the source changed. Reload and review again; no request was retried."; } }
        finally { busy = false; UpdateButtons(); }
    }
    private static JsonElement Body(JsonElement? response)
    {
        if (response is not { } value || !value.TryGetProperty("status", out var status) || !status.TryGetInt32(out int code) || code is < 200 or >= 300)
            throw new InvalidOperationException("MCP request unavailable");
        return value.GetProperty("body");
    }
    internal static string ReviewId(JsonElement body, string profile)
    {
        if (body.ValueKind != JsonValueKind.Object || !body.TryGetProperty("profile", out var name) || name.GetString() != profile ||
            !body.TryGetProperty("review_id", out var id) || !Guid.TryParseExact(id.GetString(), "D", out var parsed) ||
            !body.TryGetProperty("servers", out var servers) || servers.ValueKind != JsonValueKind.Array)
            throw new InvalidOperationException("Invalid review snapshot");
        return parsed.ToString();
    }
    internal static string Describe(JsonElement body)
    {
        string Text(JsonElement item, string key) => item.TryGetProperty(key, out var field) && field.ValueKind == JsonValueKind.String ? Safe(field.GetString()!) : "none";
        string Names(JsonElement item, string key) => item.TryGetProperty(key, out var field) && field.ValueKind == JsonValueKind.Array ? string.Join(", ", field.EnumerateArray().Select(value => Safe(value.GetString() ?? ""))) : "none";
        StringBuilder text = new();
        text.AppendLine($"Profile: {Text(body, "profile")}\r\nProfile configuration: {Text(body, "profiles_path")}");
        foreach (JsonElement server in body.GetProperty("servers").EnumerateArray())
        {
            text.AppendLine($"\r\n{Text(server, "name")} — {Text(server, "status")} / {Text(server, "transport")}");
            text.AppendLine($"Enabled: {server.GetProperty("enabled")} / Argument count: {server.GetProperty("argument_count")}");
            text.AppendLine($"Source: {Text(server, "source")}\r\nFile: {Text(server, "path")}");
            text.AppendLine($"Executable: {Text(server, "command")}\r\nWorking directory: {Text(server, "cwd")}");
            text.AppendLine($"Endpoint origin: {Text(server, "endpoint")}\r\nEnvironment names: {Names(server, "environment_names")}\r\nHeader names: {Names(server, "header_names")}\r\nOverrides: {Names(server, "overrides")}");
        }
        text.AppendLine("\r\nRead the complete source files privately before approving. Argument/environment/header values and URL paths/credentials are hidden here. TRUST permits these commands and connections as you, but starts nothing now. Native stdio workers remain unavailable.");
        return text.ToString();
    }
    internal static string[] ProfileNames(JsonElement? configuration, IEnumerable<string> trusted)
    {
        var configured = configuration is { ValueKind: JsonValueKind.Object } data && data.TryGetProperty("profiles", out var entries) && entries.ValueKind == JsonValueKind.Array
            ? entries.EnumerateArray().Select(item => item.GetProperty("name").GetString()!).ToArray() : Array.Empty<string>();
        string[] names = configured.Concat(trusted).Distinct(StringComparer.Ordinal).ToArray();
        if (names.Length != 0) return names;
        string active = configuration is { ValueKind: JsonValueKind.Object } value && value.TryGetProperty("active", out var field) && field.ValueKind == JsonValueKind.String ? field.GetString()! : "default";
        return [active];
    }
    internal static async Task<(JsonElement Trusted, JsonElement? Profiles, JsonElement? Sources)> LoadData(Func<string, Task<JsonElement?>> request)
    {
        JsonElement trusted = Body(await request("mcp-trusted"));
        JsonElement? profiles = null, sources = null;
        try { profiles = Body(await request("mcp-profiles")); } catch { }
        try { sources = Body(await request("mcp-sources")); } catch { }
        return (trusted, profiles, sources);
    }
    private async Task Reload()
    {
        InvalidateReview();
        grants.Clear();
        var (trusted, profileData, sourceData) = await LoadData(action => Engine.Control(action));
        if (closing) return;
        configurationUnavailable = profileData == null || sourceData == null;
        foreach (var grant in trusted.EnumerateArray()) grants[grant.GetProperty("profile").GetString()!] = grant.GetProperty("status").GetString()!;
        var names = ProfileNames(profileData, grants.Keys);
        profiles.Items.Clear(); profiles.Items.AddRange(names.Select(name => (object)new Profile(name)).ToArray()); profiles.SelectedIndex = 0;
        sources.Items.Clear(); sources.Items.Add(new Source("Daemon configuration: user plus explicitly selected project"));
        sources.Items.Add(new Source("Choose an MCP configuration file…", File: true));
        foreach (var source in sourceData is { ValueKind: JsonValueKind.Array } available ? available.EnumerateArray().ToArray() : Array.Empty<JsonElement>())
            if (source.GetProperty("status").GetString() == "available_for_review")
                sources.Items.Add(new Source($"{source.GetProperty("label").GetString()} — {source.GetProperty("path").GetString()}", source.GetProperty("id").GetString()));
        sources.SelectedIndex = 0;
    }
    private async Task Preview()
    {
        if (profiles.SelectedItem is not Profile profile || sources.SelectedItem is not Source source) return;
        InvalidateReview();
        Dictionary<string, string> selection = new() { ["profile"] = profile.Name };
        if (source.File)
        {
            using OpenFileDialog picker = new() { Title = "Choose MCP configuration", CheckFileExists = true, Multiselect = false };
            if (picker.ShowDialog(this) != DialogResult.OK) return;
            selection["project_path"] = picker.FileName;
        }
        else if (source.Id != null) selection["source_id"] = source.Id;
        JsonElement body = Body(await Engine.Control("mcp-preview", selection: selection));
        string id = ReviewId(body, profile.Name);
        if (closing) { await Engine.Control("mcp-decision", decision: new { action = "cancel", review_id = id }); return; }
        pending = id;
        details.Text = Describe(body);
        message.Text = "Review the source files and metadata below. Type TRUST and select Approve snapshot to authorize this exact snapshot for future sessions. Changed or expired snapshots require a fresh review.";
    }
    private async Task Approve()
    {
        if (pending is not { } id || confirmation.Text != "TRUST") return;
        Body(await Engine.Control("mcp-decision", decision: new { action = "approve", review_id = id }));
        pending = null;
        await Reload();
        if (!closing) message.Text = "Snapshot approved for future sessions. No server was started; existing sessions retain their original attachments.";
    }
    private async Task Revoke()
    {
        if (profiles.SelectedItem is not Profile profile || confirmation.Text != "REVOKE") return;
        Body(await Engine.Control("mcp-decision", decision: new { action = "revoke", profile = profile.Name }));
        await Reload();
        if (!closing) message.Text = "Profile revoked for future sessions. Close existing sessions to stop their attached servers.";
    }
}
