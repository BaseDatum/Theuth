using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace SpikeHost;

/// <summary>A normal window whose body is the SpikeLog TextBox, with an optional header element.</summary>
internal sealed partial class LogWindow : Window
{
    public LogWindow(string title, FrameworkElement? header = null, bool exitOnClose = true)
    {
        Title = title;
        Content = Ui.LogGrid(header);
        if (exitOnClose)
        {
            Closed += (s, e) =>
            {
                SpikeLog.Append($"log window \"{title}\" closed -> exit");
                Application.Current.Exit();
            };
        }
    }
}
