using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Theuth.Companion.Rpc;

/// <summary>
/// A <see cref="List{T}"/> with value equality — the C# stand-in for a Swift array inside an <c>Equatable</c>
/// struct, so the records that carry lists (items, options, answers) compare element by element.
/// </summary>
public sealed class ValueList<T> : List<T>, IEquatable<ValueList<T>>
{
    public ValueList() { }

    public ValueList(IEnumerable<T> items) : base(items) { }

    public bool Equals(ValueList<T>? other) =>
        other is not null && (ReferenceEquals(this, other) || this.SequenceEqual(other));

    public override bool Equals(object? obj) => obj is ValueList<T> other && Equals(other);

    public override int GetHashCode()
    {
        var h = new HashCode();
        foreach (var x in this) h.Add(x);
        return h.ToHashCode();
    }

    public static bool operator ==(ValueList<T>? a, ValueList<T>? b) => a is null ? b is null : a.Equals(b);

    public static bool operator !=(ValueList<T>? a, ValueList<T>? b) => !(a == b);
}

/// <summary>Builders for <see cref="ValueList{T}"/>.</summary>
public static class ValueList
{
    public static ValueList<T> Of<T>(params T[] items) => new(items);

    public static ValueList<T> ToValueList<T>(this IEnumerable<T> items) => new(items);
}

/// <summary>
/// Any JSON value — for fields the companion carries but never interprets (<c>args</c>, <c>usage</c>, secrets).
/// The port of Swift's <c>JSONValue</c>: null · bool · number · string · array · object, with value equality.
/// </summary>
[JsonConverter(typeof(JsonValueConverter))]
public abstract record JsonValue
{
    private JsonValue() { }

    public sealed record Null : JsonValue
    {
        public static readonly Null Instance = new();
    }

    public sealed record Bool(bool Value) : JsonValue;

    public sealed record Number(double Value) : JsonValue;

    public sealed record String(string Value) : JsonValue;

    public sealed record Array(ValueList<JsonValue> Items) : JsonValue;

    public sealed record Object(ValueDictionary Members) : JsonValue;

    /// <summary>The member <paramref name="key"/> of an object; null for anything else or a missing key.</summary>
    public JsonValue? this[string key] => this is Object o && o.Members.TryGetValue(key, out var v) ? v : null;

    /// <summary>The string payload, or null when this is not a string.</summary>
    public string? AsString => this is String s ? s.Value : null;
}

/// <summary>A string-keyed dictionary with value equality (the members of a <see cref="JsonValue.Object"/>).</summary>
public sealed class ValueDictionary : Dictionary<string, JsonValue>, IEquatable<ValueDictionary>
{
    public ValueDictionary() : base(StringComparer.Ordinal) { }

    public bool Equals(ValueDictionary? other)
    {
        if (other is null || other.Count != Count) return false;
        foreach (var (k, v) in this)
        {
            if (!other.TryGetValue(k, out var w) || !Equals(v, w)) return false;
        }
        return true;
    }

    public override bool Equals(object? obj) => obj is ValueDictionary other && Equals(other);

    public override int GetHashCode()
    {
        // Order-independent: XOR of the member hashes.
        int h = Count;
        foreach (var (k, v) in this) h ^= HashCode.Combine(k, v);
        return h;
    }
}

/// <summary>
/// The wire codec (Swift <c>TheuthJSON</c>): snake_case keys, RFC 3339 dates with or without fractional seconds
/// (chrono emits both), unknown fields ignored, nil optionals omitted when encoding, required fields enforced.
/// </summary>
public static class TheuthJson
{
    public static readonly JsonSerializerOptions Options = Create();

    private static JsonSerializerOptions Create()
    {
        var o = new JsonSerializerOptions
        {
            PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
            DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
            RespectNullableAnnotations = true,
            UnmappedMemberHandling = JsonUnmappedMemberHandling.Skip,
            WriteIndented = false,
        };
        o.Converters.Add(new Rfc3339DateConverter());
        // Registered here as well as on the types: a [JsonConverter] on a base record is not used for a derived
        // runtime type (an Answer.Choice passed as a positional param), a registered one is.
        o.Converters.Add(new AnswerConverter());
        o.Converters.Add(new JsonValueConverter());
        o.Converters.Add(new JsonStringEnumConverter(JsonNamingPolicy.SnakeCaseLower, allowIntegerValues: false));
        o.MakeReadOnly(populateMissingResolver: true);
        return o;
    }

    public static T Deserialize<T>(ReadOnlySpan<byte> utf8) =>
        JsonSerializer.Deserialize<T>(utf8, Options) ?? throw new JsonException($"null where {typeof(T).Name} was expected");

    public static T Deserialize<T>(string json) => Deserialize<T>(Encoding.UTF8.GetBytes(json));

    /// <summary>Like <see cref="Deserialize{T}(ReadOnlySpan{byte})"/> but a JSON <c>null</c> is allowed (Swift <c>T?</c>).</summary>
    public static T? DeserializeOrNull<T>(ReadOnlySpan<byte> utf8) where T : class =>
        JsonSerializer.Deserialize<T>(utf8, Options);

    public static string Serialize<T>(T value) => JsonSerializer.Serialize(value, Options);

    /// <summary>
    /// Positional JSON-RPC params (Swift <c>[AnyEncodable]</c>): each element encoded by its runtime type, a C# null
    /// as JSON <c>null</c> — <c>answer_ask("id-1", null)</c> → <c>["id-1",null]</c>.
    /// </summary>
    public static string EncodeParams(params object?[] values)
    {
        using var buffer = new MemoryStream();
        using (var w = new Utf8JsonWriter(buffer))
        {
            WriteParams(w, values);
        }
        return Encoding.UTF8.GetString(buffer.ToArray());
    }

    internal static void WriteParams(Utf8JsonWriter w, IReadOnlyList<object?> values)
    {
        w.WriteStartArray();
        foreach (var v in values)
        {
            if (v is null) w.WriteNullValue();
            else JsonSerializer.Serialize(w, v, v.GetType(), Options);
        }
        w.WriteEndArray();
    }
}

/// <summary>RFC 3339 / ISO 8601 with or without fractional seconds (any number of digits; chrono writes up to 9).</summary>
public sealed class Rfc3339DateConverter : JsonConverter<DateTimeOffset>
{
    private static readonly string[] Formats =
    {
        "yyyy-MM-dd'T'HH:mm:ssK",
        "yyyy-MM-dd'T'HH:mm:ss.FFFFFFFK",
    };

    public static DateTimeOffset Parse(string s)
    {
        if (TryParse(s, out var d)) return d;
        throw new JsonException($"bad date {s}");
    }

    public static bool TryParse(string s, out DateTimeOffset value)
    {
        // .NET keeps 7 fractional digits (100 ns ticks); chrono may send 9 — drop the extra ones.
        var t = s;
        int dot = t.IndexOf('.');
        if (dot > 0)
        {
            int end = dot + 1;
            while (end < t.Length && char.IsAsciiDigit(t[end])) end++;
            int digits = end - dot - 1;
            if (digits == 0) { value = default; return false; }
            if (digits > 7) t = t[..(dot + 8)] + t[end..];
        }
        return DateTimeOffset.TryParseExact(t, Formats, CultureInfo.InvariantCulture,
            DateTimeStyles.AssumeUniversal | DateTimeStyles.AdjustToUniversal, out value);
    }

    public override DateTimeOffset Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        if (reader.TokenType != JsonTokenType.String) throw new JsonException("expected an RFC 3339 date string");
        return Parse(reader.GetString()!);
    }

    public override void Write(Utf8JsonWriter writer, DateTimeOffset value, JsonSerializerOptions options) =>
        writer.WriteStringValue(value.ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss.FFFFFFF'Z'", CultureInfo.InvariantCulture));
}

/// <summary>Reads any JSON into <see cref="JsonValue"/>; a number is a double, as in Swift.</summary>
public sealed class JsonValueConverter : JsonConverter<JsonValue>
{
    public override bool HandleNull => true;

    public override bool CanConvert(Type typeToConvert) => typeof(JsonValue).IsAssignableFrom(typeToConvert);

    public override JsonValue Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        switch (reader.TokenType)
        {
            case JsonTokenType.Null: return JsonValue.Null.Instance;
            case JsonTokenType.True: return new JsonValue.Bool(true);
            case JsonTokenType.False: return new JsonValue.Bool(false);
            case JsonTokenType.Number: return new JsonValue.Number(reader.GetDouble());
            case JsonTokenType.String: return new JsonValue.String(reader.GetString()!);
            case JsonTokenType.StartArray:
            {
                var items = new ValueList<JsonValue>();
                while (reader.Read() && reader.TokenType != JsonTokenType.EndArray)
                {
                    items.Add(Read(ref reader, typeToConvert, options));
                }
                return new JsonValue.Array(items);
            }
            case JsonTokenType.StartObject:
            {
                var members = new ValueDictionary();
                while (reader.Read() && reader.TokenType != JsonTokenType.EndObject)
                {
                    var key = reader.GetString()!;
                    reader.Read();
                    members[key] = Read(ref reader, typeToConvert, options);
                }
                return new JsonValue.Object(members);
            }
            default:
                throw new JsonException($"unexpected token {reader.TokenType}");
        }
    }

    public override void Write(Utf8JsonWriter writer, JsonValue? value, JsonSerializerOptions options)
    {
        switch (value)
        {
            case null or JsonValue.Null: writer.WriteNullValue(); break;
            case JsonValue.Bool b: writer.WriteBooleanValue(b.Value); break;
            case JsonValue.Number n: writer.WriteNumberValue(n.Value); break;
            case JsonValue.String s: writer.WriteStringValue(s.Value); break;
            case JsonValue.Array a:
                writer.WriteStartArray();
                foreach (var x in a.Items) Write(writer, x, options);
                writer.WriteEndArray();
                break;
            case JsonValue.Object o:
                writer.WriteStartObject();
                foreach (var (k, v) in o.Members.OrderBy(kv => kv.Key, StringComparer.Ordinal))
                {
                    writer.WritePropertyName(k);
                    Write(writer, v, options);
                }
                writer.WriteEndObject();
                break;
        }
    }
}

/// <summary>
/// For Swift's <c>decodeIfPresent(...) ?? []</c>: a missing key keeps the property's default and a JSON
/// <c>null</c> reads as an empty list (a plain non-nullable list would reject the null).
/// </summary>
public sealed class NullAsEmptyListConverter<T> : JsonConverter<ValueList<T>>
{
    public override bool HandleNull => true;

    public override ValueList<T> Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        if (reader.TokenType == JsonTokenType.Null) return new ValueList<T>();
        if (reader.TokenType != JsonTokenType.StartArray) throw new JsonException("expected an array");
        var list = new ValueList<T>();
        while (reader.Read() && reader.TokenType != JsonTokenType.EndArray)
        {
            list.Add(JsonSerializer.Deserialize<T>(ref reader, options)!);
        }
        return list;
    }

    public override void Write(Utf8JsonWriter writer, ValueList<T> value, JsonSerializerOptions options)
    {
        writer.WriteStartArray();
        foreach (var x in value) JsonSerializer.Serialize(writer, x, options);
        writer.WriteEndArray();
    }
}
