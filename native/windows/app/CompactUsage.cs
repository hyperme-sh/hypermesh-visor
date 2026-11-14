using System.Text.Json;

namespace Hypermesh.Visor;

internal readonly record struct CompactUsage(ulong Input, ulong Output)
{
    internal string Summary => $"Last response: {Input} in / {Output} out";
    internal static CompactUsage? Read(JsonElement session)
    {
        if (session.ValueKind != JsonValueKind.Object || !session.TryGetProperty("last_response_usage", out var usage) || usage.ValueKind != JsonValueKind.Object ||
            !usage.TryGetProperty("input_tokens", out var input) || input.ValueKind != JsonValueKind.Number || !input.TryGetUInt64(out ulong inputCount) ||
            !usage.TryGetProperty("output_tokens", out var output) || output.ValueKind != JsonValueKind.Number || !output.TryGetUInt64(out ulong outputCount)) return null;
        return new(inputCount, outputCount);
    }
    internal (int Input, int Output) Widths()
    {
        double maximum = Math.Max(Input, Output);
        if (maximum == 0) return (0, 0);
        return (Input == 0 ? 0 : Math.Max(1, (int)(28 * (Input / maximum))), Output == 0 ? 0 : Math.Max(1, (int)(28 * (Output / maximum))));
    }
    internal static Icon Draw(Icon source, string phase, CompactUsage? usage)
    {
        using Bitmap image = new(32, 32);
        using (Graphics graphics = Graphics.FromImage(image))
        {
            graphics.DrawIcon(source, new Rectangle(0, 0, 32, 32));
            using SolidBrush state = new(phase == "running" ? Color.LimeGreen : phase == "paused" ? Color.Orange : Color.Gray);
            graphics.FillEllipse(Brushes.Black, 23, 1, 8, 8);
            graphics.FillEllipse(state, 24, 2, 6, 6);
            if (usage is { } counts)
            {
                var widths = counts.Widths();
                graphics.FillRectangle(Brushes.Black, 1, 21, 30, 10);
                graphics.FillRectangle(Brushes.DeepSkyBlue, 2, 22, widths.Input, 3);
                graphics.FillRectangle(Brushes.LimeGreen, 2, 27, widths.Output, 3);
            }
        }
        nint handle = image.GetHicon();
        try { return (Icon)Icon.FromHandle(handle).Clone(); }
        finally { Win32.DestroyIcon(handle); }
    }
}
