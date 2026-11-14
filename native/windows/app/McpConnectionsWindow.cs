using System.Text;
using System.Text.Json;

namespace Hypermesh.Visor;

internal sealed class McpConnectionsWindow : Form
{
    internal sealed record Target(string Session, string Server, string State)
    {
        public override string ToString() => McpTrustWindow.Safe(Server) + " / " + McpTrustWindow.Safe(State);
    }
    private readonly ComboBox targets = new() { DropDownStyle = ComboBoxStyle.DropDownList, Width = 440 };
    private readonly TextBox details = new() { Multiline = true, ReadOnly = true, ScrollBars = ScrollBars.Both, WordWrap = false, Dock = DockStyle.Fill };
    private readonly Label message = new() { Dock = DockStyle.Bottom, Height = 58, Padding = new Padding(8) };
    private readonly Button reload = new() { Text = "Read status", AutoSize = true };
    private readonly Button refresh = new() { Text = "Refresh tools", AutoSize = true };
    private readonly Button reconnect = new() { Text = "Reconnect", AutoSize = true };
    private bool busy, closing;

    internal McpConnectionsWindow()
    {
        Text = "MCP connections"; Width = 780; Height = 540;
        FlowLayoutPanel actions = new() { Dock = DockStyle.Top, AutoSize = true, Padding = new Padding(8) };
        actions.Controls.AddRange(new Control[] { targets, reload, refresh, reconnect });
        Controls.Add(details); Controls.Add(message); Controls.Add(actions);
        targets.SelectedIndexChanged += (_, _) => Buttons();
        reload.Click += async (_, _) => await Run(Read);
        refresh.Click += async (_, _) => await Change(false);
        reconnect.Click += async (_, _) => await Change(true);
        Shown += async (_, _) => await Run(Read);
        FormClosed += (_, _) => closing = true;
        Buttons();
    }

    private void Buttons()
    {
        if (closing) return;
        targets.Enabled = reload.Enabled = !busy;
        refresh.Enabled = reconnect.Enabled = !busy && targets.SelectedItem is Target;
    }
    internal static JsonElement Body(JsonElement? response)
    {
        if (response is not { } value || !value.TryGetProperty("status", out var status) || !status.TryGetInt32(out int code) || code != 200 ||
            !value.TryGetProperty("body", out var body) || body.ValueKind != JsonValueKind.Object)
            throw new InvalidOperationException("MCP status unavailable");
        return body;
    }
    internal static Target[] Targets(JsonElement body)
    {
        if (!body.TryGetProperty("session", out var session) || session.ValueKind != JsonValueKind.Object ||
            !session.TryGetProperty("id", out var id) || id.ValueKind != JsonValueKind.String || !Guid.TryParseExact(id.GetString(), "D", out var parsed) ||
            !session.TryGetProperty("mcp_attachments", out var items) || items.ValueKind != JsonValueKind.Array) return [];
        return items.EnumerateArray().Take(16).Select(item =>
        {
            if (item.ValueKind != JsonValueKind.Object || !item.TryGetProperty("server_id", out var field) || field.ValueKind != JsonValueKind.String ||
                !item.TryGetProperty("routing_status", out var state) || state.ValueKind != JsonValueKind.String) return null;
            string name = field.GetString()!, routing = state.GetString()!;
            return name.Length == 0 || Encoding.UTF8.GetByteCount(name) > 512 || name.Trim() != name || routing == "refreshing"
                ? null : new Target(parsed.ToString(), name, routing);
        }).OfType<Target>().ToArray();
    }
    internal static string Describe(JsonElement body)
    {
        if (!body.TryGetProperty("session", out var session) || session.ValueKind != JsonValueKind.Object) return "No active session.";
        // Fixed status fields only: do not render config, arguments, remote metadata or credentials.
        string Text(JsonElement item, string key) => item.TryGetProperty(key, out var value) && value.ValueKind is JsonValueKind.String or JsonValueKind.Number or JsonValueKind.True or JsonValueKind.False
            ? McpTrustWindow.Safe(value.ToString()) : "unavailable";
        StringBuilder result = new($"Session: {Text(session, "id")}\r\nPrefer MCP: {Text(session, "prefer_mcp")}\r\n");
        if (session.TryGetProperty("mcp_match", out var match) && match.ValueKind == JsonValueKind.Object)
            foreach (string key in new[] { "reason", "bindings_status", "binding_index", "identity_matches", "healthy_matches" }) result.AppendLine($"{key}: {Text(match, key)}");
        result.AppendLine($"Omitted attachments: {Text(session, "mcp_attachments_omitted")}");
        if (session.TryGetProperty("mcp_attachments", out var items) && items.ValueKind == JsonValueKind.Array)
            foreach (var item in items.EnumerateArray().Take(16).Where(v => v.ValueKind == JsonValueKind.Object))
                result.AppendLine($"\r\n{Text(item, "name")} / {Text(item, "routing_status")}");
        return result.ToString();
    }
    private async Task Read()
    {
        JsonElement body = Body(await Engine.Control("mcp-status"));
        if (closing) return;
        Target[] choices = Targets(body);
        targets.Items.Clear(); targets.Items.AddRange(choices);
        if (choices.Length != 0) targets.SelectedIndex = 0;
        details.Text = Describe(body);
        message.Text = "Status does not grant desktop access. Select an existing attachment; refreshing or unaddressable targets are excluded.";
    }
    private async Task Run(Func<Task> action)
    {
        if (busy || closing) return;
        busy = true; Buttons();
        try { await action(); }
        catch
        {
            if (!closing)
            {
                targets.Items.Clear(); details.Clear();
                message.Text = "Request failed or its outcome is unknown. Read status before deciding whether to retry. No request was retried.";
            }
        }
        finally { busy = false; Buttons(); }
    }
    internal static async Task Execute(Target selected, bool restart, Func<string, string, string, Task<JsonElement?>> send)
    {
        JsonElement? response = await send(restart ? "mcp-reconnect" : "mcp-refresh", selected.Session, selected.Server);
        if (response is not { } value || !value.TryGetProperty("status", out var status) || !status.TryGetInt32(out int code) || code != 204)
            throw new InvalidOperationException("Connection outcome unavailable");
    }
    private async Task Change(bool restart)
    {
        if (busy || closing || targets.SelectedItem is not Target selected) return;
        string explanation = restart ? "Closes the old connection and may restart its server using this session's saved configuration." : "Reads tools from the existing connection. Does not restart a disconnected server.";
        if (MessageBox.Show(this, $"{selected}\nSession: {selected.Session}\n\n{explanation}\nNo tool call is replayed.", restart ? "Reconnect MCP server?" : "Refresh MCP tools?", MessageBoxButtons.OKCancel, MessageBoxIcon.Question) != DialogResult.OK) return;
        await Run(async () =>
        {
            await Execute(selected, restart, (action, session, server) => Engine.Control(action, session, server: server));
            if (closing) return;
            targets.Items.Clear();
            message.Text = "Connection action completed. Read status to see the updated catalog and matching state. Desktop access remains paused.";
        });
    }
}
