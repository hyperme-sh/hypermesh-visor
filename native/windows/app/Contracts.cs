using System.Runtime.InteropServices;
using System.Text.Json;

namespace Hypermesh.Visor;

internal static class Contracts
{
    private static void Require(bool condition) { if (!condition) throw new Exception("Native companion contract failed"); }
    private static void Refuses(Action action) { try { action(); } catch (InvalidOperationException) { return; } catch (ArgumentOutOfRangeException) { return; } throw new Exception("Expected refusal"); }
    internal static int Run()
    {
        try
        {
            string[] unicodeProfiles = ["caf\u00e9", "cafe\u0301"];
            Require(McpTrustWindow.ProfileNames(null, unicodeProfiles).Length == 2);
            Require(McpTrustWindow.ProfileNames(null, ["approved-profile"]).SequenceEqual(["approved-profile"]));
            var partial = McpTrustWindow.LoadData(action => Task.FromResult<JsonElement?>(action == "mcp-trusted"
                ? JsonSerializer.SerializeToElement(new { status = 200, body = new[] { new { profile = "approved-profile", status = "approved" } } })
                : JsonSerializer.SerializeToElement(new { status = 500 }))).GetAwaiter().GetResult();
            Require(partial.Profiles == null && partial.Sources == null && partial.Trusted.GetArrayLength() == 1);
            JsonElement review = JsonSerializer.SerializeToElement(new {
                profile = "default", review_id = "00000000-0000-4000-8000-000000000001", profiles_path = "profiles.json",
                servers = new[] { new { name = "example", status = "selected", transport = "http", enabled = true, argument_count = 1,
                    environment_names = new[] { "KEY" }, header_names = new[] { "Authorization" }, overrides = Array.Empty<string>(),
                    environment = new { KEY = "hidden-secret" }, args = new[] { "hidden-secret" } } }
            });
            Require(McpTrustWindow.ReviewId(review, "default") == "00000000-0000-4000-8000-000000000001");
            Refuses(() => McpTrustWindow.ReviewId(review, "another-profile"));
            string description = McpTrustWindow.Describe(review);
            Require(description.Contains("KEY") && description.Contains("Authorization") && !description.Contains("hidden-secret"));
            Require(McpTrustWindow.Safe("name\u2028Endpoint origin: attacker\u2029") == "name\\u2028Endpoint origin: attacker\\u2029");
            Require(McpTrustWindow.Safe("name\n\u202e") == "name\\u000a\\u202e");
            Require(CompactUsage.DisplayPhase("running", false) == "paused");
            Require(CompactUsage.DisplayPhase("running", true) == "running");
            Require(CompactUsage.DisplayPhase("off", false) == "off");
            Require(CompactUsage.DisplayPhase("paused", true) == "paused");
            Require(new CompactUsage(100, 50).Widths() == (28, 14));
            Require(new CompactUsage(0, 0).Widths() == (0, 0));
            Require(new CompactUsage(ulong.MaxValue, 1).Widths() == (28, 1));
            Require(CompactUsage.Read(JsonSerializer.SerializeToElement(new { last_response_usage = new { input_tokens = 100, output_tokens = 50 } })) == new CompactUsage(100, 50));
            foreach (string missing in new[] { "null", "{}", "{\"last_response_usage\":null}", "{\"last_response_usage\":{\"input_tokens\":-1,\"output_tokens\":2}}" })
                Require(CompactUsage.Read(JsonSerializer.Deserialize<JsonElement>(missing)) == null);
            const string selected = "00000000-0000-4000-8000-000000000001";
            JsonElement DestinationResponse(string? forum) => JsonSerializer.SerializeToElement(new { status = 200, body = new { session = selected, forum } });
            Require(Destinations.Resolve(DestinationResponse("https://forum.example.test/t/1"), selected, selected, false) == "https://forum.example.test/t/1");
            Refuses(() => Destinations.Resolve(DestinationResponse("https://forum.example.test/t/1"), selected, "00000000-0000-4000-8000-000000000002", false));
            Refuses(() => Destinations.Resolve(null, selected, selected, false));
            foreach (string? url in new string?[] { null, "file:///C:/Windows", "http://example.test", "https://user:secret@example.test", "https://example.test/?token=x", "https://example.test/#x", "https://example.test/\n" })
                Refuses(() => Destinations.Resolve(DestinationResponse(url), selected, selected, false));
            var editorPlan = Destinations.EditorPlan(@"C:\Program Files\Microsoft VS Code\Code.exe", @"C:\workspace with spaces\$(not a command)");
            Require(!editorPlan.UseShellExecute && editorPlan.ArgumentList.SequenceEqual(new[] { "--new-window", "--", @"C:\workspace with spaces\$(not a command)" }));
            var terminalPlan = SessionTerminal.PowerShellPlan("powershell.exe", @"C:\bundle with spaces\$(literal)\hypermesh.exe", selected, "second-instance");
            Require(!terminalPlan.UseShellExecute && terminalPlan.Environment["HYPERMESH_SESSION_ID"] == selected);
            Require(terminalPlan.Environment["HYPERMESH_SESSION_CLI"] == @"C:\bundle with spaces\$(literal)\hypermesh.exe");
            Require(!terminalPlan.ArgumentList.Last().Contains("$(literal)") && !terminalPlan.ArgumentList.Last().Contains(selected));
            var wslPlan = SessionTerminal.WslPlan("wsl.exe", "/mnt/c/bundle with spaces/hypermesh.exe", selected, "second-instance");
            Require(wslPlan.ArgumentList.SequenceEqual(new[] { "--exec", "/bin/sh", "-c", SessionTerminal.WslScript, "visor-session", "/mnt/c/bundle with spaces/hypermesh.exe", selected, "second-instance" }));
            Require(!SessionTerminal.WslScript.Contains(selected) && SessionTerminal.WslScript.Contains("--raw-terminal"));
            Refuses(() => SessionTerminal.WslPlan("wsl.exe", "--help", selected, "default"));
            Refuses(() => SessionTerminal.PowerShellPlan("powershell.exe", "hypermesh.exe", "bad", "default"));
            Refuses(() => SessionTerminal.PowerShellPlan("powershell.exe", "hypermesh.exe", selected, "bad; command"));
            InputAuthority authority = new(); Require(!authority.AllowedFor("first"));
            long inputGeneration = authority.Generation;
            authority.Grant(inputGeneration, "first"); Require(authority.AllowedFor("first"));
            authority.Revoke(); Require(!authority.AllowedFor("first"));
            authority.Grant(inputGeneration, "first"); Require(!authority.AllowedFor("first"));
            inputGeneration = authority.Generation;
            authority.Grant(inputGeneration, "first"); Require(authority.AllowedFor("first"));
            authority.Revoke(); Require(!authority.AllowedFor("first"));
            authority.Grant(authority.Generation, "first");
            Require(!authority.AllowedFor("second") && !authority.AllowedFor(null));
            Consent consent = new();
            consent.Select("first"); Require(!consent.Allowed);
            long generation = consent.BeginApproval();
            consent.Pause(); Require(!consent.PendingApproval("first", generation) && !consent.Approve("first", generation));
            generation = consent.BeginApproval();
            consent.Stop(); Require(!consent.PendingApproval("first", generation) && !consent.Approve("first", generation));
            generation = consent.BeginApproval();
            consent.Input(100); Require(!consent.Approve("first", generation));
            generation = consent.BeginApproval(); Require(consent.Approve("first", generation));
            var ticket = consent.Input(1000)!.Value; Require(!consent.Allowed);
            consent.RememberIdle(ticket.Session, ticket.Generation, 9);
            Require(!consent.CanResume(120999, false)); Require(!consent.CanResume(121000, true));
            Require(consent.CanResume(121000, false));
            consent.Input(121000); Require(!consent.Resume(ticket.Session, ticket.Generation, 121001, false));
            Require(consent.Resume(ticket.Session, ticket.Generation, 241000, false));
            ticket = consent.Input(250000)!.Value; consent.RememberIdle(ticket.Session, ticket.Generation, 10);
            consent.Pause(); Require(!consent.CanResume(500000, false));
            generation = consent.BeginApproval(); consent.Select("second"); Require(!consent.Approve("first", generation));
            generation = consent.BeginApproval(); Require(consent.Approve("second", generation));
            consent.LoseMonitor(); Require(!consent.Allowed); generation = consent.BeginApproval(); Require(!consent.Approve("second", generation));
            Require(InputMonitor.IsOwn(true, 123, 123)); Require(!InputMonitor.IsOwn(false, 123, 123)); Require(!InputMonitor.IsOwn(true, 124, 123));
            nuint marker = InputMonitor.CreateMarker();
            Require(marker > 0 && marker < int.MaxValue);
            Require(InputMonitor.IsOwn(true, (nuint)(uint)marker, marker));
            Require(!InputMonitor.IsOwn(false, marker, marker));
            Require(!InputMonitor.IsOwn(true, marker | ((nuint)1 << 32), marker));
            Require(Marshal.SizeOf<Win32.Input>() == 40); Require(Marshal.SizeOf<Win32.MouseHook>() == 32); Require(Marshal.SizeOf<Win32.KeyHook>() == 24);
            Require(DesktopDriver.Normalize(1919, 1079, 1920, 1080) == (65535, 65535));
            Require(DesktopDriver.Normalize(0, 0, 1, 1) == (0, 0));
            Refuses(() => DesktopDriver.Normalize(-1, 0, 1920, 1080)); Refuses(() => DesktopDriver.Normalize(1920, 0, 1920, 1080));
            Win32.Input[] text = DesktopDriver.Keys(0x0101F680, true, false, 123);
            Require(text.Length == 2 && text.All(v => v.Data.Key.Flags == 4 && v.Data.Key.Extra == 123));
            foreach (uint shift in new uint[] { 0xFFE1, 0xFFE2 })
            {
                Win32.Input modifier = DesktopDriver.Keys(shift, true, false, 123).Single();
                Require(DesktopDriver.IsChordModifier(modifier));
                Win32.Input letter = DesktopDriver.Keys('a', true, DesktopDriver.IsChordModifier(modifier), 123).Single();
                Require(letter.Data.Key.Key == 'A' && letter.Data.Key.Flags == 0);
            }
            Require(DesktopDriver.Keys('a', true, false, 123).Single().Data.Key.Flags == 4);
            Require(DesktopDriver.Keys('c', true, true, 123).Single().Data.Key.Key == 'C');
            Require(DesktopDriver.Keys(0xFFE4, true, true, 123).Single().Data.Key.Flags == 1);
            Refuses(() => DesktopDriver.Keys('!', true, true, 123)); Refuses(() => DesktopDriver.Keys(0x0100D800, true, false, 123));
            var held = DesktopDriver.Held(DesktopDriver.Keys(0xFFE3, true, false, 123).Single());
            Require(held.Down && held.Release.Data.Key.Flags == 2 && held.Release.Data.Key.Extra == 123);
            var up = DesktopDriver.Held(held.Release); Require(!up.Down && up.Key == held.Key);
            Console.WriteLine("PASS consent, idle resume, stale approval, input provenance, coordinates, key plans, and release contracts");
            return 0;
        }
        catch (Exception error) { Console.Error.WriteLine(error.Message); return 1; }
    }
}
