using DexTui.Core;

namespace DexTui.Core.Tests;

public class DexClientTests
{
    /// <summary>Captured verbatim from a real `dex list --json --all`.</summary>
    private const string RealListJson = """
    [
      {
        "id": "s7rngopd",
        "parent_id": "b4d5gfpl",
        "name": "Handle \"quoted\" names & $pecial chars",
        "description": "Line one\nLine two with a  double space",
        "priority": 1,
        "completed": false,
        "result": null,
        "metadata": null,
        "created_at": "2026-07-27T01:47:19.253Z",
        "updated_at": "2026-07-27T01:47:19.253Z",
        "started_at": null,
        "completed_at": null,
        "blockedBy": [],
        "blocks": [],
        "children": []
      },
      {
        "id": "uqolu0wq",
        "parent_id": "b4d5gfpl",
        "name": "Pick a toolkit",
        "description": "Evaluate options",
        "priority": 2,
        "completed": true,
        "result": "Chose Terminal.Gui v2",
        "metadata": null,
        "created_at": "2026-07-27T01:47:19.093Z",
        "updated_at": "2026-07-27T01:47:26.371Z",
        "started_at": "2026-07-27T01:47:26.225Z",
        "completed_at": "2026-07-27T01:47:26.371Z",
        "blockedBy": ["s7rngopd"],
        "blocks": [],
        "children": []
      }
    ]
    """;

    [Fact]
    public async Task ListAsync_parses_the_mixed_case_wire_format()
    {
        var runner = new FakeProcessRunner(RealListJson);
        var client = new DexClient(runner);

        var result = await client.ListAsync();

        Assert.True(result.Success);
        var tasks = result.Value!;
        Assert.Equal(2, tasks.Count);

        var quoted = tasks[0];
        // snake_case key
        Assert.Equal("b4d5gfpl", quoted.ParentId);
        // camelCase key in the same payload -- the reason we map every property explicitly
        Assert.Equal(["s7rngopd"], tasks[1].BlockedBy);
        Assert.True(tasks[1].IsBlocked);
    }

    [Fact]
    public async Task ListAsync_preserves_quotes_and_newlines_in_task_text()
    {
        var runner = new FakeProcessRunner(RealListJson);
        var client = new DexClient(runner);

        var tasks = (await client.ListAsync()).Value!;

        Assert.Equal("Handle \"quoted\" names & $pecial chars", tasks[0].Name);
        Assert.Equal("Line one\nLine two with a  double space", tasks[0].Description);
    }

    [Fact]
    public async Task ListAsync_always_requests_all_so_filtering_can_stay_client_side()
    {
        var runner = new FakeProcessRunner("[]");
        await new DexClient(runner).ListAsync();

        Assert.Equal(["list", "--json", "--all"], runner.LastArgs);
    }

    [Fact]
    public async Task Status_is_derived_from_completed_and_started_at()
    {
        var runner = new FakeProcessRunner(RealListJson);
        var tasks = (await new DexClient(runner).ListAsync()).Value!;

        Assert.Equal(DexStatus.Pending, tasks[0].Status);
        Assert.Equal(DexStatus.Completed, tasks[1].Status);
    }

    [Fact]
    public async Task CompleteAsync_passes_text_as_one_argv_entry_without_shell_quoting()
    {
        var runner = new FakeProcessRunner();
        var nasty = "done: \"quoted\" & $HOME\nsecond line";

        await new DexClient(runner).CompleteAsync("abc123", nasty);

        // The dangerous text must arrive as a single unmangled argument.
        Assert.Contains(nasty, runner.LastArgs);
        Assert.Equal(["complete", "abc123", "--result", nasty, "--no-commit"], runner.LastArgs);
    }

    [Fact]
    public async Task CompleteAsync_always_sends_no_commit_so_synced_tasks_do_not_block()
    {
        var runner = new FakeProcessRunner();
        await new DexClient(runner).CompleteAsync("abc123", "done");

        Assert.Contains("--no-commit", runner.LastArgs);
    }

    [Fact]
    public async Task CompleteAsync_adds_force_only_when_asked()
    {
        var runner = new FakeProcessRunner();
        var client = new DexClient(runner);

        await client.CompleteAsync("abc123", "done");
        Assert.DoesNotContain("--force", runner.LastArgs);

        await client.CompleteAsync("abc123", "done", force: true);
        Assert.Contains("--force", runner.LastArgs);
    }

    [Fact]
    public async Task DeleteAsync_always_forces_because_dex_would_otherwise_prompt()
    {
        var runner = new FakeProcessRunner();
        await new DexClient(runner).DeleteAsync("abc123");

        // An interactive prompt would hang a TUI with no way to answer it.
        Assert.Equal(["delete", "abc123", "--force"], runner.LastArgs);
    }

    [Fact]
    public async Task CreateAsync_omits_optional_flags_when_not_supplied()
    {
        var runner = new FakeProcessRunner();
        await new DexClient(runner).CreateAsync("Just a name", null, null);

        Assert.Equal(["create", "Just a name"], runner.LastArgs);
    }

    [Fact]
    public async Task CreateAsync_includes_parent_for_subtasks()
    {
        var runner = new FakeProcessRunner();
        await new DexClient(runner).CreateAsync("Child", "details", "parent1");

        Assert.Equal(["create", "Child", "--description", "details", "--parent", "parent1"], runner.LastArgs);
    }

    [Fact]
    public async Task EditAsync_sends_only_the_fields_that_changed()
    {
        var runner = new FakeProcessRunner();
        await new DexClient(runner).EditAsync("abc123", name: "New name", description: null);

        Assert.Equal(["edit", "abc123", "--name", "New name"], runner.LastArgs);
    }

    [Fact]
    public async Task EditAsync_can_clear_a_description_with_empty_string()
    {
        var runner = new FakeProcessRunner();
        // null means "leave alone"; empty string is an explicit clear.
        await new DexClient(runner).EditAsync("abc123", name: null, description: "");

        Assert.Equal(["edit", "abc123", "--description", ""], runner.LastArgs);
    }

    [Fact]
    public async Task Failures_surface_stderr_rather_than_an_exit_code()
    {
        var runner = new FakeProcessRunner(
            stderr: "Task has 2 incomplete subtasks. Use --force to override.",
            exitCode: 1);

        var result = await new DexClient(runner).CompleteAsync("abc123", "done");

        Assert.False(result.Success);
        Assert.Contains("incomplete subtasks", result.Error);
    }

    [Fact]
    public async Task Malformed_json_is_reported_rather_than_thrown()
    {
        var runner = new FakeProcessRunner("this is not json");

        var result = await new DexClient(runner).ListAsync();

        Assert.False(result.Success);
        Assert.Contains("could not parse", result.Error);
    }

    [Fact]
    public async Task GetStoreDirAsync_trims_the_reported_path()
    {
        // Must be honoured rather than assuming ./.dex -- outside a git repo dex
        // uses a global store under ~/.config/dex.
        var runner = new FakeProcessRunner("/Users/x/.config/dex/local\n");

        var result = await new DexClient(runner).GetStoreDirAsync();

        Assert.Equal("/Users/x/.config/dex/local", result.Value);
        Assert.Equal(["dir"], runner.LastArgs);
    }
}
