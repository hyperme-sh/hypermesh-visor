using System.Text;
using System.Text.Json;

namespace Hypermesh.Visor;

// Local drafting only. This type has no engine, network or persistence dependency.
internal sealed class FeedbackDraft
{
    internal const int Limit = 4096;
    private static readonly UTF8Encoding Utf8 = new(false, true);
    internal bool OptedIn { get; private set; }
    internal string Preview { get; private set; } = "";
    internal void Consent(bool enabled) { OptedIn = enabled; Invalidate(); }
    internal void Invalidate() => Preview = "";
    internal bool Review(string category, string text)
    {
        Invalidate();
        if (!OptedIn || (category != "Problem" && category != "Idea") || string.IsNullOrWhiteSpace(text)) return false;
        try { if (Utf8.GetByteCount(text) > Limit) return false; }
        catch (EncoderFallbackException) { return false; }
        if (text.Any(c => char.IsControl(c) && c != '\n' && c != '\r' && c != '\t')) return false;
        Preview = $"Local preview — nothing has been sent.\r\nCategory: {category}\r\n\r\n{text}";
        return true;
    }
}

internal sealed class FeedbackWindow : Form
{
    private bool cleared;
    private readonly FeedbackDraft draft = new();
    private readonly CheckBox consent = new() { Text = "I choose to draft human feedback on this device", AutoSize = true };
    private readonly ComboBox category = new() { DropDownStyle = ComboBoxStyle.DropDownList, Width = 160 };
    private readonly TextBox editor = new() { Multiline = true, AcceptsReturn = true, ScrollBars = ScrollBars.Vertical, Dock = DockStyle.Fill, MaxLength = FeedbackDraft.Limit };
    private readonly TextBox preview = new() { Multiline = true, ReadOnly = true, ScrollBars = ScrollBars.Vertical, Dock = DockStyle.Fill };
    private readonly Button review = new() { Text = "Preview locally", AutoSize = true };
    private readonly Label message = new() { AutoSize = true, MaximumSize = new Size(620, 0) };

    private readonly TextBox subject = new() { PlaceholderText = "Report title (4–160 characters)", MaxLength = 160, Dock = DockStyle.Fill };
    private readonly Button send = new() { Text = "Send report", AutoSize = true };
    private readonly Button withdraw = new() { Text = "Withdraw saved consent", AutoSize = true };
    private readonly string formId = Guid.NewGuid().ToString();
    private readonly bool configured;
    private bool connected, busy, retry;
    private int generation;

    internal FeedbackWindow(bool? configured = null)
    {
        this.configured = configured ?? Environment.GetEnvironmentVariable("HYPERMESH_FEEDBACK_ENABLED") == "true";
        Text = "Human feedback"; Size = new Size(700, 640); MinimumSize = new Size(540, 500);
        TableLayoutPanel layout = new() { Dock = DockStyle.Fill, ColumnCount = 1, RowCount = 8, Padding = new Padding(12) };
        layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.RowStyles.Add(new RowStyle(SizeType.Percent, 50));
        layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.RowStyles.Add(new RowStyle(SizeType.Percent, 50));
        layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.Controls.Add(new Label { AutoSize = true, MaximumSize = new Size(620, 0), Text = "Draft up to 4096 UTF-8 bytes. Do not include secrets or private documents. Nothing is collected automatically. Configured sending requires saved consent and a separate confirmation. Closing discards this draft." });
        layout.Controls.Add(consent); category.Items.AddRange(new object[] { "Problem", "Idea" }); category.SelectedIndex = 0;
        layout.Controls.Add(category); layout.Controls.Add(subject); layout.RowStyles.Insert(3,new RowStyle(SizeType.AutoSize)); layout.Controls.Add(editor);
        FlowLayoutPanel actions = new() { AutoSize = true, Dock = DockStyle.Fill };
        Button discard = new() { Text = "Discard and close", AutoSize = true };
        actions.Controls.AddRange(new Control[] { review, discard, send, withdraw }); layout.Controls.Add(actions);
        layout.Controls.Add(preview); layout.Controls.Add(message); Controls.Add(layout);
        consent.CheckedChanged += async (_, _) =>
        {
            generation++; draft.Consent(consent.Checked);
            if (!consent.Checked) { if (this.configured) _ = Engine.Feedback("feedback-close",formId); connected = false; busy = false; editor.Clear(); subject.Clear(); editor.ClearUndo(); category.SelectedIndex = 0; }
            InvalidatePreview(); UpdateControls();
            if (consent.Checked && this.configured) await BeginSubmission();
        };
        editor.TextChanged += (_, _) => InvalidatePreview();
        subject.TextChanged += (_, _) => InvalidatePreview();
        category.SelectedIndexChanged += (_, _) => InvalidatePreview();
        review.Click += (_, _) =>
        {
            bool valid = draft.Review(category.SelectedItem as string ?? "", editor.Text);
            preview.Text = draft.Preview == "" ? "" : subject.Text + "\r\n\r\n" + draft.Preview;
            message.Text = valid ? "Edit above to replace this preview. Nothing has been sent." : "Enter nonblank text within 4096 UTF-8 bytes, without unsupported control characters.";
            UpdateControls();
        };
        discard.Click += (_, _) => Close();
        send.Click += async (_, _) => await SendReport();
        withdraw.Click += async (_, _) => await WithdrawConsent();
        UpdateControls();
    }
    private void UpdateControls() {
        bool enabled = consent.Checked && !busy && !cleared;
        editor.Enabled = subject.Enabled = category.Enabled = review.Enabled = enabled;
        send.Enabled = enabled && connected && draft.Preview != "";
        send.Text = retry ? "Retry same report" : "Send report";
        withdraw.Enabled = enabled && connected;
    }
    private void InvalidatePreview() { draft.Invalidate(); preview.Clear(); retry = false; if (connected) _ = Engine.Feedback("feedback-discard",formId); message.Text = "Nothing has been sent."; UpdateControls(); }
    private static bool Success(JsonElement? value) => value is { } v && v.TryGetProperty("status",out var status) && status.TryGetInt32(out var number) && number == 200;
    private static bool Flag(JsonElement? value,string key) => value is { } v && v.TryGetProperty("body",out var body) && body.ValueKind == JsonValueKind.Object && body.TryGetProperty(key,out var flag) && flag.ValueKind == JsonValueKind.True;
    private bool Current(int stamp) => !cleared && !IsDisposed && generation == stamp;
    private bool Approve(string text) => MessageBox.Show(this,text,"Human feedback",MessageBoxButtons.OKCancel,MessageBoxIcon.Question) == DialogResult.OK;
    private async Task BeginSubmission() {
        int stamp = generation; busy = true; UpdateControls();
        var response = await Engine.Feedback("feedback-open",formId,true);
        if (!Current(stamp)) return;
        busy = false;
        if (!Success(response)) { message.Text = "Sending unavailable. Your draft remains local."; UpdateControls(); return; }
        connected = true;
        if (!Flag(response,"enabled") && Approve("Enable saved human-report consent for this account?")) {
            busy = true; UpdateControls(); response = await Engine.Feedback("feedback-enable",formId,true);
            if (!Current(stamp)) return;
            busy = false; message.Text = Success(response) ? "Consent saved. Preview before sending." : "Consent change unconfirmed. Nothing sent.";
        }
        UpdateControls();
    }
    private async Task WithdrawConsent() {
        if (busy || !connected || !Approve("Withdraw saved human-report consent and discard this draft?")) return;
        int stamp = generation; busy = true; draft.Consent(false); editor.Clear(); subject.Clear(); preview.Clear(); retry = false; UpdateControls();
        var response = await Engine.Feedback("feedback-withdraw",formId);
        FinishWithdrawal(response,stamp);
    }
    private void FinishWithdrawal(JsonElement? response,int stamp) {
        if (!Current(stamp)) return;
        busy = false; connected = false; consent.Checked = false;
        _ = Engine.Feedback("feedback-close",formId);
        message.Text = Success(response) ? "Saved consent withdrawn." : "Withdrawal unconfirmed. No automatic retry was made."; UpdateControls();
    }
    private async Task SendReport() {
        if (busy || !connected || !draft.OptedIn || draft.Preview == "") return;
        int stamp = generation;
        if (!retry) {
            busy = true; UpdateControls();
            var prepared = await Engine.Feedback("feedback-prepare",formId,true,category.SelectedItem as string ?? "",subject.Text,editor.Text);
            if (!Current(stamp)) return;
            busy = false;
            if (!Success(prepared)) { message.Text = "Report refused. Check consent and title/body limits."; UpdateControls(); return; }
        }
        if (!Approve(retry ? "Retry the exact previously confirmed report?" : "Send the report shown in this preview to intake?")) { if (!retry) _ = Engine.Feedback("feedback-discard",formId); UpdateControls(); return; }
        if (!Current(stamp) || !draft.OptedIn) return;
        busy = true; UpdateControls(); var response = await Engine.Feedback("feedback-submit",formId,true);
        if (!Current(stamp)) return;
        busy = false;
        if (Success(response)) { editor.Clear(); subject.Clear(); draft.Invalidate(); preview.Clear(); retry = false; message.Text = "Report submitted."; }
        else { retry = Flag(response,"unknown"); message.Text = retry ? "Outcome unknown. You may explicitly retry the same report." : "Report refused. No automatic retry was made."; }
        UpdateControls();
    }
    internal static void CheckContracts()
    {
        using FeedbackWindow form = new(configured:false);
        void Require(bool ok) { if (!ok) throw new Exception("Feedback form contract failed"); }
        Require(!form.editor.Enabled && !form.review.Enabled && !form.consent.Checked);
        form.consent.Checked = true;
        Require(form.editor.Enabled && form.review.Enabled);
        form.editor.Text = "first";
        Require(form.draft.Review("Problem", form.editor.Text));
        form.preview.Text = form.draft.Preview;
        form.editor.Text = "replacement";
        Require(form.preview.Text == "" && form.draft.Preview == "");
        Require(form.draft.Review("Problem", form.editor.Text));
        form.preview.Text = form.draft.Preview;
        form.category.SelectedIndex = 1;
        Require(form.preview.Text == "" && form.draft.Preview == "");
        form.consent.Checked = false;
        Require(form.editor.Text == "" && !form.editor.CanUndo && !form.editor.Enabled);
        form.consent.Checked = true;
        Require(form.editor.Text == "" && form.category.SelectedIndex == 0);
        form.generation++; form.connected = true;
        form.FinishWithdrawal(null,form.generation-1);
        Require(form.connected && form.consent.Checked);
        form.editor.Text = "discard this";
        form.Dispose();
        Require(!form.draft.OptedIn && form.draft.Preview == "");
    }
    protected override void Dispose(bool disposing)
    {
        if (disposing && !cleared) { cleared = true; generation++; if (configured) _ = Engine.Feedback("feedback-close",formId); draft.Consent(false); editor.Clear(); subject.Clear(); editor.ClearUndo(); preview.Clear(); }
        base.Dispose(disposing);
    }
}
