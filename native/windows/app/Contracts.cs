using System.Runtime.InteropServices;

namespace Hypermesh.Visor;

internal static class Contracts
{
    private static void Require(bool condition) { if (!condition) throw new Exception("Native companion contract failed"); }
    private static void Refuses(Action action) { try { action(); } catch (InvalidOperationException) { return; } catch (ArgumentOutOfRangeException) { return; } throw new Exception("Expected refusal"); }
    internal static int Run()
    {
        try
        {
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
