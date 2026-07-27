namespace DexTui.Core;

/// <summary>What the user has done to the view, expressed only in task ids.</summary>
public sealed record ViewState(IReadOnlySet<string> ExpandedIds, string? SelectedId)
{
    public static ViewState Empty { get; } =
        new(new HashSet<string>(StringComparer.Ordinal), null);
}

/// <summary>
/// Decides how the view should look after a background refresh.
///
/// This is deliberately a pure function with no Terminal.Gui dependency: it is
/// the rule that a refresh must never disturb the user, and it is the part most
/// worth testing. Everything is keyed by task id, never by row index, because
/// row indices shift the moment anything is added or removed.
/// </summary>
public static class Reconciler
{
    public static ViewState Reconcile(
        ViewState previous,
        IReadOnlyDictionary<string, DexTask> previousById,
        IReadOnlyList<DexTask> next)
    {
        var nextIds = new HashSet<string>(next.Select(t => t.Id), StringComparer.Ordinal);

        // Keep expansion only for tasks that still exist. Tasks that appeared since
        // the last refresh are absent here, so new work arrives collapsed and a
        // background agent adding subtasks cannot explode the tree under the cursor.
        var expanded = new HashSet<string>(
            previous.ExpandedIds.Where(nextIds.Contains),
            StringComparer.Ordinal);

        var selected = ResolveSelection(previous.SelectedId, previousById, nextIds, next);

        return new ViewState(expanded, selected);
    }

    private static string? ResolveSelection(
        string? selectedId,
        IReadOnlyDictionary<string, DexTask> previousById,
        HashSet<string> nextIds,
        IReadOnlyList<DexTask> next)
    {
        if (selectedId is null)
        {
            return FirstRoot(next);
        }

        // The common case: whatever was selected is still there.
        if (nextIds.Contains(selectedId))
        {
            return selectedId;
        }

        // It vanished (deleted or archived elsewhere). Prefer a sibling, so the
        // cursor stays at roughly the same place in the tree...
        var sibling = NearestSurvivingSibling(selectedId, previousById, nextIds);
        if (sibling is not null)
        {
            return sibling;
        }

        // ...then fall back to the nearest surviving ancestor, which keeps the
        // cursor in the same branch rather than snapping to the top of the list.
        var ancestor = NearestSurvivingAncestor(selectedId, previousById, nextIds);
        return ancestor ?? FirstRoot(next);
    }

    private static string? NearestSurvivingSibling(
        string selectedId,
        IReadOnlyDictionary<string, DexTask> previousById,
        HashSet<string> nextIds)
    {
        if (!previousById.TryGetValue(selectedId, out var removed))
        {
            return null;
        }

        // Reconstruct the sibling order as it was before the refresh.
        List<string> siblings = removed.ParentId is not null
                                && previousById.TryGetValue(removed.ParentId, out var parent)
            ? parent.Children.ToList()
            : previousById.Values
                .Where(t => t.ParentId is null || !previousById.ContainsKey(t.ParentId))
                .OrderBy(t => t.Priority)
                .ThenBy(t => t.CreatedAt ?? DateTimeOffset.MaxValue)
                .Select(t => t.Id)
                .ToList();

        var idx = siblings.IndexOf(selectedId);
        if (idx < 0)
        {
            return null;
        }

        // Scan outward from where the task used to be: next sibling first, then previous.
        for (var offset = 1; offset < siblings.Count; offset++)
        {
            if (idx + offset < siblings.Count && nextIds.Contains(siblings[idx + offset]))
            {
                return siblings[idx + offset];
            }

            if (idx - offset >= 0 && nextIds.Contains(siblings[idx - offset]))
            {
                return siblings[idx - offset];
            }
        }

        return null;
    }

    private static string? NearestSurvivingAncestor(
        string selectedId,
        IReadOnlyDictionary<string, DexTask> previousById,
        HashSet<string> nextIds)
    {
        var seen = new HashSet<string>(StringComparer.Ordinal);
        var cursor = selectedId;

        while (cursor is not null && seen.Add(cursor))
        {
            if (!previousById.TryGetValue(cursor, out var task))
            {
                return null;
            }

            cursor = task.ParentId;
            if (cursor is not null && nextIds.Contains(cursor))
            {
                return cursor;
            }
        }

        return null;
    }

    private static string? FirstRoot(IReadOnlyList<DexTask> next)
    {
        var ids = new HashSet<string>(next.Select(t => t.Id), StringComparer.Ordinal);
        return next
            .Where(t => t.ParentId is null || !ids.Contains(t.ParentId))
            .OrderBy(t => t.Priority)
            .ThenBy(t => t.CreatedAt ?? DateTimeOffset.MaxValue)
            .Select(t => t.Id)
            .FirstOrDefault();
    }
}
