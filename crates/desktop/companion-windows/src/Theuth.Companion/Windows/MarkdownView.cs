using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;

namespace Theuth.Companion.Ui;

/// <summary>
/// 189w D4: the Ask reply as markdown that updates as text arrives — headings, bullets (nested), numbered items, fenced code, table
/// rows (monospace) and paragraphs with **bold**, *italic*, `code` and [links](url), in the system's own Fluent controls
/// (<see cref="TextBlock"/> with <see cref="Run"/>s and <see cref="Hyperlink"/>s, theme brushes from <see cref="Theme"/>, no custom
/// theme). The library check found nothing that fits (see docs/design/189-as-built-WIN.md): the toolkit's MarkdownTextBlock pulls the
/// <c>Microsoft.WindowsAppSDK</c> 1.6 meta package this app deliberately avoids and rebuilds its whole tree on every Text change.
/// So the parse is <see cref="AnswerMarkdown"/> (Core, tested on the Mac) and the view keeps the elements it already drew: a chunk of
/// streamed text is diffed against the previous blocks (<see cref="MdDiff"/>) and only the block being written is touched — updated in
/// place, its last run's text grown when only that grew — never the whole answer. Streaming mode closes the unclosed marks of the last
/// block and holds back a bare marker line, so nothing flashes. A <see cref="StackPanel"/> like <c>ReplyField</c>: one UI thread.
/// 192w: picture fences (<c>json-render</c>, <c>mermaid</c>, <c>openui</c>) are blocks of their own — see MarkdownView.Pictures.cs.
/// </summary>
internal sealed partial class MarkdownView : StackPanel
{
    private static readonly FontFamily Mono = new("Cascadia Mono, Consolas");

    private readonly Func<ElementTheme> theme;
    private readonly double size;
    private readonly List<string> signatures = new();
    private readonly List<MdBlock> shown = new();
    private bool streaming;

    public MarkdownView(Func<ElementTheme> theme, double size = 13)
    {
        this.theme = theme;
        this.size = size;
        Spacing = 6;
    }

    /// <summary>The text last shown.</summary>
    public string Markdown { get; private set; } = "";

    /// <summary>How many times <see cref="Set"/> ran.</summary>
    public int Sets { get; private set; }

    /// <summary>The last <see cref="Set"/>: blocks left untouched, updated in place, built new — the probe's log proves a chunk does not rebuild the answer.</summary>
    public (int Kept, int Updated, int Built) LastDiff { get; private set; }

    public void Reset()
    {
        DisposeWatches();
        Children.Clear();
        signatures.Clear();
        shown.Clear();
        Markdown = "";
    }

    /// <summary>The system theme changed: draw everything again with the new brushes.</summary>
    public void Retheme()
    {
        var text = Markdown;
        Reset();
        if (text.Length > 0) Set(text, streaming);
    }

    /// <summary>Show <paramref name="markdown"/> — the whole text so far. <paramref name="streaming"/>: it is still arriving (unclosed marks shown as closed).</summary>
    public (int Kept, int Updated, int Built) Set(string markdown, bool streaming)
    {
        Markdown = markdown;
        this.streaming = streaming;
        Sets++;
        var blocks = AnswerMarkdown.Parse(markdown, streaming);
        var sigs = new List<string>(blocks.Count);
        foreach (var b in blocks) sigs.Add(MdDiff.Signature(b));
        int keep = MdDiff.CommonPrefix(signatures, sigs), updated = 0, built = 0;
        for (int i = keep; i < blocks.Count; i++)
        {
            var block = blocks[i];
            if (i < Children.Count && i < shown.Count && MdDiff.Kind(shown[i]) == MdDiff.Kind(block))
            {
                Update((FrameworkElement)Children[i], shown[i], block);
                updated++;
            }
            else
            {
                var element = Build(block);
                if (i < Children.Count)
                {
                    DisposeWatch(Children[i]);
                    Children[i] = element;
                }
                else Children.Add(element);
                built++;
            }
        }
        while (Children.Count > blocks.Count)
        {
            DisposeWatch(Children[^1]);
            Children.RemoveAt(Children.Count - 1);
        }
        signatures.Clear();
        signatures.AddRange(sigs);
        shown.Clear();
        shown.AddRange(blocks);
        LastDiff = (keep, updated, built);
        return LastDiff;
    }

    // MARK: building

    private TextBlock Text(double fontSize, global::Windows.UI.Text.FontWeight weight) => new()
    {
        FontSize = fontSize,
        FontWeight = weight,
        TextWrapping = TextWrapping.Wrap,
        IsTextSelectionEnabled = true,
        Foreground = Theme.Brush(NativeLook.TextPrimary, theme()),
    };

    private FrameworkElement Build(MdBlock block)
    {
        switch (block)
        {
            case MdBlock.Heading h:
            {
                var t = Text(HeadingSize(h.Level), FontWeights.SemiBold);
                Fill(t, h.Inlines);
                return t;
            }
            case MdBlock.Bullet b:
            {
                var grid = new Grid { ColumnSpacing = 6 };
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                var marker = new TextBlock { FontSize = size, Foreground = Theme.Brush(NativeLook.TextSecondary, theme()), VerticalAlignment = VerticalAlignment.Top };
                var body = Text(size, FontWeights.Normal);
                Grid.SetColumn(body, 1);
                grid.Children.Add(marker);
                grid.Children.Add(body);
                marker.Text = Marker(b);
                grid.Margin = BulletMargin(b);
                Fill(body, b.Inlines);
                return grid;
            }
            case MdBlock.Code c:
                return CodeBox(c.Text);
            case MdBlock.Picture g:
                return BuildPicture(g);
            default:
            {
                var t = Text(size, FontWeights.Normal);
                Fill(t, ((MdBlock.Paragraph)block).Inlines);
                return t;
            }
        }
    }

    /// <summary>A code block: monospace text on a control fill, scrolling sideways instead of wrapping.</summary>
    private Border CodeBox(string text)
    {
        var t = new TextBlock { FontFamily = Mono, FontSize = size - 0.5, IsTextSelectionEnabled = true, Foreground = Theme.Brush(NativeLook.TextPrimary, theme()), Text = text };
        return new Border
        {
            Background = Theme.Brush(NativeLook.ControlFill, theme()),
            CornerRadius = CardChrome.ControlRadius,
            Padding = new Thickness(8, 6, 8, 6),
            Child = new ScrollViewer { Content = t, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto, VerticalScrollBarVisibility = ScrollBarVisibility.Disabled, HorizontalScrollMode = ScrollMode.Enabled },
        };
    }

    /// <summary>The element of the same kind is kept; its content follows <paramref name="next"/> (the block being written).</summary>
    private void Update(FrameworkElement element, MdBlock before, MdBlock next)
    {
        switch (next)
        {
            case MdBlock.Heading h:
                ((TextBlock)element).FontSize = HeadingSize(h.Level);
                Refill((TextBlock)element, ((MdBlock.Heading)before).Inlines, h.Inlines);
                break;
            case MdBlock.Bullet b:
            {
                var grid = (Grid)element;
                ((TextBlock)grid.Children[0]).Text = Marker(b);
                grid.Margin = BulletMargin(b);
                Refill((TextBlock)grid.Children[1], ((MdBlock.Bullet)before).Inlines, b.Inlines);
                break;
            }
            case MdBlock.Code c:
                ((TextBlock)((ScrollViewer)((Border)element).Child).Content).Text = c.Text;
                break;
            case MdBlock.Picture g:
                ShowPicture((PictureHost)element, g);
                break;
            case MdBlock.Paragraph p:
                Refill((TextBlock)element, ((MdBlock.Paragraph)before).Inlines, p.Inlines);
                break;
        }
    }

    private double HeadingSize(int level) => level <= 2 ? size + 2 : size + 1;

    private static string Marker(MdBlock.Bullet b) => b.Numbered ? $"{b.Number}." : "•";

    private static Thickness BulletMargin(MdBlock.Bullet b) => new(6 + 14 * b.Indent, 0, 0, 0);

    // MARK: inlines

    private void Fill(TextBlock t, IReadOnlyList<MdInline> inlines)
    {
        foreach (var i in inlines)
        {
            switch (i)
            {
                case MdInline.Text x: t.Inlines.Add(new Run { Text = x.Value }); break;
                case MdInline.Bold x: t.Inlines.Add(new Run { Text = x.Value, FontWeight = FontWeights.SemiBold }); break;
                case MdInline.Italic x: t.Inlines.Add(new Run { Text = x.Value, FontStyle = global::Windows.UI.Text.FontStyle.Italic }); break;
                case MdInline.Code x: t.Inlines.Add(new Run { Text = x.Value, FontFamily = Mono }); break;
                case MdInline.Link x:
                {
                    if (Uri.TryCreate(x.Url, UriKind.Absolute, out var uri))
                    {
                        // The default hyperlink brush resolves against the app's theme, not this window's: dark blue on dark (seen on build-B).
                        var link = new Hyperlink { Foreground = Theme.Brush(NativeLook.AccentText, theme()) };
                        link.Inlines.Add(new Run { Text = x.Value });
                        link.Click += (_, _) => _ = Links.Open(uri);
                        t.Inlines.Add(link);
                    }
                    else t.Inlines.Add(new Run { Text = x.Value });
                    break;
                }
            }
        }
    }

    /// <summary>Same marks as before and only the last run's text grew: grow that run; otherwise the paragraph's runs are drawn again (still one block).</summary>
    private void Refill(TextBlock t, IReadOnlyList<MdInline> before, IReadOnlyList<MdInline> after)
    {
        if (!TryGrowLast(t, before, after))
        {
            t.Inlines.Clear();
            Fill(t, after);
        }
    }

    private static bool TryGrowLast(TextBlock t, IReadOnlyList<MdInline> before, IReadOnlyList<MdInline> after)
    {
        if (before.Count == 0 || before.Count != after.Count || t.Inlines.Count != before.Count) return false;
        for (int i = 0; i < before.Count - 1; i++)
        {
            if (!before[i].Equals(after[i])) return false;
        }
        string? grown = (before[^1], after[^1]) switch
        {
            (MdInline.Text a, MdInline.Text b) when b.Value.StartsWith(a.Value, StringComparison.Ordinal) => b.Value,
            (MdInline.Bold a, MdInline.Bold b) when b.Value.StartsWith(a.Value, StringComparison.Ordinal) => b.Value,
            (MdInline.Italic a, MdInline.Italic b) when b.Value.StartsWith(a.Value, StringComparison.Ordinal) => b.Value,
            (MdInline.Code a, MdInline.Code b) when b.Value.StartsWith(a.Value, StringComparison.Ordinal) => b.Value,
            _ => null,
        };
        if (grown is null || t.Inlines[t.Inlines.Count - 1] is not Run run) return false;
        run.Text = grown;
        return true;
    }
}