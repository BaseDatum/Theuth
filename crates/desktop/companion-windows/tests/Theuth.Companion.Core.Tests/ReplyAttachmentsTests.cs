namespace Theuth.Companion.Core.Tests;

// 149 WW3 R2: the parked P screenshots on a card's reply.
public sealed class ReplyAttachmentsTests
{
    [Fact]
    public void AttachTakeAndClear()
    {
        var clock = new ManualScheduler();
        var m = new StoreModel(new FakeEngine(), clock, () => clock.Now, _ => { });
        var changed = new List<string>();
        m.ReplyAttachmentsChanged += s => changed.Add(s);
        Assert.Empty(m.ReplyAttachmentsFor("s1"));
        Assert.Empty(m.ReplyAttachmentsFor(null));
        m.AttachToReply("s1", new[] { "a.png" });
        m.AttachToReply("s1", new[] { "b.png", "c.png" }); // a second P replaces
        Assert.Equal(new[] { "b.png", "c.png" }, m.ReplyAttachmentsFor("s1"));
        Assert.Equal("2 screenshots attached", StoreModel.AttachedText(m.ReplyAttachmentsFor("s1").Count));
        Assert.Equal(new[] { "b.png", "c.png" }, m.TakeReplyAttachments("s1"));
        Assert.Empty(m.ReplyAttachmentsFor("s1"));
        Assert.Empty(m.TakeReplyAttachments("s1"));
        m.AttachToReply("s2", new[] { "d.png" });
        m.ClearReplyAttachments("s2");
        Assert.Empty(m.ReplyAttachmentsFor("s2"));
        Assert.Equal(new[] { "s1", "s1", "s1", "s2", "s2" }, changed);
        Assert.Equal("1 screenshot attached", StoreModel.AttachedText(1));
        Assert.Null(StoreModel.AttachedText(0));
    }
}
