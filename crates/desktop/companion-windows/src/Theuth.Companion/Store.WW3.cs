namespace Theuth.Companion;

/// <summary>149 WW3's app-side Store members: the parked P screenshots re-render the panel like any other change.</summary>
public sealed partial class Store
{
    private bool ww3Installed;

    /// <summary>Once, from <c>WW3Install</c>: a changed attachment park raises <c>PropertyChanged("ReplyAttachments")</c> so the panel redraws.</summary>
    internal void InstallWw3()
    {
        if (ww3Installed) return;
        ww3Installed = true;
        ReplyAttachmentsChanged += _ => OnPropertyChanged("ReplyAttachments");
    }
}
