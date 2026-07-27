using System.Text;

namespace DexTui.Core;

/// <summary>
/// Composes the text shown in the detail pane.
///
/// This lives in Core rather than in the view so it can be tested without a
/// terminal, and so `--selftest` can exercise the same code the UI renders.
/// It is built entirely from the already-fetched list: `dex show` is never
/// called, because selection changes on every arrow key and a ~180ms process
/// spawn per keypress would make navigation unusable.
/// </summary>
public static class TaskDetail
{
    public static string Glyph(DexTask t) => t.Status switch
    {
        DexStatus.Completed => "✓",
        DexStatus.InProgress => "◐",
        _ => "○",
    };

    public static string Render(DexTask t, IReadOnlyDictionary<string, DexTask> byId)
    {
        var sb = new StringBuilder();
        sb.AppendLine(t.Name);
        sb.AppendLine(new string('─', Math.Clamp(t.Name.Length, 8, 60)));
        sb.AppendLine();
        sb.AppendLine($"id        {t.Id}");
        sb.AppendLine($"status    {Glyph(t)} {Describe(t.Status)}");
        sb.AppendLine($"priority  {t.Priority}");

        if (t.ParentId is not null && byId.TryGetValue(t.ParentId, out var parent))
        {
            sb.AppendLine($"parent    {parent.Name}");
        }

        if (t.BlockedBy.Count > 0)
        {
            var names = t.BlockedBy.Select(id => byId.TryGetValue(id, out var b) ? b.Name : id);
            sb.AppendLine($"blocked   {string.Join(", ", names)}");
        }

        sb.AppendLine($"created   {Local(t.CreatedAt)}");

        if (t.StartedAt is not null)
        {
            sb.AppendLine($"started   {Local(t.StartedAt)}");
        }

        if (t.CompletedAt is not null)
        {
            sb.AppendLine($"done      {Local(t.CompletedAt)}");
        }

        // Full text, never truncated -- the whole reason for a detail pane.
        if (!string.IsNullOrWhiteSpace(t.Description))
        {
            sb.AppendLine();
            sb.AppendLine(t.Description);
        }

        if (!string.IsNullOrWhiteSpace(t.Result))
        {
            sb.AppendLine();
            sb.AppendLine("result");
            sb.AppendLine(t.Result);
        }

        return sb.ToString();
    }

    private static string Describe(DexStatus s) => s switch
    {
        DexStatus.Completed => "completed",
        DexStatus.InProgress => "in progress",
        _ => "pending",
    };

    private static string Local(DateTimeOffset? d)
        => d is null ? "-" : d.Value.ToLocalTime().ToString("yyyy-MM-dd HH:mm");

    /// <summary>
    /// A human label for the store. dex reports a path, which is either a
    /// project-local `.dex` directory or the shared global store; neither
    /// basename alone reads well in a title bar.
    /// </summary>
    public static string StoreLabel(string storeDir)
    {
        var trimmed = Path.TrimEndingDirectorySeparator(storeDir);
        var name = Path.GetFileName(trimmed);

        if (string.Equals(name, ".dex", StringComparison.Ordinal))
        {
            var parent = Path.GetFileName(Path.GetDirectoryName(trimmed) ?? "");
            return string.IsNullOrEmpty(parent) ? ".dex" : parent;
        }

        // Outside a git repo dex falls back to ~/.config/dex/local.
        return trimmed.Contains(Path.Combine(".config", "dex"), StringComparison.Ordinal)
            ? "global"
            : name;
    }
}
