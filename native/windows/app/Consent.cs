namespace Hypermesh.Visor;

// No remote prompt, session text, environment variable or poll may grant consent.
internal sealed class Consent
{
    internal string? Selected { get; private set; }
    internal string? Approved { get; private set; }
    internal bool Allowed { get; private set; }
    internal long Generation { get; private set; }
    internal (string Session, ulong Epoch, long Generation)? Idle { get; private set; }
    internal long LastInput { get; private set; }
    private bool pendingApproval;
    internal bool MonitorLost { get; private set; }
    internal void Select(string? session)
    {
        if (session == Selected) return;
        Selected = session; Approved = null; Pause();
    }
    internal long BeginApproval() { Pause(); pendingApproval = true; return Generation; }
    internal bool PendingApproval(string session, long generation) => !MonitorLost && pendingApproval && generation == Generation && session == Selected;
    internal bool Approve(string session, long generation)
    {
        if (!PendingApproval(session, generation)) return false;
        pendingApproval = false; Approved = session; Allowed = true; Idle = null; return true;
    }
    internal void Pause() { pendingApproval = false; Allowed = false; Idle = null; Generation++; }
    internal void Stop() { Pause(); Approved = null; }
    internal void LoseMonitor() { MonitorLost = true; Stop(); }
    internal (string Session, long Generation)? Input(long now)
    {
        LastInput = now;
        if (pendingApproval) { Pause(); return null; }
        if (!Allowed || Selected == null || Approved != Selected) return null;
        Allowed = false; Generation++;
        return (Selected, Generation);
    }
    internal void RememberIdle(string session, long generation, ulong epoch)
    {
        if (!MonitorLost && !Allowed && generation == Generation && session == Selected && session == Approved)
            Idle = (session, epoch, generation);
    }
    internal bool CanResume(long now, bool heldInput) => !MonitorLost && !heldInput && Idle is { } ticket
        && !Allowed && ticket.Generation == Generation && ticket.Session == Selected && Approved == Selected
        && now - LastInput >= 120_000;
    internal bool Resume(string session, long generation, long now, bool heldInput)
    {
        if (!CanResume(now, heldInput) || Idle is not { } ticket || generation != Generation || ticket.Generation != generation || session != Selected || session != Approved) return false;
        Allowed = true; Idle = null; return true;
    }
}

// A late asynchronous grant cannot authorize operations after newer input.
internal sealed class InputAuthority
{
    private long generation;
    private long approvedGeneration = -1;
    internal long Generation => Interlocked.Read(ref generation);
    internal bool Allowed => Interlocked.Read(ref approvedGeneration) == Generation;
    internal void Revoke() => Interlocked.Increment(ref generation);
    internal void Grant(long expectedGeneration) => Interlocked.Exchange(ref approvedGeneration, expectedGeneration);
}
