using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Theuth.Companion.Core;

namespace Theuth.Companion.Ui;

/// <summary>
/// 192w P3: the model's thinking above a reply, in the system's own controls and secondary text. While it is live
/// (<see cref="ThinkingView.Active"/> — reasoning came and no answer text yet) it is a "Thinking" line and ≈ 6 lines of the newest
/// reasoning that follow the tail, in place of the bare spinner; at the first answer text (or the finish) it folds into the one-line
/// expander "Thought for N s ›" whose click opens the full text (a scrolling box of at most <see cref="ExpandedMaxHeight"/> epx).
/// The user's open/closed choice for the expander is <see cref="Expanded"/>; a window that redraws everything (the answer card)
/// passes it back in. One UI thread, like <see cref="MarkdownView"/>.
/// </summary>
internal sealed class ThinkingSection : StackPanel
{
    /// <summary>One line of the body, so "six lines" is a height the layout knows before the text is measured.</summary>
    public const double LineHeight = 16;

    public const int FollowLines = 6;

    public const double ActiveMaxHeight = LineHeight * FollowLines;

    public const double ExpandedMaxHeight = 180;

    private readonly Func<ElementTheme> theme;
    private readonly FontIcon chevron;
    private readonly TextBlock header;
    private readonly TextBlock body;
    private readonly ScrollViewer bodyScroll;
    private bool active;
    private bool followTail = true;
    private double lastScrollable;
    private ThinkingView? last;

    public ThinkingSection(Func<ElementTheme> theme, bool expanded = false, double size = 11.5)
    {
        this.theme = theme;
        Expanded = expanded;
        Spacing = 2;
        Visibility = Visibility.Collapsed;
        chevron = new FontIcon { Glyph = "\uE76C", FontSize = 9, VerticalAlignment = VerticalAlignment.Center };
        header = new TextBlock { FontSize = size, VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetAutomationId(header, "thinking-header");
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        row.Children.Add(chevron);
        row.Children.Add(header);
        var toggle = new Button
        {
            Content = row, Padding = new Thickness(0, 2, 8, 2), MinWidth = 0, MinHeight = 0, BorderThickness = new Thickness(0),
            Background = Theme.Transparent, HorizontalAlignment = HorizontalAlignment.Left,
        };
        AutomationProperties.SetAutomationId(toggle, "thinking-toggle");
        toggle.Click += (_, _) =>
        {
            if (active) return; // live: the text is already open
            Expanded = !Expanded;
            Apply();
            Toggled?.Invoke(Expanded);
        };
        body = new TextBlock { FontSize = size, LineHeight = LineHeight, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
        AutomationProperties.SetAutomationId(body, "thinking-body");
        bodyScroll = new ScrollViewer
        {
            Content = body, Visibility = Visibility.Collapsed, MaxHeight = ActiveMaxHeight, Padding = new Thickness(15, 0, 8, 0),
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
        };
        body.SizeChanged += (_, _) =>
        {
            if (!active || !followTail) return;
            bodyScroll.UpdateLayout(); // the extent is measured after SizeChanged: scroll to where it ends now, not where it ended before this chunk
            lastScrollable = bodyScroll.ScrollableHeight;
            bodyScroll.ChangeView(null, bodyScroll.ScrollableHeight, null, disableAnimation: true);
        };
        bodyScroll.ViewChanged += (_, e) =>
        {
            if (e.IsIntermediate) return;
            if (Math.Abs(bodyScroll.ScrollableHeight - lastScrollable) > 0.5) // the text grew: not the user scrolling
            {
                lastScrollable = bodyScroll.ScrollableHeight;
                return;
            }
            followTail = bodyScroll.VerticalOffset >= bodyScroll.ScrollableHeight - 12; // scrolled up: stop following
        };
        Children.Add(toggle);
        Children.Add(bodyScroll);
        AutomationProperties.SetAutomationId(this, "thinking-section");
    }

    /// <summary>The expander is open (the user's choice; only matters once the thinking is over).</summary>
    public bool Expanded { get; private set; }

    /// <summary>The user opened or closed the expander (so a window that redraws everything remembers it).</summary>
    public event Action<bool>? Toggled;

    /// <summary>Show <paramref name="view"/>; null hides the section (no reasoning came: today's spinner).</summary>
    public void Set(ThinkingView? view)
    {
        last = view;
        if (view is null)
        {
            Reset();
            return;
        }
        if (!active && view.Active) followTail = true;
        active = view.Active;
        Visibility = Visibility.Visible;
        header.Text = view.Header;
        body.Text = active ? view.Tail : view.Text;
        Apply();
    }

    /// <summary>The system theme changed: the brushes are read again.</summary>
    public void Retheme() => Set(last);

    /// <summary>A new reply: nothing shown, the expander closed.</summary>
    public void Reset()
    {
        last = null;
        active = false;
        Expanded = false;
        followTail = true;
        body.Text = "";
        Visibility = Visibility.Collapsed;
        bodyScroll.Visibility = Visibility.Collapsed;
    }

    private void Apply()
    {
        var t = theme();
        var secondary = Theme.Brush(NativeLook.TextSecondary, t);
        header.Foreground = secondary;
        chevron.Foreground = secondary;
        body.Foreground = secondary;
        chevron.Visibility = active ? Visibility.Collapsed : Visibility.Visible;
        chevron.Glyph = Expanded ? "\uE70D" : "\uE76C";
        bool open = active || Expanded;
        bodyScroll.Visibility = open ? Visibility.Visible : Visibility.Collapsed;
        bodyScroll.MaxHeight = active ? ActiveMaxHeight : ExpandedMaxHeight;
        AutomationProperties.SetName(this, last is null ? "" : active ? "Thinking" : $"{last.Header}, {(Expanded ? "open" : "closed")}");
    }
}