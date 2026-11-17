using System.Text;

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

    internal FeedbackWindow()
    {
        Text = "Human feedback — local draft"; Size = new Size(700, 640); MinimumSize = new Size(540, 500);
        TableLayoutPanel layout = new() { Dock = DockStyle.Fill, ColumnCount = 1, RowCount = 7, Padding = new Padding(12) };
        layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.RowStyles.Add(new RowStyle(SizeType.Percent, 50));
        layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.RowStyles.Add(new RowStyle(SizeType.Percent, 50));
        layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.Controls.Add(new Label { AutoSize = true, MaximumSize = new Size(620, 0), Text = "Draft up to 4096 UTF-8 bytes. Do not include secrets or private documents. Nothing is collected automatically. Sending is unavailable; closing or withdrawing consent discards this draft." });
        layout.Controls.Add(consent); category.Items.AddRange(new object[] { "Problem", "Idea" }); category.SelectedIndex = 0;
        layout.Controls.Add(category); layout.Controls.Add(editor);
        FlowLayoutPanel actions = new() { AutoSize = true, Dock = DockStyle.Fill };
        Button discard = new() { Text = "Discard and close", AutoSize = true };
        actions.Controls.AddRange(new Control[] { review, discard }); layout.Controls.Add(actions);
        layout.Controls.Add(preview); layout.Controls.Add(message); Controls.Add(layout);
        consent.CheckedChanged += (_, _) =>
        {
            draft.Consent(consent.Checked);
            if (!consent.Checked) { editor.Clear(); editor.ClearUndo(); category.SelectedIndex = 0; }
            InvalidatePreview(); UpdateControls();
        };
        editor.TextChanged += (_, _) => InvalidatePreview();
        category.SelectedIndexChanged += (_, _) => InvalidatePreview();
        review.Click += (_, _) =>
        {
            bool valid = draft.Review(category.SelectedItem as string ?? "", editor.Text);
            preview.Text = draft.Preview;
            message.Text = valid ? "Edit above to replace this preview. Nothing has been sent." : "Enter nonblank text within 4096 UTF-8 bytes, without unsupported control characters.";
        };
        discard.Click += (_, _) => Close();
        UpdateControls();
    }
    private void UpdateControls() => editor.Enabled = category.Enabled = review.Enabled = consent.Checked;
    private void InvalidatePreview() { draft.Invalidate(); preview.Clear(); message.Text = "Nothing has been sent."; }
    internal static void CheckContracts()
    {
        using FeedbackWindow form = new();
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
        form.editor.Text = "discard this";
        form.Dispose();
        Require(!form.draft.OptedIn && form.draft.Preview == "");
    }
    protected override void Dispose(bool disposing)
    {
        if (disposing && !cleared) { cleared = true; draft.Consent(false); editor.Clear(); editor.ClearUndo(); preview.Clear(); }
        base.Dispose(disposing);
    }
}
