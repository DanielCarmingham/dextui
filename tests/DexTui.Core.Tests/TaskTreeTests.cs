namespace DexTui.Core.Tests;

public class TaskTreeTests
{
    private static DexTask Task(
        string id,
        string? parent = null,
        string name = "task",
        int priority = 1,
        bool completed = false,
        DateTimeOffset? started = null,
        string? description = null,
        int createdOffsetSeconds = 0) => new()
        {
            Id = id,
            ParentId = parent,
            Name = name,
            Description = description,
            Priority = priority,
            Completed = completed,
            StartedAt = started,
            CreatedAt = new DateTimeOffset(2026, 1, 1, 0, 0, 0, TimeSpan.Zero).AddSeconds(createdOffsetSeconds),
        };

    [Fact]
    public void Build_nests_children_under_their_parent()
    {
        List<DexTask> tasks =
        [
            Task("root"),
            Task("child", parent: "root"),
            Task("grandchild", parent: "child"),
        ];

        var roots = TaskTree.Build(tasks, filter: StatusFilter.All);

        var root = Assert.Single(roots);
        Assert.Equal("root", root.Id);
        var child = Assert.Single(root.Children);
        Assert.Equal("child", child.Id);
        Assert.Equal("grandchild", Assert.Single(child.Children).Id);
    }

    [Fact]
    public void Build_orders_siblings_by_priority_then_creation_time()
    {
        // dex returns tasks sorted by id, which is meaningless to a reader.
        List<DexTask> tasks =
        [
            Task("a", parent: "r", priority: 2, createdOffsetSeconds: 0),
            Task("b", parent: "r", priority: 1, createdOffsetSeconds: 50),
            Task("c", parent: "r", priority: 1, createdOffsetSeconds: 10),
            Task("r"),
        ];

        var root = Assert.Single(TaskTree.Build(tasks, filter: StatusFilter.All));

        Assert.Equal(["c", "b", "a"], root.Children.Select(n => n.Id));
    }

    [Fact]
    public void Build_promotes_orphans_to_roots_instead_of_dropping_them()
    {
        // Parent absent from the payload (e.g. archived); the child must stay visible.
        List<DexTask> tasks = [Task("orphan", parent: "missing")];

        var roots = TaskTree.Build(tasks, filter: StatusFilter.All);

        Assert.Equal("orphan", Assert.Single(roots).Id);
    }

    [Fact]
    public void Build_survives_a_parent_cycle_without_stack_overflow()
    {
        List<DexTask> tasks =
        [
            Task("a", parent: "b"),
            Task("b", parent: "a"),
        ];

        // A malformed store must not take the whole TUI down.
        var roots = TaskTree.Build(tasks, filter: StatusFilter.All);

        Assert.NotNull(roots);
    }

    [Fact]
    public void Pending_filter_hides_completed_tasks()
    {
        List<DexTask> tasks =
        [
            Task("done", completed: true),
            Task("todo"),
        ];

        var roots = TaskTree.Build(tasks, filter: StatusFilter.Pending);

        Assert.Equal("todo", Assert.Single(roots).Id);
    }

    [Fact]
    public void InProgress_filter_shows_only_started_incomplete_tasks()
    {
        List<DexTask> tasks =
        [
            Task("started", started: DateTimeOffset.UtcNow),
            Task("not-started"),
            Task("finished", completed: true, started: DateTimeOffset.UtcNow),
        ];

        var roots = TaskTree.Build(tasks, filter: StatusFilter.InProgress);

        Assert.Equal("started", Assert.Single(roots).Id);
    }

    [Fact]
    public void A_matching_child_keeps_its_non_matching_ancestors_as_scaffolding()
    {
        List<DexTask> tasks =
        [
            Task("parent", name: "unrelated"),
            Task("child", parent: "parent", name: "login bug"),
        ];

        var roots = TaskTree.Build(tasks, query: "login", filter: StatusFilter.All);

        var parent = Assert.Single(roots);
        Assert.Equal("parent", parent.Id);
        // Kept only to lead to the match, so the UI can dim it.
        Assert.False(parent.IsMatch);
        Assert.True(Assert.Single(parent.Children).IsMatch);
    }

    [Fact]
    public void Query_matches_description_as_well_as_name()
    {
        List<DexTask> tasks = [Task("a", name: "nothing", description: "mentions LOGIN here")];

        var roots = TaskTree.Build(tasks, query: "login", filter: StatusFilter.All);

        Assert.Single(roots);
    }

    [Fact]
    public void Query_that_matches_nothing_yields_an_empty_tree()
    {
        List<DexTask> tasks = [Task("a", name: "alpha"), Task("b", name: "beta")];

        Assert.Empty(TaskTree.Build(tasks, query: "zzz", filter: StatusFilter.All));
    }

    [Fact]
    public void Flatten_walks_the_tree_depth_first()
    {
        List<DexTask> tasks =
        [
            Task("r"),
            Task("a", parent: "r", createdOffsetSeconds: 1),
            Task("b", parent: "r", createdOffsetSeconds: 2),
            Task("a1", parent: "a"),
        ];

        var flat = TaskTree.Flatten(TaskTree.Build(tasks, filter: StatusFilter.All)).Select(n => n.Id);

        Assert.Equal(["r", "a", "a1", "b"], flat);
    }
}
