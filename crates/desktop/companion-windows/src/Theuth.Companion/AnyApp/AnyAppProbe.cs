using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;

namespace Theuth.Companion.AnyApp;

/// <summary>
/// 153 §16.1 #24 (the Windows form of <c>--probe-anyapp</c>): drive the real any-app path on the FOCUSED window without a key or a voice —
/// the press (UIA assessment), fixture utterances fed as the engine's finals, the release, delivery (direct or lease), verification, the
/// pill, the card. Flags (values limited to the build service's argument alphabet, so <c>_</c> stands for a space):
/// <list type="bullet">
/// <item><c>--probe-anyapp &lt;text|short|long|multi|mail&gt;</c> after <c>--probe-anyapp-delay &lt;s&gt;</c> (default 5);
/// <c>--probe-anyapp-lease</c> forces the lease; <c>--probe-anyapp-send</c> says "send it" at the end;</item>
/// <item><c>--probe-anyapp-lock</c>: a synthetic WM_WTSSESSION_CHANGE(LOCK) to our own window, the probe text (refused), then UNLOCK;</item>
/// <item><c>--probe-anyapp-card</c>: the failure card from a fixture; <c>--probe-anyapp-rewrite &lt;instruction&gt;</c>: the rewrite on the
/// current selection;</item>
/// <item><c>--probe-anyapp-watch</c>: re-run on every write of <c>%LOCALAPPDATA%\theuth\companion\anyapp-probe.txt</c> (one line of
/// <c>key=value</c>: <c>text= lease= send= lock= card= rewrite= history=</c>), so one launch serves a whole live round.</item>
/// </list>
/// </summary>
internal static class AnyAppProbe
{
    private static readonly List<DispatcherQueueTimer> timers = new(); // dispatcher timers are not kept alive by the queue (PointProbe's finding)
    private static FileSystemWatcher? watcher;
    private static DateTime lastRun;

    public static string TriggerFile => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "theuth", "companion", "anyapp-probe.txt");

    private static readonly Dictionary<string, string> Fixtures = new(StringComparer.OrdinalIgnoreCase)
    {
        ["short"] = "The deploy to us-east-1 is done.",
        ["mail"] = "Hi Dana, the deploy to us-east-1 is done. Next step is the dry run flag on deploy.sh.",
        ["multi"] = "First line of the note.\nSecond line, after a new line.",
        ["long"] = "Rebase the branch onto main, then force push with lease so nobody's work is lost; after that, open the pull request, " +
                   "ask for two reviews, and wait for the Windows build to go green before merging. Remember the clipboard should come back " +
                   "exactly as it was, and Win+V history must not show this paragraph.",
        ["secret"] = "hunter2 correct horse",
    };

    public static void Install(AnyAppController c, DispatcherQueue dispatcher)
    {
        var args = Environment.GetCommandLineArgs();
        string? Arg(string flag)
        {
            int i = Array.IndexOf(args, flag);
            return i >= 0 && i + 1 < args.Length && !args[i + 1].StartsWith("--", StringComparison.Ordinal) ? args[i + 1] : null;
        }
        bool Has(string flag) => args.Contains(flag);
        double delay = double.TryParse(Arg("--probe-anyapp-delay"), System.Globalization.NumberStyles.Float, System.Globalization.CultureInfo.InvariantCulture, out var d) ? d : 5;
        var once = new Dictionary<string, string>();
        if (Arg("--probe-anyapp") is { } text) once["text"] = text;
        if (Has("--probe-anyapp-lease")) once["lease"] = "1";
        if (Has("--probe-anyapp-send")) once["send"] = "1";
        if (Has("--probe-anyapp-lock")) once["lock"] = "1";
        if (Has("--probe-anyapp-card")) once["card"] = "1";
        if (Has("--probe-anyapp-history")) once["history"] = "1";
        if (Arg("--probe-anyapp-rewrite") is { } ins) once["rewrite"] = ins;
        if (once.Count > 0)
        {
            Log.Write($"anyapp-probe: armed — {string.Join(" ", once.Select(kv => $"{kv.Key}={kv.Value}"))} in {delay:0.#} s (focus the target now)");
            After(dispatcher, delay, () => _ = RunAsync(c, once));
        }
        if (Has("--probe-anyapp-watch"))
        {
            try
            {
                Directory.CreateDirectory(Path.GetDirectoryName(TriggerFile)!);
                watcher = new FileSystemWatcher(Path.GetDirectoryName(TriggerFile)!, Path.GetFileName(TriggerFile)) { NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.Size | NotifyFilters.FileName };
                FileSystemEventHandler on = (_, _) => dispatcher.TryEnqueue(() => OnTrigger(c, dispatcher));
                watcher.Changed += on;
                watcher.Created += on;
                watcher.EnableRaisingEvents = true;
                Log.Write($"anyapp-probe: watching {TriggerFile}");
            }
            catch (Exception e) { Log.Write($"anyapp-probe: watch failed — {e.Message}"); }
        }
    }

    private static void OnTrigger(AnyAppController c, DispatcherQueue dispatcher)
    {
        if ((DateTime.Now - lastRun).TotalMilliseconds < 800) return; // one write raises several events
        lastRun = DateTime.Now;
        string line;
        try { line = File.ReadAllText(TriggerFile).Trim(); }
        catch (IOException) { return; }
        var kv = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var part in line.Split(' ', StringSplitOptions.RemoveEmptyEntries))
        {
            var pair = part.Split('=', 2);
            if (pair.Length == 2) kv[pair[0]] = pair[1];
        }
        double delay = kv.TryGetValue("delay", out var ds) && double.TryParse(ds, System.Globalization.NumberStyles.Float, System.Globalization.CultureInfo.InvariantCulture, out var d) ? d : 0.5;
        Log.Write($"anyapp-probe: trigger \"{line}\" in {delay:0.#} s");
        After(dispatcher, delay, () => _ = RunAsync(c, kv));
    }

    private static void After(DispatcherQueue dispatcher, double seconds, Action a)
    {
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromSeconds(Math.Max(0.05, seconds));
        t.IsRepeating = false;
        t.Tick += (_, _) => { t.Stop(); timers.Remove(t); a(); };
        timers.Add(t);
        t.Start();
    }

    private static string Text(string spec) => Fixtures.TryGetValue(spec, out var f) ? f : spec.Replace('_', ' ').Replace("~n", "\n", StringComparison.Ordinal);

    private static async Task RunAsync(AnyAppController c, IReadOnlyDictionary<string, string> o)
    {
        try
        {
            nint fg = AnyAppNative.GetForegroundWindow();
            Log.Write($"anyapp-probe: run — foreground 0x{fg:X} \"{AnyAppNative.TitleOf(fg)}\" pid {AnyAppNative.ProcessOf(fg)} · locked={c.Locked}");
            if (o.ContainsKey("card")) { ShowFixtureCard(c); await PngAsync(o); return; }
            if (o.ContainsKey("history")) await HistoryAsync(c, null);
            bool lockProbe = o.TryGetValue("lock", out var lk) && lk == "1";
            if (lockProbe)
            {
                bool posted = c.Window.Synthesize(SessionLockGate.WtsSessionLock);
                Log.Write($"anyapp-probe: synthetic WM_WTSSESSION_CHANGE(WTS_SESSION_LOCK) posted to our window 0x{c.Window.Handle:X} → {posted}");
                await Task.Delay(300);
            }
            if (o.TryGetValue("rewrite", out var instruction)) { await RewriteAsync(c, Text(instruction)); await PngAsync(o); return; }
            if (!o.TryGetValue("text", out var spec)) return;
            var text = Text(spec);
            string? before = await c.Clipboard.TextAsync();
            Log.Write($"anyapp-probe: clipboard before = {Quote(before)}");
            bool started = await c.BeginAsync("probe", fixture: true);
            if (!started)
            {
                if (c.LastAssessment is { Refused: true } a && c.LastCapture is { } cap)
                {
                    // A refusal at the press records nothing; the probe's words exist, so they go to the card (kind noEditableTarget, §13.5).
                    c.ShowCard(new FailureCardModel(a.App, DeliveryFailure.NoEditableTarget, text, AnyAppCopy.Pill(a), CanRetry: false), cap, a);
                }
                Log.Write($"anyapp-probe: refused at the press — {(c.LastAssessment is { } la ? AnyAppCopy.Pill(la) : "busy/off")}");
            }
            else
            {
                if (o.TryGetValue("lease", out var ls) && ls == "1") c.ForceLease();
                bool send = o.TryGetValue("send", out var ss) && ss == "1";
                var pieces = Pieces(text);
                for (int i = 0; i < pieces.Count; i++)
                {
                    c.Feed(pieces[i], sendIt: send && i == pieces.Count - 1);
                    await Task.Delay(450);
                }
                await c.EndAsync("probe release");
            }
            await Task.Delay(700); // §16.1 #24: the clipboard is back within 600 ms of the last settle
            string? after = await c.Clipboard.TextAsync();
            Log.Write($"anyapp-probe: clipboard after = {Quote(after)} → {(after == before ? "RESTORED (equal to before)" : "DIFFERENT")}");
            if (o.ContainsKey("history") || o.ContainsKey("lease")) await HistoryAsync(c, text);
            if (lockProbe)
            {
                bool posted = c.Window.Synthesize(SessionLockGate.WtsSessionUnlock);
                Log.Write($"anyapp-probe: synthetic WM_WTSSESSION_CHANGE(WTS_SESSION_UNLOCK) → {posted}");
            }
            await PngAsync(o);
            Log.Write($"anyapp-probe: done — counters {c.Counters}");
        }
        catch (Exception e) { Log.Write($"anyapp-probe: failed — {e.GetType().Name}: {e.Message}"); }
    }

    /// <summary>The fixture as the engine would close it: one utterance per sentence (a newline stays inside its piece).</summary>
    private static List<string> Pieces(string text)
    {
        var parts = new List<string>();
        int start = 0;
        for (int i = 0; i < text.Length; i++)
        {
            if (text[i] is '.' or '?' or '!' && (i + 1 == text.Length || text[i + 1] == ' '))
            {
                parts.Add(text[start..(i + 1)].Trim());
                start = i + 1;
            }
        }
        if (start < text.Length && text[start..].Trim().Length > 0) parts.Add(text[start..].Trim());
        return parts;
    }

    private static void ShowFixtureCard(AnyAppController c)
    {
        var model = new FailureCardModel("Terminal", DeliveryFailure.PasteNotLanded, "git rebase -i HEAD~3 then force push with lease",
            AnyAppCopy.CardFooter(PasteVerifier.LeaseReadsMs, PasteVerifier.FieldState(PasteVerdict.NotLanded), clipboardRestored: true), CanRetry: false);
        c.ShowCard(model, null, null);
        Log.Write("anyapp-probe: fixture card shown (Terminal · paste not landed)");
    }

    private static async Task RewriteAsync(AnyAppController c, string instruction)
    {
        bool started = await c.BeginAsync("probe-rewrite", fixture: true);
        if (!started) { Log.Write("anyapp-probe: rewrite refused at the press"); return; }
        if (!c.InRewrite)
        {
            Log.Write("anyapp-probe: no selection in the focused field — not a rewrite; cancelling");
            c.Cancel("probe: no selection");
            return;
        }
        c.Feed(instruction);
        await c.EndAsync("probe rewrite release");
        Log.Write("anyapp-probe: rewrite requested");
    }

    /// <summary>Clipboard history (Win+V) through WinRT: is it on, and does any item hold the dictated text?</summary>
    private static async Task HistoryAsync(AnyAppController c, string? dictated)
    {
        try
        {
            bool on = global::Windows.ApplicationModel.DataTransfer.Clipboard.IsHistoryEnabled();
            var items = await global::Windows.ApplicationModel.DataTransfer.Clipboard.GetHistoryItemsAsync();
            Log.Write($"anyapp-probe: clipboard history enabled={on} status={items.Status} items={items.Items.Count}");
            int hits = 0, n = 0;
            foreach (var item in items.Items)
            {
                string t = "";
                try
                {
                    if (item.Content.Contains(global::Windows.ApplicationModel.DataTransfer.StandardDataFormats.Text)) t = await item.Content.GetTextAsync();
                }
                catch (Exception) { t = "(unreadable)"; }
                if (dictated is { Length: > 0 } && t.Contains(dictated.Split('.')[0], StringComparison.Ordinal)) hits++;
                if (n++ < 6) Log.Write($"anyapp-probe:   history[{n - 1}] {Quote(t)}");
            }
            if (dictated is not null) Log.Write($"anyapp-probe: history items holding the dictated text: {hits} {(hits == 0 ? "(excluded — the hints worked)" : "(LEAKED into history)")}");
        }
        catch (Exception e) { Log.Write($"anyapp-probe: clipboard history unavailable — {e.GetType().Name} 0x{e.HResult:X8} {e.Message}"); }
    }

    /// <summary><c>png=&lt;name&gt;</c>: the answer card's XAML rendered to <c>%TEMP%\153-anyapp-&lt;name&gt;.png</c> (W153C's
    /// <c>w153c-gate.ps1 pngs/b64</c> fetch it) — the card is excluded from capture unless <c>--shareable</c>; the render is not.</summary>
    private static async Task PngAsync(IReadOnlyDictionary<string, string> o)
    {
        if (!o.TryGetValue("png", out var name)) return;
        await Task.Delay(700); // the card's layout pass
        var path = Path.Combine(Path.GetTempPath(), $"153-anyapp-{name}.png");
        bool ok = Ui.AnswerCardWindow.Instance is { } w && await w.CapturePng(path);
        Log.Write($"anyapp-probe: card png {path} → {(ok ? "written" : "no card shown")}");
    }

    private static string Quote(string? s) => s is null ? "(none)" : "\"" + (s.Length > 60 ? s[..60] + "…" : s).Replace("\n", "\\n", StringComparison.Ordinal) + "\"";
}
