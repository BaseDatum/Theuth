using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core.Tests;

/// <summary>153 wave C (proof 17): the §10.6 spoken-approval grammar — yes / go / no / near-miss / unknown.</summary>
public sealed class SpokenApprovalTests
{
    [Theory]
    [InlineData("yes")]
    [InlineData("Yes.")]
    [InlineData("yeah")]
    [InlineData("YEAH!")]
    [InlineData("approve")]
    [InlineData("yes please")]
    [InlineData("yes, do it")]
    public void Yes(string said) => Assert.Equal(Decision.Allow, SpokenApproval.Decide(said));

    [Theory]
    [InlineData("go")]
    [InlineData("Go ahead.")]
    [InlineData("go ahead now")]
    [InlineData("do it")]
    [InlineData("Do it!")]
    [InlineData("yeah go ahead")]
    public void Go(string said) => Assert.Equal(Decision.Allow, SpokenApproval.Decide(said));

    [Theory]
    [InlineData("no")]
    [InlineData("No.")]
    [InlineData("nope")]
    [InlineData("stop")]
    [InlineData("Cancel!")]
    [InlineData("deny")]
    [InlineData("no, stop")]
    [InlineData("no thanks")]
    public void No(string said) => Assert.Equal(Decision.Deny, SpokenApproval.Decide(said));

    [Fact]
    public void NearMiss()
    {
        // §7.1: phrases of ≥ 5 chars tolerate max(1, len/5) edits.
        Assert.Equal(5, SpokenApproval.NearMissMinLen);
        Assert.Equal(1, SpokenApproval.Tolerance("approve"));
        Assert.Equal(1, SpokenApproval.Tolerance("go ahead"));
        Assert.Equal(1, SpokenApproval.Tolerance("do it"));
        Assert.Equal(0, SpokenApproval.Tolerance("yes"));
        Assert.Equal(0, SpokenApproval.Tolerance("nope"));
        Assert.Equal(Decision.Allow, SpokenApproval.Decide("approved"));
        Assert.Equal(Decision.Allow, SpokenApproval.Decide("go ahed"));
        Assert.Equal(Decision.Allow, SpokenApproval.Decide("do itt"));
        Assert.Equal(Decision.Deny, SpokenApproval.Decide("cancels"));
        Assert.Equal(Decision.Deny, SpokenApproval.Decide("cancal"));
        // Short phrases need an exact match.
        Assert.Null(SpokenApproval.Decide("yess"));
        Assert.Null(SpokenApproval.Decide("nop"));
        Assert.Null(SpokenApproval.Decide("stopp"));
        Assert.Null(SpokenApproval.Decide("gone"));
        // Beyond tolerance.
        Assert.Null(SpokenApproval.Decide("appreciate"));
        // A near miss never beats a word of the other polarity ("no it" is one edit from "do it").
        Assert.Null(SpokenApproval.Decide("no it"));
    }

    [Theory]
    [InlineData("")]
    [InlineData("   ")]
    [InlineData("maybe")]
    [InlineData("what does it do")]
    [InlineData("okay")]
    [InlineData("sure")]
    [InlineData("don't do it")]
    [InlineData("no, go ahead")]
    [InlineData("yes but stop")]
    [InlineData("please")]
    [InlineData("do not")]
    public void Unknown(string said) => Assert.Null(SpokenApproval.Decide(said));

    [Fact]
    public void ListenWindowClosesOnAMatchOrAfterEightSeconds()
    {
        Assert.Equal(8, SpokenApproval.ListenSeconds);
        var l = new ApprovalListen("ap1", 1000);
        Assert.Null(l.Final("what is that", 2000));
        Assert.Equal(SpokenApproval.DidntCatch, l.Line);
        Assert.False(l.Closed);
        Assert.Equal(Decision.Allow, l.Final("yes", 3000));
        Assert.True(l.Closed);
        Assert.Null(l.Line);
        Assert.Null(l.Final("no", 3500)); // closed: later words are ignored
        var late = new ApprovalListen("ap2", 1000);
        Assert.True(late.Expired(9000));
        Assert.False(late.Expired(8999));
        Assert.Null(late.Final("yes", 9000));
        Assert.True(late.Closed);
        Assert.Null(late.Decided);
    }
}
