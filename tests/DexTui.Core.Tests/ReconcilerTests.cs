namespace DexTui.Core.Tests;

/// <summary>
/// These encode the promise that a background refresh never disturbs the user.
/// </summary>
public class ReconcilerTests
{
    private static DexTask Task(string id, string? parent = null, params string[] children) => new()
    {
        Id = id,
        ParentId = parent,
        Name = id,
        Children = children,
        CreatedAt = new DateTimeOffset(2026, 1, 1, 0, 0, 0, TimeSpan.Zero),
    };

    private static Dictionary<string, DexTask> Index(params DexTask[] tasks)
        => tasks.ToDictionary(t => t.Id, StringComparer.Ordinal);

    private static ViewState State(string? selected, params string[] expanded)
        => new(new HashSet<string>(expanded, StringComparer.Ordinal), selected);

    [Fact]
    public void Selection_survives_a_refresh_that_changes_nothing()
    {
        var prev = Index(Task("a"), Task("b"));
        List<DexTask> next = [Task("a"), Task("b")];

        var result = Reconciler.Reconcile(State("b", "a"), prev, next);

        Assert.Equal("b", result.SelectedId);
        Assert.Contains("a", result.ExpandedIds);
    }

    [Fact]
    public void Selection_survives_when_unrelated_tasks_are_added()
    {
        // The exact scenario of an agent creating tasks while you are reading.
        var prev = Index(Task("a"), Task("b"));
        List<DexTask> next = [Task("a"), Task("b"), Task("new1"), Task("new2")];

        var result = Reconciler.Reconcile(State("b"), prev, next);

        Assert.Equal("b", result.SelectedId);
    }

    [Fact]
    public void New_tasks_arrive_collapsed_so_the_tree_does_not_explode()
    {
        var prev = Index(Task("a"));
        List<DexTask> next = [Task("a"), Task("newparent", null, "kid"), Task("kid", "newparent")];

        var result = Reconciler.Reconcile(State("a"), prev, next);

        Assert.DoesNotContain("newparent", result.ExpandedIds);
    }

    [Fact]
    public void Expansion_is_dropped_only_for_tasks_that_disappeared()
    {
        var prev = Index(Task("a"), Task("gone"));
        List<DexTask> next = [Task("a")];

        var result = Reconciler.Reconcile(State("a", "a", "gone"), prev, next);

        Assert.Contains("a", result.ExpandedIds);
        Assert.DoesNotContain("gone", result.ExpandedIds);
    }

    [Fact]
    public void A_deleted_selection_falls_back_to_its_next_sibling()
    {
        var prev = Index(
            Task("parent", null, "s1", "s2", "s3"),
            Task("s1", "parent"),
            Task("s2", "parent"),
            Task("s3", "parent"));

        List<DexTask> next = [Task("parent", null, "s1", "s3"), Task("s1", "parent"), Task("s3", "parent")];

        var result = Reconciler.Reconcile(State("s2"), prev, next);

        // Stays where the cursor visually was, rather than jumping to the top.
        Assert.Equal("s3", result.SelectedId);
    }

    [Fact]
    public void A_deleted_last_sibling_falls_back_to_the_previous_one()
    {
        var prev = Index(
            Task("parent", null, "s1", "s2"),
            Task("s1", "parent"),
            Task("s2", "parent"));

        List<DexTask> next = [Task("parent", null, "s1"), Task("s1", "parent")];

        var result = Reconciler.Reconcile(State("s2"), prev, next);

        Assert.Equal("s1", result.SelectedId);
    }

    [Fact]
    public void When_the_whole_branch_is_gone_selection_climbs_to_a_surviving_ancestor()
    {
        var prev = Index(
            Task("root", null, "mid"),
            Task("mid", "root", "leaf"),
            Task("leaf", "mid"));

        // Both mid and leaf were removed; only root survives.
        List<DexTask> next = [Task("root")];

        var result = Reconciler.Reconcile(State("leaf"), prev, next);

        Assert.Equal("root", result.SelectedId);
    }

    [Fact]
    public void An_empty_store_yields_no_selection_rather_than_a_stale_id()
    {
        var prev = Index(Task("a"));

        var result = Reconciler.Reconcile(State("a"), prev, []);

        Assert.Null(result.SelectedId);
    }

    [Fact]
    public void No_previous_selection_lands_on_the_first_root()
    {
        var result = Reconciler.Reconcile(ViewState.Empty, new Dictionary<string, DexTask>(), [Task("a"), Task("b")]);

        Assert.Equal("a", result.SelectedId);
    }

    [Fact]
    public void A_selected_root_that_is_deleted_moves_to_another_root()
    {
        var prev = Index(Task("r1"), Task("r2"));
        List<DexTask> next = [Task("r2")];

        var result = Reconciler.Reconcile(State("r1"), prev, next);

        Assert.Equal("r2", result.SelectedId);
    }
}
