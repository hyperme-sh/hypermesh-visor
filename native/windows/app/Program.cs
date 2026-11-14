namespace Hypermesh.Visor;

internal static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        if (args.SequenceEqual(new[] { "--self-test" })) return Contracts.Run();
        if (args.Length != 0) return 2;
        Application.SetHighDpiMode(HighDpiMode.PerMonitorV2);
        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        using Companion window = new();
        Application.Run(window);
        return 0;
    }
}
