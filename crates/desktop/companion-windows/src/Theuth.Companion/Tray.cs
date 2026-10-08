using System.Runtime.InteropServices;
using Microsoft.Win32;
using Theuth.Companion.Interop;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.Shell;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion;

/// <summary>
/// The notification-area icon (design 134 D9, §4.5): <c>Shell_NotifyIcon</c> with <c>NOTIFYICON_VERSION_4</c> on the host
/// window (class <c>TheuthCompanionRail</c>), theuth's glyph with the gold <c>●N</c> badge drawn into pre-rendered sprites
/// (<c>Assets\tray\tray-&lt;dark|light&gt;-&lt;0…9|9plus&gt;.ico</c>, PNG entries 16–48 px, made by
/// <c>make-tray-sprites.swift</c>) chosen for the taskbar's light/dark setting, tooltip "theuth · 3 need you". Left click
/// (or Enter) → <see cref="Activated"/> (the panel above the icon); right click / the menu key → <see cref="MenuRequested"/>.
/// Re-added when Explorer restarts (<c>TaskbarCreated</c>); removed on quit.
/// </summary>
internal sealed class Tray : IDisposable
{
    private const uint Id = 1;
    private readonly HostWindow host;
    private bool added;
    private int count = -1;
    private bool lightTaskbar;
    private string iconFile = "";
    private HICON icon;
    private long lastActivate;
    private long lastMenu;

    public Tray(HostWindow host)
    {
        this.host = host;
        host.Tray += OnTray;
        host.TaskbarCreated += () =>
        {
            added = false;
            Add();
        };
        host.SystemChanged += msg =>
        {
            if (msg == PInvoke.WM_SETTINGCHANGE && LightTaskbar() != lightTaskbar) Update(Math.Max(0, count), force: true);
        };
    }

    /// <summary>Left click / Enter / Space on the icon, with the anchor point (screen px).</summary>
    public event Action<System.Drawing.Point>? Activated;

    /// <summary>Right click / Shift+F10 / the menu key.</summary>
    public event Action<System.Drawing.Point>? MenuRequested;

    public bool Added => added;

    /// <summary>When the first successful <c>NIM_ADD</c> happened (W6: the pin tip's clock starts here); null until then.</summary>
    public DateTimeOffset? AddedAt { get; private set; }

    public string Tooltip { get; private set; } = "theuth";

    public static string AssetsDir => Path.Combine(AppContext.BaseDirectory, "Assets", "tray");

    private NOTIFYICONDATAW Data(NOTIFY_ICON_DATA_FLAGS flags)
    {
        var d = new NOTIFYICONDATAW
        {
            cbSize = (uint)Marshal.SizeOf<NOTIFYICONDATAW>(),
            hWnd = host.Handle,
            uID = Id,
            uFlags = flags,
            uCallbackMessage = HostWindow.TrayMessage,
            hIcon = icon,
        };
        d.szTip = Tooltip;
        return d;
    }

    /// <summary>Adds the icon (NIM_ADD + NIM_SETVERSION 4) with the current count.</summary>
    public void Add()
    {
        if (added) return;
        LoadIcon(Math.Max(0, count));
        var d = Data(NOTIFY_ICON_DATA_FLAGS.NIF_MESSAGE | NOTIFY_ICON_DATA_FLAGS.NIF_ICON | NOTIFY_ICON_DATA_FLAGS.NIF_TIP | NOTIFY_ICON_DATA_FLAGS.NIF_SHOWTIP);
        added = PInvoke.Shell_NotifyIcon(NOTIFY_ICON_MESSAGE.NIM_ADD, in d);
        string addErr = Win32.LastError();
        if (added) AddedAt ??= DateTimeOffset.Now;
        d.uVersion = PInvoke.NOTIFYICON_VERSION_4;
        bool v4 = added && PInvoke.Shell_NotifyIcon(NOTIFY_ICON_MESSAGE.NIM_SETVERSION, in d);
        Log.Write($"tray: Shell_NotifyIcon(NIM_ADD) -> {(added ? "true" : $"false {addErr}")}, NIM_SETVERSION 4 -> {(v4 ? "true" : "false")}; icon {Path.GetFileName(iconFile)} tooltip \"{Tooltip}\"");
    }

    /// <summary>The needs-you count: the sprite (●N, 9+) and the tooltip.</summary>
    public void Update(int needsYou, bool force = false)
    {
        bool light = LightTaskbar();
        if (!force && needsYou == count && light == lightTaskbar) return;
        count = needsYou;
        Tooltip = needsYou == 0 ? "theuth · nothing needs you" : needsYou == 1 ? "theuth · 1 needs you" : $"theuth · {needsYou} need you";
        LoadIcon(needsYou);
        if (!added)
        {
            Add();
            return;
        }
        var d = Data(NOTIFY_ICON_DATA_FLAGS.NIF_ICON | NOTIFY_ICON_DATA_FLAGS.NIF_TIP | NOTIFY_ICON_DATA_FLAGS.NIF_SHOWTIP);
        bool ok = PInvoke.Shell_NotifyIcon(NOTIFY_ICON_MESSAGE.NIM_MODIFY, in d);
        Log.Write($"tray: ●{needsYou} -> {Path.GetFileName(iconFile)} \"{Tooltip}\" NIM_MODIFY -> {(ok ? "true" : $"false {Win32.LastError()}")}");
    }

    /// <summary>The icon's rectangle on screen (the overflow chevron's when the icon is in the overflow, R1); null if unknown.</summary>
    public RECT? IconRect()
    {
        if (!added) return null;
        var id = new NOTIFYICONIDENTIFIER { cbSize = (uint)Marshal.SizeOf<NOTIFYICONIDENTIFIER>(), hWnd = host.Handle, uID = Id };
        var hr = PInvoke.Shell_NotifyIconGetRect(in id, out var r);
        return hr.Succeeded ? r : null;
    }

    private void LoadIcon(int needsYou)
    {
        lightTaskbar = LightTaskbar();
        var name = $"tray-{(lightTaskbar ? "light" : "dark")}-{(needsYou <= 0 ? "0" : needsYou > 9 ? "9plus" : needsYou.ToString(System.Globalization.CultureInfo.InvariantCulture))}.ico";
        var file = Path.Combine(AssetsDir, name);
        uint dpi = PInvoke.GetDpiForWindow(host.Handle);
        int size = (int)Math.Round(16 * (dpi == 0 ? 96 : dpi) / 96.0);
        HICON next = default;
        unsafe
        {
            fixed (char* p = file)
            {
                var h = PInvoke.LoadImage(default, p, GDI_IMAGE_TYPE.IMAGE_ICON, size, size, IMAGE_FLAGS.LR_LOADFROMFILE);
                next = (HICON)(nint)h.Value;
            }
        }
        if (next.IsNull)
        {
            Log.Write($"tray: LoadImage({file}) failed {Win32.LastError()} — keeping the previous icon");
            return;
        }
        if (!icon.IsNull) PInvoke.DestroyIcon(icon);
        icon = next;
        iconFile = file;
    }

    /// <summary><c>HKCU\…\Themes\Personalize\SystemUsesLightTheme</c> = 1: a light taskbar (ink glyph); else white.</summary>
    private static bool LightTaskbar()
    {
        try
        {
            using var k = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
            return k?.GetValue("SystemUsesLightTheme") is int v && v == 1;
        }
        catch (Exception)
        {
            return false;
        }
    }

    private void OnTray(uint evt, System.Drawing.Point at)
    {
        long now = Environment.TickCount64;
        if (evt == PInvoke.NIN_SELECT || evt == Win32.NinKeySelect || evt == PInvoke.WM_LBUTTONUP)
        {
            if (now - lastActivate < 400) return; // version 4 can send both NIN_SELECT and WM_LBUTTONUP for one click
            lastActivate = now;
            Activated?.Invoke(at);
        }
        else if (evt == PInvoke.WM_CONTEXTMENU || evt == PInvoke.WM_RBUTTONUP)
        {
            if (now - lastMenu < 400) return;
            lastMenu = now;
            MenuRequested?.Invoke(at);
        }
    }

    public void Dispose()
    {
        if (added)
        {
            var d = Data(0);
            PInvoke.Shell_NotifyIcon(NOTIFY_ICON_MESSAGE.NIM_DELETE, in d);
            added = false;
        }
        if (!icon.IsNull)
        {
            PInvoke.DestroyIcon(icon);
            icon = default;
        }
    }
}
