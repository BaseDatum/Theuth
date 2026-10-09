namespace Theuth.Companion.Rpc;

/// <summary>
/// Design 153 W153B (the PIN): the dictation, dictionary, history, device, shaping and polish calls (153 §14.2), typed over
/// <see cref="CallAsync{T}"/> / <see cref="CallOrNullAsync{T}"/> / <see cref="CallRawAsync"/>, positional params in
/// <c>crates/rpc/src/api.rs</c> order (integrate/153 b7ec00b9: :1594-1685, :461-466), enums as their spelled-out wire words.
/// A one-element list argument is wrapped (<c>new object?[] { list }</c>): a bare <c>string[]</c> would spread into N params
/// (the W4 <c>set_event_focus</c> bug). An engine before 153 answers the new methods <c>-32601</c> (<see cref="RpcException.Code"/>);
/// callers degrade. The existing one-argument <c>dictation_start</c> stays Core's <c>AskRpc.DictationStartAsync(rate)</c>.
/// </summary>
public sealed partial class RpcClient
{
    // MARK: dictation (api.rs:1594 dictation_start · :1614 dictation_status · :1618 dictation_correction · :1571 voice_status)

    /// <summary>
    /// <c>dictation_start(sample_rate, opts)</c> → the <c>DictationId</c>. <paramref name="opts"/> null sends the one-element
    /// <c>[rate]</c> an engine before 153 accepts; non-null sends <c>[rate, opts]</c> (153 §14.1). Never pass a null rate from the
    /// companion: <c>None</c> opens the ENGINE host's microphone (130 W6 rule).
    /// </summary>
    public Task<string> DictationStartAsync(uint? sampleRate, DictationOpts? opts) =>
        opts is null ? CallAsync<string>("dictation_start", sampleRate) : CallAsync<string>("dictation_start", sampleRate, opts);

    /// <summary><c>dictation_status()</c> → model, running dictation, and (153) <c>input_device</c> + <c>last</c> timings.</summary>
    public Task<DictationState> DictationStatusAsync() => CallAsync<DictationState>("dictation_status");

    /// <summary><c>voice_status()</c> → the TTS/STT model state and (153) <c>input_devices</c>, the ordered mic preference.</summary>
    public Task<VoiceStatus> VoiceStatusAsync() => CallAsync<VoiceStatus>("voice_status");

    /// <summary>
    /// <c>dictation_correction(id, sent_text)</c> (153 §6.1): the text actually sent after dictation <paramref name="id"/>; the engine
    /// diffs it against what it delivered and learns single-word corrections. Not sent when the field was cleared or the dictation discarded.
    /// </summary>
    public Task DictationCorrectionAsync(string id, string sentText) => CallRawAsync("dictation_correction", id, sentText);

    // MARK: history (api.rs:1625 dictation_history · :1628 dictation_history_forget · :1632 dictation_audio_get) — local peers only

    /// <summary><c>dictation_history(limit)</c>: the newest rows (raw / shaped / polished, timings); null limit = the engine's.</summary>
    public Task<ValueList<HistoryRow>> DictationHistoryAsync(uint? limit = null) =>
        CallAsync<ValueList<HistoryRow>>("dictation_history", limit);

    /// <summary><c>dictation_history_forget(id)</c>: forget one row and its audio, or everything when <paramref name="id"/> is null.</summary>
    public Task DictationHistoryForgetAsync(string? id) => CallRawAsync("dictation_history_forget", id);

    /// <summary><c>dictation_audio_get(id, utterance)</c> → base64 of mono i16 16 kHz PCM (one utterance, or all when null);
    /// <c>Err("no audio")</c> when evicted.</summary>
    public Task<string> DictationAudioGetAsync(string id, uint? utterance) => CallAsync<string>("dictation_audio_get", id, utterance);

    // MARK: dictionary (api.rs:1639 dictionary_list · :1642 dictionary_put · :1645 dictionary_delete · :1648 dictionary_suggestion_answer)

    /// <summary><c>dictionary_list(scope)</c>: user + learned entries, optionally one scope.</summary>
    public Task<ValueList<Entry>> DictionaryListAsync(EntryScope? scope = null) => CallAsync<ValueList<Entry>>("dictionary_list", scope);

    /// <summary><c>dictionary_put(entry)</c> → the stored entry (upsert by canonical + scope; an empty id gets a uuid).</summary>
    public Task<Entry> DictionaryPutAsync(Entry entry) => CallAsync<Entry>("dictionary_put", entry);

    /// <summary><c>dictionary_delete(id)</c> → false when unknown.</summary>
    public Task<bool> DictionaryDeleteAsync(string id) => CallAsync<bool>("dictionary_delete", id);

    /// <summary><c>dictionary_suggestion_answer(id, answer)</c>: the suggestion card's Add / Not now / Never (153 §6.3 step 4).</summary>
    public Task DictionarySuggestionAnswerAsync(string id, SuggestionAnswer answer) =>
        CallRawAsync("dictionary_suggestion_answer", id, answer.Word());

    // MARK: devices and shaping (api.rs:1655 list_input_devices · :1658 set_input_devices · :1661 set_shape · :1664 set_end_phrases)

    /// <summary><c>list_input_devices()</c>: every input the engine host enumerates, the default flagged (153 §8.1).</summary>
    public Task<ValueList<InputDevice>> ListInputDevicesAsync() => CallAsync<ValueList<InputDevice>>("list_input_devices");

    /// <summary><c>set_input_devices(names)</c>: persist <c>voice.input_devices</c> (ordered; empty = the system default only). One positional list.</summary>
    public Task SetInputDevicesAsync(IReadOnlyList<string> names) =>
        CallRawAsync("set_input_devices", new object?[] { names.ToArray() });

    /// <summary><c>set_shape(surface, preset)</c>: persist <c>voice.shape.&lt;surface&gt;</c> (153 §4.5).</summary>
    public Task SetShapeAsync(Surface surface, Preset preset) => CallRawAsync("set_shape", surface.Word(), preset.Word());

    /// <summary><c>set_end_phrases(phrases)</c>: persist <c>voice.end_commands</c> (153 §7.2).</summary>
    public Task SetEndPhrasesAsync(EndPhrases phrases) => CallRawAsync("set_end_phrases", phrases);

    // MARK: polish (api.rs:461 polish_model · :465 set_polish_model · :1668 polish · :1677 rewrite)

    /// <summary><c>polish_model()</c>: the live <c>voice.polish_model</c>, or null (the chain: title model, then the session's).</summary>
    public Task<ModelInfo?> PolishModelAsync() => CallOrNullAsync<ModelInfo>("polish_model");

    /// <summary><c>set_polish_model(model)</c>: persist <c>voice.polish_model</c>; null clears it. Answers the new value.</summary>
    public Task<ModelInfo?> SetPolishModelAsync(string? model) => CallOrNullAsync<ModelInfo>("set_polish_model", model);

    /// <summary><c>polish(text, destination, session)</c> (153 §9): <c>agent_prompt</c> is refused; fails closed (<see cref="PolishReply.Refused"/>).</summary>
    public Task<PolishReply> PolishAsync(string text, Destination destination, string? session = null) =>
        CallAsync<PolishReply>("polish", text, destination, session);

    /// <summary>
    /// <c>rewrite(instruction, selection, app, thread)</c> (153 §13.6): <paramref name="thread"/> carries the earlier
    /// (instruction, result) pairs of the same selection — Rust <c>Vec&lt;(String, String)&gt;</c>, two-element arrays on the wire.
    /// </summary>
    public Task<PolishReply> RewriteAsync(string instruction, string selection, AppProfile app, IReadOnlyList<(string Instruction, string Result)>? thread = null) =>
        CallAsync<PolishReply>("rewrite", instruction, selection, app, thread?.Select(t => new[] { t.Instruction, t.Result }).ToArray());
}
