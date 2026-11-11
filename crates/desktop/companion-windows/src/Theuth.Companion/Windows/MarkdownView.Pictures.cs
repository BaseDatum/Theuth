using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Theuth.Companion.Core;

namespace Theuth.Companion.Ui;

/// <summary>
/// 192w P1/P2: picture fences inside the streaming markdown view. A <see cref="MdBlock.Picture"/> is a host that shows, in turn:
/// the one line "drawing…" (the fence is still arriving, or its PNG is on the way — never the source), the PNG at content width once
/// <see cref="Pictures"/> has it (a click opens the reply in theuth), or — when no picture will come (no renderer, failed, timed out,
/// cancelled, an engine without <c>fence_snapshot</c>) — the fence as a code block with the caption "open theuth to see the diagram".
/// The host stays one element for the block's whole life, so a picture that arrives changes its own size only; a window that
/// follows its height (the Ask bar, the answer card) re-measures from the host's <c>SizeChanged</c>.
/// </summary>
internal sealed partial class MarkdownView
{
    /// <summary>The pictures of the replies on screen; null draws every picture fence as code (a view with no engine behind it).</summary>
    public PictureCache? Pictures { get; set; }

    /// <summary>The content width in effective pixels — what <c>fence_snapshot</c> is asked to draw at.</summary>
    public Func<double> PictureWidth { get; set; } = () => 400;

    /// <summary>The XamlRoot's rasterization scale (1 until the window is on screen). Named so because <c>UIElement.Scale</c> is a vector.</summary>
    public Func<double> RasterScale { get; set; } = () => 1;

    /// <summary>A picture or its "open theuth" caption was clicked: open the reply in theuth.</summary>
    public event Action? OpenRequested;

    /// <summary>A picture's draw ended (a PNG or a fallback replaced "drawing…").</summary>
    public event Action? PictureChanged;

    /// <summary>One element for the block's whole life; what it shows (the "drawing…" line, the image, the fallback) is its single child.</summary>
    private sealed class PictureHost : StackPanel
    {
        public MdBlock.Picture? Block;
        public IDisposable? Watch;

        public void Show(UIElement child)
        {
            Children.Clear();
            Children.Add(child);
        }
    }

    private PictureKey KeyFor(MdBlock.Picture p) =>
        PictureKey.Of(p.Lang, p.Source, PictureWidth(), Theme.Resolve(theme()) == ElementTheme.Light ? "light" : "dark", RasterScale());

    private FrameworkElement BuildPicture(MdBlock.Picture p)
    {
        var host = new PictureHost();
        AutomationProperties.SetAutomationId(host, "reply-picture");
        ShowPicture(host, p);
        return host;
    }

    private void ShowPicture(PictureHost host, MdBlock.Picture p)
    {
        host.Watch?.Dispose();
        host.Watch = null;
        host.Block = p;
        if (!p.Closed)
        {
            host.Show(DrawingLine());
            return;
        }
        if (Pictures is null)
        {
            host.Show(FallbackOf(p));
            return;
        }
        var key = KeyFor(p);
        var result = Pictures.Request(key);
        Apply(host, p, result);
        if (result.State == PictureState.Drawing)
        {
            host.Watch = Pictures.Watch(key, done =>
            {
                if (!ReferenceEquals(host.Block, p)) return; // the block moved on (its source grew, it was replaced)
                Apply(host, p, done);
                PictureChanged?.Invoke();
            });
        }
    }

    private void Apply(PictureHost host, MdBlock.Picture p, PictureResult result) =>
        host.Show(result.State switch
        {
            PictureState.Ready => ImageOf(p, result),
            PictureState.Fallback => FallbackOf(p),
            _ => DrawingLine(),
        });

    private void DisposeWatch(UIElement element)
    {
        if (element is not PictureHost host) return;
        host.Watch?.Dispose();
        host.Watch = null;
    }

    private void DisposeWatches()
    {
        foreach (var child in Children) DisposeWatch(child);
    }

    private TextBlock DrawingLine() => new()
    {
        Text = PictureFence.Drawing,
        FontSize = size - 1,
        Foreground = Theme.Brush(NativeLook.TextSecondary, theme()),
    };

    /// <summary>The fence as code, then the one line that says where to see it.</summary>
    private FrameworkElement FallbackOf(MdBlock.Picture p)
    {
        var panel = new StackPanel { Spacing = 4 };
        panel.Children.Add(CodeBox(p.Source));
        var caption = new TextBlock
        {
            Text = PictureFence.FallbackCaption,
            FontSize = size - 1.5,
            Foreground = Theme.Brush(NativeLook.AccentText, theme()),
            TextWrapping = TextWrapping.Wrap,
        };
        AutomationProperties.SetAutomationId(caption, "reply-picture-caption");
        caption.Tapped += (_, e) =>
        {
            e.Handled = true;
            OpenRequested?.Invoke();
        };
        panel.Children.Add(caption);
        return panel;
    }

    private FrameworkElement ImageOf(MdBlock.Picture p, PictureResult result)
    {
        double max = Math.Max(1, PictureWidth());
        var image = new Image
        {
            Source = PictureBitmaps.Of(result.Png!),
            Stretch = Stretch.Uniform,
            HorizontalAlignment = HorizontalAlignment.Left,
            MaxWidth = max,
        };
        if (result.PixelWidth > 0 && result.PixelHeight > 0)
        {
            // The PNG is (width × scale) device pixels: shown at width effective pixels it is pixel for pixel. Both sizes are set, so the
            // layout does not move when the bitmap finishes decoding.
            image.Width = Math.Min(max, result.PixelWidth / Math.Max(1, result.Scale));
            image.Height = image.Width * result.PixelHeight / result.PixelWidth;
        }
        else
        {
            image.Width = max;
        }
        AutomationProperties.SetAutomationId(image, "reply-picture-image");
        AutomationProperties.SetName(image, $"{p.Lang} diagram — click to open the reply in theuth");
        image.Tapped += (_, e) =>
        {
            e.Handled = true;
            OpenRequested?.Invoke();
        };
        return image;
    }
}

/// <summary>192w P2: the decoded bitmaps of the PNGs the cache holds — one <see cref="BitmapImage"/> per PNG however many views show it.</summary>
internal static class PictureBitmaps
{
    private static readonly System.Runtime.CompilerServices.ConditionalWeakTable<byte[], BitmapImage> Decoded = new();

    public static BitmapImage Of(byte[] png)
    {
        if (Decoded.TryGetValue(png, out var known)) return known;
        var bitmap = new BitmapImage();
        Decoded.Add(png, bitmap);
        _ = Load(bitmap, png);
        return bitmap;
    }

    private static async Task Load(BitmapImage bitmap, byte[] png)
    {
        try
        {
            using var stream = new global::Windows.Storage.Streams.InMemoryRandomAccessStream();
            using (var writer = new global::Windows.Storage.Streams.DataWriter(stream))
            {
                writer.WriteBytes(png);
                await writer.StoreAsync();
                writer.DetachStream();
            }
            stream.Seek(0);
            await bitmap.SetSourceAsync(stream);
        }
        catch (Exception e)
        {
            Log.Write($"picture: decoding {png.Length} bytes failed: {e.GetType().Name}: {e.Message}");
        }
    }
}