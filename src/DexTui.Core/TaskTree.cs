namespace DexTui.Core;

public enum StatusFilter
{
    /// <summary>Everything, including completed. Mirrors `dex list --all`.</summary>
    All,

    /// <summary>Not yet completed. Mirrors the default `dex list`.</summary>
    Pending,

    /// <summary>Started but not completed. Mirrors `dex list --in-progress`.</summary>
    InProgress,
}

public sealed class TaskNode
{
    public required DexTask Task { get; init; }
    public required IReadOnlyList<TaskNode> Children { get; init; }

    /// <summary>
    /// False when this node only survived filtering because a descendant matched.
    /// Lets the UI dim pure scaffolding so it is clear what actually hit the filter.
    /// </summary>
    public required bool IsMatch { get; init; }

    public string Id => Task.Id;
}

public static class TaskTree
{
    /// <summary>
    /// Builds the hierarchy from dex's flat array. dex returns tasks sorted by id,
    /// which is meaningless to a human, so siblings are ordered by priority, then
    /// creation time, then name.
    ///
    /// Filtering keeps any task whose descendant matches, so a match is never
    /// orphaned from the path that leads to it.
    /// </summary>
    public static IReadOnlyList<TaskNode> Build(
        IEnumerable<DexTask> tasks,
        string? query = null,
        StatusFilter filter = StatusFilter.Pending)
    {
        var all = tasks.ToList();
        var byId = new Dictionary<string, DexTask>(StringComparer.Ordinal);
        foreach (var t in all)
        {
            byId[t.Id] = t;
        }

        var byParent = new Dictionary<string, List<DexTask>>(StringComparer.Ordinal);
        var roots = new List<DexTask>();
        foreach (var t in all)
        {
            // A task whose parent is missing from the payload is treated as a root
            // rather than dropped, so nothing silently disappears from the view.
            if (t.ParentId is not null && byId.ContainsKey(t.ParentId))
            {
                if (!byParent.TryGetValue(t.ParentId, out var list))
                {
                    byParent[t.ParentId] = list = [];
                }

                list.Add(t);
            }
            else
            {
                roots.Add(t);
            }
        }

        var q = string.IsNullOrWhiteSpace(query) ? null : query.Trim();
        var visiting = new HashSet<string>(StringComparer.Ordinal);

        return Sort(roots)
            .Select(r => BuildNode(r, byParent, q, filter, visiting))
            .OfType<TaskNode>()
            .ToList();
    }

    public static IEnumerable<TaskNode> Flatten(IEnumerable<TaskNode> nodes)
    {
        foreach (var n in nodes)
        {
            yield return n;
            foreach (var c in Flatten(n.Children))
            {
                yield return c;
            }
        }
    }

    private static TaskNode? BuildNode(
        DexTask task,
        Dictionary<string, List<DexTask>> byParent,
        string? query,
        StatusFilter filter,
        HashSet<string> visiting)
    {
        // Guards against a malformed store where parent links form a cycle.
        if (!visiting.Add(task.Id))
        {
            return null;
        }

        List<TaskNode> children = [];
        if (byParent.TryGetValue(task.Id, out var kids))
        {
            children = Sort(kids)
                .Select(k => BuildNode(k, byParent, query, filter, visiting))
                .OfType<TaskNode>()
                .ToList();
        }

        visiting.Remove(task.Id);

        var isMatch = Matches(task, query, filter);
        if (!isMatch && children.Count == 0)
        {
            return null;
        }

        return new TaskNode { Task = task, Children = children, IsMatch = isMatch };
    }

    private static IEnumerable<DexTask> Sort(IEnumerable<DexTask> tasks) =>
        tasks
            .OrderBy(t => t.Priority)
            .ThenBy(t => t.CreatedAt ?? DateTimeOffset.MaxValue)
            .ThenBy(t => t.Name, StringComparer.OrdinalIgnoreCase);

    private static bool Matches(DexTask t, string? query, StatusFilter filter)
    {
        var statusOk = filter switch
        {
            StatusFilter.All => true,
            StatusFilter.Pending => !t.Completed,
            StatusFilter.InProgress => t.Status == DexStatus.InProgress,
            _ => true,
        };

        if (!statusOk)
        {
            return false;
        }

        if (query is null)
        {
            return true;
        }

        return t.Name.Contains(query, StringComparison.OrdinalIgnoreCase)
            || (t.Description?.Contains(query, StringComparison.OrdinalIgnoreCase) ?? false);
    }
}
