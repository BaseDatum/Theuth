using System.Text.Json;
using System.Text.Json.Serialization;

namespace Theuth.Companion.Rpc;

// Design 153 W153B — THE PIN: hand-written mirrors of the wave-A Rust types the companion sends or receives (153 §14.1,
// §14.2, §14.4). The Rust source is the truth and each type names it: crates/voice/src/{lib.rs, shape/mod.rs,
// dictionary/mod.rs, history/mod.rs, dictation.rs} and crates/agent/src/lib.rs (integrate/153 b7ec00b9). The wire is
// serde snake_case. Rules, as Models.cs / Models.149.cs: every field serde marks `#[serde(default)]` or `Option` is
// optional here, so an older engine still decodes; unknown keys are ignored. Three kinds of enum-valued fields:
//   1. shapes the companion SENDS (DictationOpts, Destination, AppProfile, Preset, SuggestionAnswer, EndPhrases): C# enums
//      and records written with the shared snake_case converter (or the spelled-out Word() for positional params);
//   2. enum-valued fields the companion RECEIVES (events, replies): tolerant — an unknown word (a newer engine's variant)
//      reads as null through TolerantWordConverter, never throws (149 §10's rule);
//   3. shapes that round-trip (Entry, Scope, Variant: dictionary_list → edit → dictionary_put): the wire word is
//      kept as a string, so a word this build does not know survives the round trip unchanged.
// C# type names follow the Swift mirrors (H3-CC, TheuthKit/Models.swift `// MARK: 153 — voice`) except: voice::shape::Action →
// ShapeAction (System.Action would be ambiguous in every file), voice::ModelState → VoiceModelState; EntryKind / EntrySource are
// the wire words as string constants (Entry keeps them as strings so unknown words round-trip). EntryId, SuggestionId and
// DictationId are serde-transparent strings (uuid text for the last two) and stay plain strings.

// MARK: enums (voice::Surface · shape::Preset · history::SuggestionAnswer · DeviceState · shape::Action)

/// <summary><c>voice::Surface</c> (153 §4.5): which client surface a dictation feeds. <c>None</c> in <see cref="DictationOpts"/> = <c>composer</c>.
/// Wire: <c>composer</c> · <c>tui</c> · <c>companion</c> · <c>phone</c> · <c>any_app</c>.</summary>
public enum Surface { Composer, Tui, Companion, Phone, AnyApp }

/// <summary><c>voice::shape::Preset</c> (153 §4.5; Swift <c>ShapePreset</c>), <c>voice.shape.&lt;surface&gt;</c>: <c>full</c> · <c>plain</c> (punctuation + literals + numbers, no
/// dictionary / end commands) · <c>off</c> (raw model text).</summary>
public enum ShapePreset { Full, Plain, Off }

/// <summary><c>voice::history::SuggestionAnswer</c> (153 §6.3 step 4): <c>add</c> · <c>not_now</c> (7-day cooldown) · <c>never</c> (a negative).</summary>
public enum SuggestionAnswer { Add, NotNow, Never }

/// <summary><c>voice::DeviceState</c> (153 §8.2): <c>changed</c> (re-armed on <c>device</c>) · <c>lost</c> (no device could be opened).</summary>
public enum DeviceState { Changed, Lost }

/// <summary><c>voice::shape::Action</c> (W-j): <c>new_line</c> · <c>new_paragraph</c> · <c>tab</c> — already rendered into <c>shaped</c> as
/// <c>\n</c> / <c>\n\n</c> / <c>\t</c> by the engine; the list says which ones ran. Named ShapeAction: <c>Action</c> would shadow System.Action.</summary>
public enum ShapeAction { NewLine, NewParagraph, Tab }

/// <summary>The 153 enums' wire words, spelled out (as WireWords149) so a C# rename cannot drift the wire.</summary>
public static class WireWords153
{
    /// <summary><c>composer</c> · <c>tui</c> · <c>companion</c> · <c>phone</c> · <c>any_app</c>.</summary>
    public static string Word(this Surface s) => s switch
    {
        Surface.Composer => "composer",
        Surface.Tui => "tui",
        Surface.Companion => "companion",
        Surface.Phone => "phone",
        _ => "any_app",
    };

    /// <summary><c>full</c> · <c>plain</c> · <c>off</c>.</summary>
    public static string Word(this ShapePreset p) => p switch { ShapePreset.Plain => "plain", ShapePreset.Off => "off", _ => "full" };

    /// <summary><c>add</c> · <c>not_now</c> · <c>never</c>.</summary>
    public static string Word(this SuggestionAnswer a) => a switch { SuggestionAnswer.NotNow => "not_now", SuggestionAnswer.Never => "never", _ => "add" };

    /// <summary>The surface a wire word names; null for an unknown word (a newer engine's surface) or null.</summary>
    public static Surface? ParseSurface(string? word) => TolerantWordConverter<Surface>.Parse(word);

    /// <summary>The shaping preset a wire word names (<c>full</c> · <c>plain</c> · <c>off</c>); null for an unknown word or null.</summary>
    public static ShapePreset? ParsePreset(string? word) => TolerantWordConverter<ShapePreset>.Parse(word);

    /// <summary>The shaping action a wire word names; null for an unknown word.</summary>
    public static ShapeAction? ParseAction(string? word) => TolerantWordConverter<ShapeAction>.Parse(word);
}

/// <summary>
/// A snake_case wire word ↔ a C# enum for fields the companion RECEIVES: an unknown word, a non-string token or a JSON
/// <c>null</c> reads as <c>null</c> instead of throwing (149 §10's tolerance rule); writes the word, or <c>null</c>.
/// </summary>
public sealed class TolerantWordConverter<T> : JsonConverter<T?> where T : struct, Enum
{
    private static readonly Dictionary<string, T> ByWord =
        Enum.GetValues<T>().ToDictionary(v => JsonNamingPolicy.SnakeCaseLower.ConvertName(v.ToString()), v => v, StringComparer.Ordinal);

    private static readonly Dictionary<T, string> ByValue = ByWord.ToDictionary(kv => kv.Value, kv => kv.Key);

    public override bool HandleNull => true;

    /// <summary>The value <paramref name="word"/> names, or null.</summary>
    public static T? Parse(string? word) => word is not null && ByWord.TryGetValue(word, out var v) ? v : null;

    /// <summary>The snake_case word of <paramref name="value"/>.</summary>
    public static string Word(T value) => ByValue[value];

    public override T? Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        if (reader.TokenType == JsonTokenType.String) return Parse(reader.GetString());
        reader.Skip(); // an object / array where a word was expected: consume it, read as unknown
        return null;
    }

    public override void Write(Utf8JsonWriter writer, T? value, JsonSerializerOptions options)
    {
        if (value is { } v) writer.WriteStringValue(Word(v));
        else writer.WriteNullValue();
    }
}

// MARK: dictation_start's options (voice/src/lib.rs: Destination, DictationOpts, AppProfile)

/// <summary>
/// <c>voice::Destination</c> (153 §9.1), <c>#[serde(tag = "kind", rename_all = "snake_case")]</c>: <c>{"kind":"agent_prompt"}</c> ·
/// <c>{"kind":"document"}</c> · <c>{"kind":"message_reply"}</c> · <c>{"kind":"any_app","app":"&lt;bundle id / exe&gt;"}</c>. Who reads
/// the dictated text — decides whether polish may run (<c>agent_prompt</c>: never).
/// </summary>
public sealed record Destination
{
    public string Kind { get; init; } = "agent_prompt";

    /// <summary><c>any_app</c> only: the bundle id (macOS) or exe name (Windows); omitted otherwise.</summary>
    public string? App { get; init; }

    /// <summary>The agent reads it (Ask bar, row mic, voice key, command): never polished.</summary>
    public static Destination AgentPrompt { get; } = new() { Kind = "agent_prompt" };

    public static Destination Document { get; } = new() { Kind = "document" };

    public static Destination MessageReply { get; } = new() { Kind = "message_reply" };

    /// <summary>Another app (153 §13), named by exe on Windows.</summary>
    public static Destination AnyApp(string app) => new() { Kind = "any_app", App = app };
}

/// <summary>
/// <c>voice::AppProfile</c> (153 §13): a companion-side per-app row sent in <see cref="DictationOpts.App"/> (any-app shaping) and
/// <c>rewrite</c>'s <c>app</c>. Rust gives no serde default except the two prompts, so every field is always written
/// (<see cref="Commands"/> as <c>[]</c> when empty). The engine never persists it; the companion's settings store does.
/// </summary>
public sealed record AppProfile
{
    /// <summary>Bundle id (macOS) or exe name (Windows), e.g. <c>WindowsTerminal.exe</c>.</summary>
    public string Id { get; init; } = "";

    /// <summary>Display name.</summary>
    public string Name { get; init; } = "";

    /// <summary>A terminal: trailing-space / autocomplete spacing rules apply (§4.2 stage 9).</summary>
    public bool Terminal { get; init; }

    /// <summary>Polish may run for this app (§9.1).</summary>
    public bool Polish { get; init; }

    /// <summary>Observe the target for corrections after a paste (§6.6).</summary>
    public bool Learn { get; init; }

    public string? PolishPrompt { get; init; }

    public string? RewritePrompt { get; init; }

    /// <summary>Slash commands this app accepts.</summary>
    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> Commands { get; init; } = new();
}

/// <summary>
/// <c>voice::DictationOpts</c> (153 §14.1), <c>#[serde(default)]</c>: <c>dictation_start</c>'s trailing optional. Null fields are
/// omitted on the wire; <c>None</c> opts (or an engine before 153) = today's Composer dictation. The companion's own fields send
/// <c>{surface: companion, destination: agent_prompt, press_ago_ms}</c> (153 §4.5, §8.5, §8.7).
/// </summary>
public sealed record DictationOpts
{
    /// <summary>Picks the shaping preset <c>voice.shape.&lt;surface&gt;</c>; null = <c>composer</c>.</summary>
    public Surface? Surface { get; init; }

    /// <summary>Who reads the text; null = <c>agent_prompt</c>.</summary>
    public Destination? Destination { get; init; }

    /// <summary>§13's per-app row for shaping (terminal spacing, commands).</summary>
    public AppProfile? App { get; init; }

    /// <summary>Slash commands the surface accepts (shaping stage 4); empty = none recognised.</summary>
    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> Commands { get; init; } = new();

    /// <summary>§8.7: <c>now − press</c> on the companion's clock at the moment of the call (ms).</summary>
    public uint? PressAgoMs { get; init; }
}

// MARK: dictation_status / voice_status (voice/src/dictation.rs DictationState · lib.rs VoiceStatus, DictationTimings, InputDevice)

/// <summary>
/// <c>voice::DictationTimings</c> (153 §8.7): one dictation's latency relative to the key press — <c>dictation_status.last</c>, the
/// Settings › Dictation "Last dictation" line. Every number optional (a stage that never happened).
/// </summary>
public sealed record DictationTimings
{
    [JsonConverter(typeof(TolerantWordConverter<Surface>))]
    public Surface? Surface { get; init; }

    public uint? PressToFirstPcmMs { get; init; }
    public uint? PressToFirstPartialMs { get; init; }
    public uint? PressToFirstFinalMs { get; init; }
    public uint? StopToFinalMs { get; init; }
    public uint? PolishMs { get; init; }

    /// <summary>The input device that opened.</summary>
    public string? Device { get; init; }
}

/// <summary><c>voice::ModelState</c>, <c>#[serde(tag = "state", rename_all = "snake_case")]</c>: <c>missing</c> · <c>loading</c> · <c>ready</c> ·
/// <c>failed</c> (+ <c>message</c>), kept as the word.</summary>
public sealed record VoiceModelState
{
    public string State { get; init; } = "";
    public string? Message { get; init; }
}

/// <summary><c>dictation_status()</c> → <c>voice::DictationState</c>: the streaming model, the running dictation and (153, serde default)
/// the input device it opened plus the last finished dictation's timings.</summary>
public sealed record DictationState
{
    public VoiceModelState? Model { get; init; }
    public string? Active { get; init; }
    public string? InputDevice { get; init; }
    public DictationTimings? Last { get; init; }
}

/// <summary><c>voice_status()</c> → <c>voice::VoiceStatus</c>; 153 adds <c>input_devices</c> (§8.1), the ordered mic preference the
/// companion's own capture follows too.</summary>
public sealed record VoiceStatus
{
    public VoiceModelState? Model { get; init; }
    public string Accelerator { get; init; } = "";
    public string Voice { get; init; } = "";

    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> Voices { get; init; } = new();

    public string? Speaking { get; init; }

    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> Queued { get; init; } = new();

    /// <summary><c>voice.input_devices</c>: ordered; empty = the system default only.</summary>
    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> InputDevices { get; init; } = new();
}

/// <summary><c>list_input_devices()</c> row (<c>voice::InputDevice</c>, 153 §8.1): the host's name for the mic and whether it is the default.</summary>
public sealed record InputDevice
{
    [JsonRequired] public string Name { get; init; } = "";
    public bool Default { get; init; }
}

// MARK: shaping (voice/src/shape/mod.rs EndCommand, EndPhrases)

/// <summary>
/// <c>voice::shape::EndCommand</c> (153 §7), <c>#[serde(tag = "command", rename_all = "snake_case")]</c>:
/// <c>{"command":"send","near_miss":false}</c> · <c>{"command":"scratch_that"}</c>. Carried by <c>DictationText.command</c>; sticky for
/// the utterance once armed. The surface acts on <c>send</c> after <c>DictationFinished</c> (§7.3); the engine executes
/// <c>scratch_that</c> itself (the previous utterance is re-sent empty). An unknown command word decodes as itself.
/// </summary>
public sealed record EndCommand
{
    public const string SendWord = "send";
    public const string ScratchThatWord = "scratch_that";

    public string Command { get; init; } = "";

    /// <summary><c>send</c> only: accepted by the §7.1 near-miss tolerance at the final, not the exact phrase.</summary>
    public bool NearMiss { get; init; }

    [JsonIgnore] public bool IsSend => Command == SendWord;

    [JsonIgnore] public bool IsScratchThat => Command == ScratchThatWord;

    public static EndCommand Send(bool nearMiss = false) => new() { Command = SendWord, NearMiss = nearMiss };

    public static EndCommand ScratchThat { get; } = new() { Command = ScratchThatWord };
}

/// <summary><c>voice::shape::EndPhrases</c> (153 §7.2), <c>#[serde(default)]</c>: <c>set_end_phrases</c>'s argument; the defaults are the
/// engine's ("send it", "scratch that", "literal").</summary>
public sealed record EndPhrases
{
    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> Send { get; init; } = ValueList.Of("send it");

    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> Scratch { get; init; } = ValueList.Of("scratch that");

    /// <summary>The escape word: the next phrase or symbol word is written as said.</summary>
    public string Literal { get; init; } = "literal";
}

// MARK: events (VoiceEvent::DictationDevice · VoiceEvent::DictionarySuggestion; DictationText's new fields are on DictationEvent)

/// <summary>
/// <c>VoiceEvent::DictationDevice</c> (153 §8.2): <c>{"type":"DictationDevice","id":…,"device":"…"|null,"state":"changed"|"lost"}</c> —
/// the mic changed mid-dictation (re-armed on <c>device</c>) or was lost; the field shows "mic changed → ‹name›" instead of ending.
/// </summary>
public sealed record DictationDevice
{
    public string? Id { get; init; }

    /// <summary>The device now open; null when lost.</summary>
    public string? Device { get; init; }

    [JsonConverter(typeof(TolerantWordConverter<DeviceState>))]
    public DeviceState? State { get; init; }

    public static DictationDevice Decode(ReadOnlySpan<byte> utf8) => TheuthJson.Deserialize<DictationDevice>(utf8);
}

/// <summary>
/// <c>VoiceEvent::DictionarySuggestion</c> (153 §6.3 step 3): a correction pair is due — the surface that produced the last
/// occurrence shows the suggestion card (frame D) and answers with <c>dictionary_suggestion_answer(id, …)</c>.
/// </summary>
public sealed record DictionarySuggestion
{
    /// <summary><c>SuggestionId</c> (uuid v7 text).</summary>
    public string Id { get; init; } = "";

    /// <summary>What the model heard.</summary>
    public string Heard { get; init; } = "";

    /// <summary>What the user typed instead.</summary>
    public string Typed { get; init; } = "";

    /// <summary>How often the pair was seen (7 days).</summary>
    public uint Occurrences { get; init; }

    /// <summary>Where it was dictated; null for a surface this build does not know.</summary>
    [JsonConverter(typeof(TolerantWordConverter<Surface>))]
    public Surface? Surface { get; init; }

    /// <summary>The utterance's audio is on disk (<c>dictation_audio_get</c> works: "▶ play what you said").</summary>
    public bool HasAudio { get; init; }

    public static DictionarySuggestion Decode(ReadOnlySpan<byte> utf8) => TheuthJson.Deserialize<DictionarySuggestion>(utf8);
}

// MARK: history (voice/src/history/mod.rs HistoryRow) and polish (agent/src/lib.rs PolishReply, ModelInfo)

/// <summary>
/// One <c>dictation_history(limit)</c> row (<c>voice::history::HistoryRow</c>, 153 §6.5/§6.7): newest first. <c>destination</c> is the
/// stored word (<c>agent_prompt</c> · <c>document</c> · <c>message_reply</c> · <c>any_app:&lt;exe&gt;</c>), not a <see cref="Rpc.Destination"/>.
/// </summary>
public sealed record HistoryRow
{
    [JsonRequired] public string Id { get; init; } = "";

    /// <summary>Unix ms.</summary>
    public ulong StartedMs { get; init; }

    /// <summary>Unix ms; null while running.</summary>
    public ulong? FinishedMs { get; init; }

    [JsonConverter(typeof(TolerantWordConverter<Surface>))]
    public Surface? Surface { get; init; }

    public string Destination { get; init; } = "";
    public string? App { get; init; }

    /// <summary>Finals joined.</summary>
    public string Raw { get; init; } = "";

    /// <summary>Finals joined, shaped.</summary>
    public string Shaped { get; init; } = "";

    /// <summary>Null unless polish ran (§9).</summary>
    public string? Polished { get; init; }

    /// <summary>What actually went out (<c>polished ?? shaped</c>) after end-command edits.</summary>
    public string? Delivered { get; init; }

    public uint? PressMs { get; init; }
    public uint? FirstPartialMs { get; init; }
    public uint? FirstFinalMs { get; init; }
    public uint? FinalMs { get; init; }

    /// <summary><c>done</c> · <c>stopped</c> · <c>error:&lt;message&gt;</c>.</summary>
    public string Reason { get; init; } = "";

    public ulong AudioBytes { get; init; }

    /// <summary>Audio is on disk (<c>dictation_audio_get</c> works).</summary>
    public bool HasAudio { get; init; }
}

/// <summary>
/// <c>polish</c> / <c>rewrite</c> → <c>agent::PolishReply</c> (153 §9, §13.6): fails closed — <c>text: null</c> + <c>refused</c> means deliver
/// the shaped text (or, for rewrite, leave the selection).
/// </summary>
public sealed record PolishReply
{
    /// <summary>The polished / rewritten text when the call succeeded.</summary>
    public string? Text { get; init; }

    /// <summary>The model that answered.</summary>
    public string? Model { get; init; }

    /// <summary>Round trip in ms.</summary>
    public uint Ms { get; init; }

    /// <summary>Why nothing was polished.</summary>
    public string? Refused { get; init; }
}

/// <summary><c>polish_model()</c> / <c>set_polish_model(model)</c> → <c>Option&lt;agent::ModelInfo&gt;</c> (as <c>title_model</c>): what the
/// Settings row shows — id / name / provider only, as the Swift mirror (the rest of the catalog row is ignored).</summary>
public sealed record ModelInfo
{
    /// <summary><c>provider/model</c>.</summary>
    public string Id { get; init; } = "";

    public string Name { get; init; } = "";
    public string Provider { get; init; } = "";
}

// MARK: dictionary (voice/src/dictionary/mod.rs Entry, Scope, Variant; Kind and Source as their words)

/// <summary><c>voice::dictionary::Kind</c> wire words (kept as strings on <see cref="Entry.Kind"/> so unknown words round-trip).</summary>
public static class EntryKind
{
    public const string Term = "term";
    public const string Symbol = "symbol";
    public const string Formatter = "formatter";
    public const string Abbrev = "abbrev";
    public const string Mention = "mention";
    public const string Command = "command";
    public const string Path = "path";
}

/// <summary><c>voice::dictionary::Source</c> wire words: <c>packaged</c> (shipped) · <c>user</c> (Settings / <c>dictionary_put</c>) · <c>learned</c> (§6.3 Add).</summary>
public static class EntrySource
{
    public const string Packaged = "packaged";
    public const string User = "user";
    public const string Learned = "learned";
}

/// <summary>
/// <c>voice::dictionary::Scope</c>, <c>#[serde(tag = "scope", rename_all = "snake_case")]</c> with named fields (153-C):
/// <c>{"scope":"global"}</c> · <c>{"scope":"surface","surface":"companion"}</c> · <c>{"scope":"app","app":"Code.exe"}</c>. Also
/// <c>dictionary_list</c>'s optional filter. The tag is <see cref="Kind"/> here (a C# member cannot share its type's name).
/// </summary>
public sealed record Scope
{
    /// <summary>The tag word: <c>global</c> · <c>surface</c> · <c>app</c>.</summary>
    [JsonPropertyName("scope")]
    public string Kind { get; init; } = "global";

    /// <summary><c>surface</c> scope only: the surface word.</summary>
    [JsonPropertyName("surface")]
    public string? SurfaceWord { get; init; }

    /// <summary><c>app</c> scope only: <c>AppProfile.id</c>.</summary>
    public string? App { get; init; }

    public static Scope Global { get; } = new();

    public static Scope ForSurface(Surface surface) => new() { Kind = "surface", SurfaceWord = surface.Word() };

    public static Scope ForApp(string app) => new() { Kind = "app", App = app };
}

/// <summary><c>voice::dictionary::Variant</c>: one heard form with its Double Metaphone pair (Rust <c>(String, String)</c>, a
/// two-element array; send <c>["",""]</c> and the engine computes it) and usage counts.</summary>
public sealed record Variant
{
    public string Heard { get; init; } = "";

    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> Metaphone { get; init; } = ValueList.Of("", "");

    public uint Seen { get; init; }
    public ulong LastMs { get; init; }
}

/// <summary>
/// <c>voice::dictionary::Entry</c> (153 §5.3): <c>dictionary_list</c> rows and <c>dictionary_put</c>'s argument and reply. Rust has no
/// serde default here, so every field is written on put. A new user entry goes out with <c>id: ""</c> (the engine assigns a
/// uuid) and is upserted by canonical + scope.
/// </summary>
public sealed record Entry
{
    /// <summary><c>EntryId</c> (a transparent string); "" for a new entry.</summary>
    public string Id { get; init; } = "";

    /// <summary>The written form; its casing wins.</summary>
    public string Canonical { get; init; } = "";

    /// <summary>A <see cref="EntryKind"/> word.</summary>
    public string Kind { get; init; } = EntryKind.Term;

    /// <summary>Spoken forms (exact lookups).</summary>
    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> Spoken { get; init; } = new();

    [JsonConverter(typeof(NullAsEmptyListConverter<Variant>))]
    public ValueList<Variant> Variants { get; init; } = new();

    public Scope Scope { get; init; } = Scope.Global;

    /// <summary>An <see cref="EntrySource"/> word.</summary>
    public string Source { get; init; } = EntrySource.User;

    /// <summary><c>tld</c>, <c>ext</c>, <c>mention</c>, <c>path</c>, <c>terminal</c>, <c>keep_case</c> …</summary>
    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> Tags { get; init; } = new();
}

// MARK: settings (153 F1, main bfafefeb: voice_settings → voice/src/lib.rs VoiceSettings · shape/mod.rs ShapePresets · HistoryConfig · Duck)

/// <summary><c>voice::shape::ShapePresets</c> (153 §4.5): <c>voice.shape</c>, one preset per surface, <c>#[serde(default)]</c> (every
/// default <c>full</c>). Received, so each preset stays its wire word (a newer engine's word must not throw); <see cref="Preset"/> reads one.</summary>
public sealed record ShapePresets
{
    public string Composer { get; init; } = "full";
    public string Tui { get; init; } = "full";
    public string Companion { get; init; } = "full";
    public string Phone { get; init; } = "full";
    public string AnyApp { get; init; } = "full";

    /// <summary>The wire word stored for <paramref name="surface"/>.</summary>
    public string Word(Surface surface) => surface switch
    {
        Surface.Composer => Composer,
        Surface.Tui => Tui,
        Surface.Companion => Companion,
        Surface.Phone => Phone,
        _ => AnyApp,
    };

    /// <summary>The preset for <paramref name="surface"/>; null for a word this build does not know.</summary>
    public ShapePreset? Preset(Surface surface) => WireWords153.ParsePreset(Word(surface));
}

/// <summary><c>voice::HistoryConfig</c> (153 §6.5): <c>voice.history</c> — rows and audio kept, and whether corrections are learned.</summary>
public sealed record HistoryConfig
{
    public bool Enabled { get; init; } = true;
    public uint Rows { get; init; } = 200;
    public uint AudioMib { get; init; } = 64;
    public bool Learn { get; init; } = true;
}

/// <summary>
/// <c>voice_settings()</c>'s reply (153 F1): the user-facing <c>voice.*</c> keys as the engine holds them now (what every <c>set_*</c> left behind,
/// <c>polish_model</c> from the live slot). <c>standby_secs</c> is a u64 (bigint in TS); <c>duck_media</c> stays its word
/// (<c>off</c> · <c>duck</c> · <c>pause</c>, §8.6). An engine without F1 answers -32601 — Settings then falls back to the last values written here.
/// </summary>
public sealed record VoiceSettings
{
    public ShapePresets Shape { get; init; } = new();
    public EndPhrases EndCommands { get; init; } = new();

    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> InputDevices { get; init; } = new();

    public ulong StandbySecs { get; init; } = 8;
    public string DuckMedia { get; init; } = "off";
    public string? PolishModel { get; init; }
    public Dictionary<string, string> PolishProfiles { get; init; } = new(StringComparer.Ordinal);
    public HistoryConfig History { get; init; } = new();
}
