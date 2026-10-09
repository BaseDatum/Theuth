namespace Theuth.Companion.Core;

// 153 §13.3 — the clipboard lease, the pure half of the Windows twin (W153D). The app (AnyApp/Clipboard.cs) owns the Win32 calls;
// this machine decides the order and owns the generations, so a late settle timer can never restore over a newer lease.

/// <summary>Where one lease is (§13.3 steps 1–5).</summary>
public enum LeasePhase
{
    /// <summary>The slot is free: the next utterance may begin.</summary>
    Idle,
    /// <summary>1 — the user's clipboard is being copied (every format, capped).</summary>
    Snapshotting,
    /// <summary>2 — our one item: CF_UNICODETEXT + the three clipboard-history hints + the session marker.</summary>
    Writing,
    /// <summary>3 — Ctrl+V into the foreground thread (our hotkey modifiers released first).</summary>
    Pasting,
    /// <summary>4 — pasted; the settle timer runs (500 ms) while the verifier reads the field.</summary>
    Holding,
    /// <summary>5 — the snapshot is being put back.</summary>
    Restoring,
}

/// <summary>What the app must do next for a lease; every effect carries the lease's generation.</summary>
public abstract record LeaseEffect(int Generation)
{
    /// <summary>Step 1: copy every clipboard format (CF_UNICODETEXT, CF_HDROP, CF_DIB, registered formats by name; 32 MiB per format).</summary>
    public sealed record TakeSnapshot(int Generation) : LeaseEffect(Generation);

    /// <summary>Step 2: write <paramref name="Text"/> + the history hints + the registered session format holding <paramref name="Marker"/>.</summary>
    public sealed record Write(int Generation, string Text, string Marker) : LeaseEffect(Generation);

    /// <summary>Step 3: release our physical modifiers, then Ctrl+V.</summary>
    public sealed record Paste(int Generation) : LeaseEffect(Generation);

    /// <summary>Step 5's timer: call <see cref="ClipboardLease.SettleDue"/> after <paramref name="Ms"/> (0 = now).</summary>
    public sealed record ArmSettle(int Generation, int Ms) : LeaseEffect(Generation);

    /// <summary>Step 6: run the verification triple (§13.4) for <paramref name="Text"/>, in parallel with the settle.</summary>
    public sealed record Verify(int Generation, string Text) : LeaseEffect(Generation);

    /// <summary>Put the snapshot back (it is still ours, or our own write failed half-way).</summary>
    public sealed record Restore(int Generation) : LeaseEffect(Generation);

    /// <summary>Forget the snapshot without restoring it (the user copied meanwhile, or "Leave the dictated text on the clipboard").</summary>
    public sealed record DropSnapshot(int Generation) : LeaseEffect(Generation);

    /// <summary>A failure kind for the card (§13.5); <paramref name="Text"/> is the utterance the user must not lose.</summary>
    public sealed record Failed(int Generation, DeliveryFailure Kind, string Text) : LeaseEffect(Generation);

    /// <summary>The slot is free again; <paramref name="Restored"/>: the user's clipboard was put back.</summary>
    public sealed record Done(int Generation, bool Restored, SettleOutcome Outcome = SettleOutcome.Nothing) : LeaseEffect(Generation);
}

/// <summary>How a lease ended (H3-D1 names): the user's clipboard put back; the user's newer copy kept; our text left on purpose
/// ("leave on clipboard"); a stale generation (no effects at all); nothing to do (a failed snapshot, write or restore).</summary>
public enum SettleOutcome
{
    Restored,
    KeptUsers,
    LeftText,
    Stale,
    Nothing,
}

/// <summary>
/// 153 §13.3 — snapshot → write → paste → settle, one lease slot, generation-counted. Deliveries are serialised: <see cref="Begin"/>
/// answers false while a lease is in flight (the caller queues the utterance and begins it when <see cref="Ready"/> again). The
/// settle at 500 ms restores the snapshot only if the clipboard still holds our session marker (the app reads the registered
/// format and passes <c>stillOurs</c>); a user's copy in between is left alone. Every input names its generation, and an input for
/// an older generation is a no-op — a settle timer that fires after a newer lease began cannot restore over it.
/// </summary>
public sealed class ClipboardLease
{
    /// <summary>§13.3 step 5: the settle runs this long after the paste.</summary>
    public const int SettleMs = 500;

    /// <summary>§13.3 step 1: a representation larger than this makes the restore keep only the text formats.</summary>
    public const long MaxRepresentationBytes = 32L * 1024 * 1024;

    /// <summary>The registered clipboard format that marks our item (§13.3 step 2, the Windows "io.theuth.dictation-session").</summary>
    public const string SessionFormat = "theuth.dictation-session";

    /// <summary>The three Windows clipboard-history hints written with our item (§13.3 step 2, §13.9).</summary>
    public const string ExcludeFromMonitors = "ExcludeClipboardContentFromMonitorProcessing";
    public const string CanIncludeInHistory = "CanIncludeInClipboardHistory";
    public const string CanUploadToCloud = "CanUploadToCloudClipboard";

    private string text = "";

    public ClipboardLease(string gestureId, bool leaveOnClipboard = false)
    {
        GestureId = gestureId;
        LeaveOnClipboard = leaveOnClipboard;
    }

    /// <summary>The gesture this lease slot belongs to; the session marker is "‹gesture›:‹generation›".</summary>
    public string GestureId { get; }

    /// <summary>"Leave the dictated text on the clipboard" (default off): skips step 5's restore.</summary>
    public bool LeaveOnClipboard { get; set; }

    public LeasePhase Phase { get; private set; } = LeasePhase.Idle;

    /// <summary>The current lease; bumped by every <see cref="Begin"/>.</summary>
    public int Generation { get; private set; }

    /// <summary>A snapshot is held for the current generation (taken, not yet restored or dropped).</summary>
    public bool HoldsSnapshot { get; private set; }

    /// <summary>The slot is free.</summary>
    public bool Ready => Phase == LeasePhase.Idle;

    /// <summary>Leases that ended with the user's clipboard put back, and those that ended without (the debug row's counters, §16.1 #26).</summary>
    public int RestoredCount { get; private set; }

    public int Delivered { get; private set; }

    /// <summary>The marker the session format holds for <paramref name="generation"/>.</summary>
    public string MarkerFor(int generation) => $"{GestureId}:{generation}";

    /// <summary>Is the marker read from the clipboard one of this gesture's (any generation)? A null marker is never ours.</summary>
    public bool IsOurs(string? markerOnClipboard) =>
        markerOnClipboard is { Length: > 0 } m && m.StartsWith(GestureId + ":", StringComparison.Ordinal);

    /// <summary>Begin a lease for <paramref name="utterance"/>: false (nothing to do) while the slot is busy; else step 1.</summary>
    public bool Begin(string utterance, out IReadOnlyList<LeaseEffect> effects)
    {
        if (Phase == LeasePhase.Holding && HoldsSnapshot)
        {
            // H3-D1: a Begin over our own unsettled item keeps the PREVIOUS snapshot — the pending settle turns stale and our transient
            // text is never taken for the user's clipboard.
            Generation++;
            text = utterance;
            WriteSequence = 0;
            Phase = LeasePhase.Writing;
            effects = new LeaseEffect[] { new LeaseEffect.Write(Generation, text, MarkerFor(Generation)) };
            return true;
        }
        if (Phase != LeasePhase.Idle)
        {
            effects = Array.Empty<LeaseEffect>();
            return false;
        }
        Generation++;
        text = utterance;
        WriteSequence = 0;
        if (HoldsSnapshot)
        {
            // A restore that failed kept the snapshot: the clipboard may still hold our text, so the user's snapshot is the one to keep.
            Phase = LeasePhase.Writing;
            effects = new LeaseEffect[] { new LeaseEffect.Write(Generation, text, MarkerFor(Generation)) };
            return true;
        }
        Phase = LeasePhase.Snapshotting;
        effects = new LeaseEffect[] { new LeaseEffect.TakeSnapshot(Generation) };
        return true;
    }

    /// <summary>Step 1 finished. A failed snapshot fails the lease without touching the clipboard (nothing could be put back).</summary>
    public IReadOnlyList<LeaseEffect> Snapshotted(int generation, bool ok)
    {
        if (Stale(generation, LeasePhase.Snapshotting)) return Array.Empty<LeaseEffect>();
        if (!ok)
        {
            Phase = LeasePhase.Idle;
            return new LeaseEffect[] { new LeaseEffect.Failed(generation, DeliveryFailure.ClipboardSnapshotFailed, text), new LeaseEffect.Done(generation, false) };
        }
        HoldsSnapshot = true;
        Phase = LeasePhase.Writing;
        return new LeaseEffect[] { new LeaseEffect.Write(generation, text, MarkerFor(generation)) };
    }

    /// <summary>Step 2 finished. A failed write restores the snapshot (EmptyClipboard may have run) and reports the failure.</summary>
    public IReadOnlyList<LeaseEffect> Written(int generation, bool ok, uint sequence = 0)
    {
        if (Stale(generation, LeasePhase.Writing)) return Array.Empty<LeaseEffect>();
        WriteSequence = ok ? sequence : 0;
        if (!ok)
        {
            Phase = LeasePhase.Restoring;
            return new LeaseEffect[] { new LeaseEffect.Failed(generation, DeliveryFailure.ClipboardWriteFailed, text), new LeaseEffect.Restore(generation) };
        }
        Phase = LeasePhase.Pasting;
        return new LeaseEffect[] { new LeaseEffect.Paste(generation) };
    }

    /// <summary>Step 3 finished. A refused paste (SendInput blocked — an elevated window) settles at once and reports the failure.</summary>
    public IReadOnlyList<LeaseEffect> Pasted(int generation, bool ok)
    {
        if (Stale(generation, LeasePhase.Pasting)) return Array.Empty<LeaseEffect>();
        Phase = LeasePhase.Holding;
        if (!ok) return new LeaseEffect[] { new LeaseEffect.Failed(generation, DeliveryFailure.PasteCommandFailed, text), new LeaseEffect.ArmSettle(generation, 0) };
        Delivered++;
        return new LeaseEffect[] { new LeaseEffect.ArmSettle(generation, SettleMs), new LeaseEffect.Verify(generation, text) };
    }

    /// <summary>
    /// Step 5: the settle timer of <paramref name="generation"/> fired. A stale generation is a no-op. Still ours → restore; the user
    /// copied meanwhile (<paramref name="stillOurs"/> false) → leave their clipboard alone; "leave on clipboard" → drop the snapshot.
    /// </summary>
    public IReadOnlyList<LeaseEffect> SettleDue(int generation, bool stillOurs)
    {
        if (Stale(generation, LeasePhase.Holding)) return Array.Empty<LeaseEffect>();
        if (LeaveOnClipboard || !stillOurs)
        {
            Phase = LeasePhase.Idle;
            HoldsSnapshot = false;
            return new LeaseEffect[] { new LeaseEffect.DropSnapshot(generation), new LeaseEffect.Done(generation, false, LeaveOnClipboard && stillOurs ? SettleOutcome.LeftText : SettleOutcome.KeptUsers) };
        }
        Phase = LeasePhase.Restoring;
        return new LeaseEffect[] { new LeaseEffect.Restore(generation) };
    }

    /// <summary>The restore finished; a failed one is reported (the card says "clipboard not restored").</summary>
    public IReadOnlyList<LeaseEffect> Restored(int generation, bool ok)
    {
        if (Stale(generation, LeasePhase.Restoring)) return Array.Empty<LeaseEffect>();
        Phase = LeasePhase.Idle;
        // A failed restore keeps the snapshot: the next Begin writes over our own item without snapshotting it as the user's clipboard.
        HoldsSnapshot = !ok;
        if (!ok) return new LeaseEffect[] { new LeaseEffect.Failed(generation, DeliveryFailure.RestoreFailed, text), new LeaseEffect.Done(generation, false) };
        RestoredCount++;
        return new LeaseEffect[] { new LeaseEffect.Done(generation, true, SettleOutcome.Restored) };
    }

    /// <summary>
    /// The gesture ends (or the app quits) while a lease holds: settle now instead of at 500 ms — the same rule as
    /// <see cref="SettleDue"/> for the current generation. Nothing to do in any other phase (an in-flight step finishes by itself).
    /// </summary>
    public IReadOnlyList<LeaseEffect> SettleNow(bool stillOurs) =>
        Phase == LeasePhase.Holding ? SettleDue(Generation, stillOurs) : Array.Empty<LeaseEffect>();

    /// <summary>The clipboard sequence number right after our write (0 = unknown: the marker alone decides).</summary>
    public uint WriteSequence { get; private set; }

    /// <summary>
    /// H3-D1 "still ours": the clipboard sequence number is the one our write produced (nobody wrote since) AND the clipboard carries this
    /// lease's session marker for <paramref name="generation"/>. A sequence of 0 on either side (no window-station access) leaves the marker.
    /// </summary>
    public bool StillOurs(int generation, uint sequenceNow, string? markerNow)
    {
        if (!string.Equals(markerNow, MarkerFor(generation), StringComparison.Ordinal)) return false;
        return WriteSequence == 0 || sequenceNow == 0 || sequenceNow == WriteSequence;
    }

    private bool Stale(int generation, LeasePhase expected) => generation != Generation || Phase != expected;
}

/// <summary>One clipboard format as the snapshot sees it before copying (GlobalSize is cheap; the copy is not).</summary>
public sealed record ClipFormat(uint Id, string Name, long Bytes);

/// <summary>§13.3 step 1's policy: which formats the snapshot copies. Pure, so the cap and the GDI rules are tested on the Mac.</summary>
public static class ClipboardSnapshotPolicy
{
    public const uint CfText = 1, CfBitmap = 2, CfMetafilePict = 3, CfOemText = 7, CfDib = 8, CfPalette = 9, CfUnicodeText = 13,
        CfEnhMetafile = 14, CfHDrop = 15, CfLocale = 16, CfDibV5 = 17, CfOwnerDisplay = 0x80, CfDspText = 0x81, CfDspBitmap = 0x82,
        CfDspMetafilePict = 0x83, CfDspEnhMetafile = 0x8E, CfPrivateFirst = 0x200, CfPrivateLast = 0x2FF, CfGdiObjFirst = 0x300,
        CfGdiObjLast = 0x3FF;

    /// <summary>Text formats: what the restore keeps when any representation is over the cap.</summary>
    public static bool IsText(uint id) => id is CfUnicodeText or CfText or CfOemText or CfLocale;

    /// <summary>Formats whose handle is not a copyable HGLOBAL (GDI objects, metafiles, owner-drawn, private ranges) — never copied;
    /// CF_BITMAP is re-synthesised by Windows from CF_DIB on restore.</summary>
    public static bool IsHandleFormat(uint id) =>
        id is CfBitmap or CfMetafilePict or CfPalette or CfEnhMetafile or CfOwnerDisplay or CfDspBitmap or CfDspMetafilePict or CfDspEnhMetafile
        || (id >= CfPrivateFirst && id <= CfPrivateLast) || (id >= CfGdiObjFirst && id <= CfGdiObjLast);

    /// <summary>Our own formats are never part of a snapshot (a snapshot taken over our item would restore our text).</summary>
    public static bool IsOurs(string name) =>
        name is ClipboardLease.SessionFormat or ClipboardLease.ExcludeFromMonitors or ClipboardLease.CanIncludeInHistory or ClipboardLease.CanUploadToCloud;

    /// <summary>The ids to copy, in clipboard order: HGLOBAL formats, not ours; any representation over the cap → text formats only.</summary>
    public static IReadOnlyList<uint> Plan(IReadOnlyList<ClipFormat> formats)
    {
        var copyable = formats.Where(f => !IsHandleFormat(f.Id) && !IsOurs(f.Name)).ToList();
        bool tooBig = copyable.Any(f => f.Bytes > ClipboardLease.MaxRepresentationBytes);
        return copyable.Where(f => !tooBig || IsText(f.Id)).Where(f => f.Bytes <= ClipboardLease.MaxRepresentationBytes).Select(f => f.Id).ToList();
    }
}