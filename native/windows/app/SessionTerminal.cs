using System.Diagnostics;
using System.Text.RegularExpressions;

namespace Hypermesh.Visor;

internal static class SessionTerminal
{
    internal const string WslScript = "saved=$(stty -g) || exit 1; trap 'stty \"$saved\"' 0; trap 'exit 130' HUP INT TERM; stty raw -echo || exit 1; \"$1\" session-terminal --raw-terminal --session \"$2\" --instance \"$3\"; result=$?; exit \"$result\"";
    internal static ProcessStartInfo PowerShellPlan(string powershell, string cli, string session, string instance)
    {
        Validate(session, instance);
        ProcessStartInfo plan = new(powershell) { UseShellExecute = false };
        foreach (string argument in new[] { "-NoLogo", "-NoProfile", "-Command", "& $env:HYPERMESH_SESSION_CLI session-terminal --session $env:HYPERMESH_SESSION_ID --instance $env:HYPERMESH_VISOR_INSTANCE; exit $LASTEXITCODE" })
            plan.ArgumentList.Add(argument);
        // Values are data read by a fixed script, never interpolated into script text.
        plan.Environment["HYPERMESH_SESSION_CLI"] = cli;
        plan.Environment["HYPERMESH_SESSION_ID"] = session;
        plan.Environment["HYPERMESH_VISOR_INSTANCE"] = instance;
        return plan;
    }

    internal static ProcessStartInfo WslPlan(string wsl, string cli, string session, string instance)
    {
        Validate(session, instance);
        if (!cli.StartsWith('/') || cli.Length > 4096 || cli.Any(char.IsControl)) throw new InvalidOperationException("WSL path unavailable");
        ProcessStartInfo plan = new(wsl) { UseShellExecute = false };
        // A fixed wrapper owns the Linux PTY settings even when Windows interop supplies pipe handles.
        foreach (string argument in new[] { "--exec", "/bin/sh", "-c", WslScript, "visor-session", cli, session, instance }) plan.ArgumentList.Add(argument);
        return plan;
    }

    private static void Validate(string session, string instance)
    {
        if (!Guid.TryParseExact(session, "D", out _) || !Regex.IsMatch(instance, "\\A[a-zA-Z0-9-]{1,64}\\z"))
            throw new InvalidOperationException("Session or instance unavailable");
    }

    private static async Task<string> ReadBounded(StreamReader reader)
    {
        char[] buffer = new char[4097];
        int count = 0;
        while (count < buffer.Length)
        {
            int read = await reader.ReadAsync(buffer.AsMemory(count));
            if (read == 0) return new string(buffer, 0, count);
            count += read;
        }
        throw new InvalidOperationException("WSL response exceeds the path limit");
    }

    internal static async Task<ProcessStartInfo> Plan(bool wsl, string session)
    {
        string instance = Environment.GetEnvironmentVariable("HYPERMESH_VISOR_INSTANCE") ?? "default";
        Validate(session, instance);
        string cli = Path.Combine(AppContext.BaseDirectory, "hypermesh.exe");
        if (!File.Exists(cli)) throw new InvalidOperationException("Bundled CLI missing");
        string system = Environment.GetFolderPath(Environment.SpecialFolder.System);
        if (!wsl) return PowerShellPlan(Path.Combine(system, "WindowsPowerShell", "v1.0", "powershell.exe"), cli, session, instance);
        string executable = Path.Combine(system, "wsl.exe");
        ProcessStartInfo convert = new(executable) { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true };
        foreach (string argument in new[] { "--exec", "wslpath", "-a", "-u", cli }) convert.ArgumentList.Add(argument);
        using Process process = Process.Start(convert) ?? throw new InvalidOperationException("WSL unavailable");
        try
        {
            Task<string> output = ReadBounded(process.StandardOutput);
            Task<string> errors = ReadBounded(process.StandardError);
            await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(10));
            string path = (await output).TrimEnd('\r', '\n');
            await errors;
            if (process.ExitCode != 0) throw new InvalidOperationException("Configure a default WSL distribution with Windows interoperability");
            return WslPlan(executable, path, session, instance);
        }
        finally { if (!process.HasExited) { process.Kill(true); await process.WaitForExitAsync(); } }
    }
}
