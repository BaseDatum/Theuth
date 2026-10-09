namespace Theuth.Companion.Core.Tests;

/// <summary>149 voice-key fix R3: the modifiers Esc is registered with while the voice key is held, and the Task Manager skip.</summary>
public sealed class EscDuringHoldTests
{
    [Fact]
    public void TheDefaultVoiceKeyGivesCtrlAltEsc()
    {
        var voice = new KeyTable().ChordFor(KeyAction.VoiceKey);
        Assert.Equal("Ctrl+Alt+Space", voice!.Symbol);
        Assert.Equal(ChordModifiers.Control | ChordModifiers.Alt, EscDuringHold.Modifiers(voice));
        Assert.Null(EscDuringHold.SkipReason(voice));
    }

    [Fact]
    public void CtrlShiftIsTaskManagerAndIsSkipped()
    {
        var voice = new Chord(" ", VirtualKeys.Space, ChordModifiers.Control | ChordModifiers.Shift);
        Assert.Null(EscDuringHold.Modifiers(voice));
        Assert.Contains("Task Manager", EscDuringHold.SkipReason(voice));
        // Ctrl+Shift+Alt is not Task Manager: registered.
        var three = new Chord(" ", VirtualKeys.Space, ChordModifiers.Control | ChordModifiers.Shift | ChordModifiers.Alt);
        Assert.Equal(ChordModifiers.Control | ChordModifiers.Shift | ChordModifiers.Alt, EscDuringHold.Modifiers(three));
    }

    [Fact]
    public void NoKeyOrNoModifiersMeansNothingExtra()
    {
        Assert.Null(EscDuringHold.Modifiers(null));
        Assert.Null(EscDuringHold.SkipReason(null));
        var bare = new Chord("s", 'S');
        Assert.Null(EscDuringHold.Modifiers(bare));
        Assert.Null(EscDuringHold.SkipReason(bare));
    }

    [Fact]
    public void AWinChordIsWindowsOwn()
    {
        var win = new Chord(" ", VirtualKeys.Space, ChordModifiers.Win);
        Assert.Null(EscDuringHold.Modifiers(win));
        Assert.Contains("Windows' own", EscDuringHold.SkipReason(win));
        var winAlt = new Chord(" ", VirtualKeys.Space, ChordModifiers.Win | ChordModifiers.Alt);
        Assert.Equal(ChordModifiers.Alt, EscDuringHold.Modifiers(winAlt)); // Win is dropped, Alt+Esc stays
    }
}
