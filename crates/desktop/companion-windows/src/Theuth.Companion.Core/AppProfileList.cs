using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// 153 W153D — the pure half of Settings › Dictation's "Apps" list (the Mac's AppProfilesSettings.swift): add a running app (deduped by
/// exe, terminal from the heuristics), add the any_app default row ("Other apps", not removable once added — the Mac's rule), remove,
/// change one row, the label, the apps the Add menu offers, and the JSON blob stored under <c>appProfiles</c>.
/// </summary>
public static class AppProfileList
{
    public const string DefaultRowName = "Other apps";

    public static bool HasDefault(IReadOnlyList<AppProfile> rows) =>
        rows.Any(p => string.Equals(p.Id, AppMatch.DefaultRowId, StringComparison.OrdinalIgnoreCase));

    public static string Label(AppProfile p) =>
        string.Equals(p.Id, AppMatch.DefaultRowId, StringComparison.OrdinalIgnoreCase) ? DefaultRowName : p.Name.Length > 0 ? p.Name : p.Id;

    /// <summary>A row for <paramref name="exe"/> (the id keeps the ".exe"), unless one exists; terminal from <see cref="AppMatch.IsTerminal"/>.</summary>
    public static List<AppProfile> Add(IReadOnlyList<AppProfile> rows, string exe, string name)
    {
        var list = rows.ToList();
        string id = Exe(exe);
        if (id.Length == 0 || list.Any(p => AppMatch.Same(p.Id, id))) return list;
        list.Add(new AppProfile { Id = id, Name = name.Trim().Length > 0 ? name.Trim() : AppMatch.Normalize(id), Terminal = AppMatch.IsTerminal(id, null) });
        return list;
    }

    public static List<AppProfile> AddDefault(IReadOnlyList<AppProfile> rows)
    {
        var list = rows.ToList();
        if (!HasDefault(list)) list.Add(new AppProfile { Id = AppMatch.DefaultRowId, Name = DefaultRowName });
        return list;
    }

    public static bool Removable(AppProfile p) => !string.Equals(p.Id, AppMatch.DefaultRowId, StringComparison.OrdinalIgnoreCase);

    public static List<AppProfile> Remove(IReadOnlyList<AppProfile> rows, string id) =>
        rows.Where(p => !(Removable(p) && string.Equals(p.Id, id, StringComparison.OrdinalIgnoreCase))).ToList();

    public static List<AppProfile> Update(IReadOnlyList<AppProfile> rows, string id, Func<AppProfile, AppProfile> change) =>
        rows.Select(p => string.Equals(p.Id, id, StringComparison.OrdinalIgnoreCase) ? change(p) : p).ToList();

    /// <summary>The Add menu: running apps with a window, minus the ones already listed and the companion itself, deduped, by name.</summary>
    public static List<(string Exe, string Name)> Addable(IEnumerable<(string Exe, string Name)> running, IReadOnlyList<AppProfile> rows, string ownExe)
    {
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var result = new List<(string Exe, string Name)>();
        foreach (var (exe, name) in running)
        {
            string id = Exe(exe);
            if (id.Length == 0 || AppMatch.Same(id, ownExe) || rows.Any(p => AppMatch.Same(p.Id, id)) || !seen.Add(AppMatch.Normalize(id))) continue;
            result.Add((id, name.Trim().Length > 0 ? name.Trim() : AppMatch.Normalize(id)));
        }
        return result.OrderBy(a => a.Name, StringComparer.CurrentCultureIgnoreCase).ToList();
    }

    public static string Serialize(IReadOnlyList<AppProfile> rows) => TheuthJson.Serialize(rows.ToList());

    public static List<AppProfile> Parse(string? json)
    {
        if (string.IsNullOrWhiteSpace(json)) return new List<AppProfile>();
        try { return TheuthJson.Deserialize<List<AppProfile>>(json) ?? new List<AppProfile>(); }
        catch (Exception e) when (e is System.Text.Json.JsonException or NotSupportedException or InvalidOperationException) { return new List<AppProfile>(); }
    }

    private static string Exe(string exe)
    {
        var s = (exe ?? "").Trim();
        int slash = Math.Max(s.LastIndexOf('\\'), s.LastIndexOf('/'));
        if (slash >= 0) s = s[(slash + 1)..];
        if (s.Length == 0) return "";
        return s.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) ? s : s + ".exe";
    }
}
