using System.Text.Json;

namespace DexTui.Core;

public sealed record DexResult<T>(bool Success, T? Value, string Error)
{
    public static DexResult<T> Ok(T value) => new(true, value, "");
    public static DexResult<T> Fail(string error) => new(false, default, error);
}

/// <summary>
/// The only type that knows dex exists. Every mutation goes through the CLI
/// rather than touching tasks.jsonl directly, so dex's own validation and its
/// GitHub/Shortcut sync hooks always run.
/// </summary>
public sealed class DexClient(IProcessRunner runner, string? workingDirectory = null, string executable = "dex")
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true,
    };

    private readonly IProcessRunner _runner = runner;
    private readonly string? _cwd = workingDirectory;
    private readonly string _exe = executable;

    /// <summary>
    /// Resolves the store dex is actually using. This is NOT always ./.dex --
    /// outside a git repo dex falls back to a global store under ~/.config/dex,
    /// so the file watcher must follow whatever this reports.
    /// </summary>
    public async Task<DexResult<string>> GetStoreDirAsync(CancellationToken ct = default)
    {
        var r = await _runner.RunAsync(_exe, ["dir"], _cwd, ct).ConfigureAwait(false);
        return r.Success
            ? DexResult<string>.Ok(r.StdOut.Trim())
            : DexResult<string>.Fail(Describe(r, "dex dir"));
    }

    /// <summary>
    /// Always fetches with --all. Status filtering happens client-side so that
    /// flipping the filter is instant and never costs a process spawn.
    /// </summary>
    public async Task<DexResult<IReadOnlyList<DexTask>>> ListAsync(CancellationToken ct = default)
    {
        var r = await _runner.RunAsync(_exe, ["list", "--json", "--all"], _cwd, ct).ConfigureAwait(false);
        if (!r.Success)
        {
            return DexResult<IReadOnlyList<DexTask>>.Fail(Describe(r, "dex list"));
        }

        try
        {
            var tasks = JsonSerializer.Deserialize<List<DexTask>>(r.StdOut, JsonOptions) ?? [];
            return DexResult<IReadOnlyList<DexTask>>.Ok(tasks);
        }
        catch (JsonException ex)
        {
            return DexResult<IReadOnlyList<DexTask>>.Fail($"could not parse dex output: {ex.Message}");
        }
    }

    public Task<DexResult<bool>> StartAsync(string id, CancellationToken ct = default)
        => RunVoidAsync(["start", id], "dex start", ct);

    /// <summary>
    /// --no-commit is always sent: for tasks synced to GitHub, dex refuses to
    /// complete without either --commit or --no-commit, and a TUI has no way to
    /// answer that prompt. <paramref name="force"/> bypasses the incomplete-subtask check.
    /// </summary>
    public Task<DexResult<bool>> CompleteAsync(string id, string result, bool force = false, CancellationToken ct = default)
    {
        List<string> args = ["complete", id, "--result", result, "--no-commit"];
        if (force)
        {
            args.Add("--force");
        }

        return RunVoidAsync(args, "dex complete", ct);
    }

    public Task<DexResult<bool>> CreateAsync(string name, string? description, string? parentId, CancellationToken ct = default)
    {
        List<string> args = ["create", name];
        if (!string.IsNullOrWhiteSpace(description))
        {
            args.AddRange(["--description", description]);
        }

        if (!string.IsNullOrWhiteSpace(parentId))
        {
            args.AddRange(["--parent", parentId]);
        }

        return RunVoidAsync(args, "dex create", ct);
    }

    public Task<DexResult<bool>> EditAsync(string id, string? name, string? description, CancellationToken ct = default)
    {
        List<string> args = ["edit", id];
        if (name is not null)
        {
            args.AddRange(["--name", name]);
        }

        if (description is not null)
        {
            args.AddRange(["--description", description]);
        }

        return RunVoidAsync(args, "dex edit", ct);
    }

    /// <summary>Always forced: dex prompts interactively when subtasks exist, which would hang the TUI.</summary>
    public Task<DexResult<bool>> DeleteAsync(string id, CancellationToken ct = default)
        => RunVoidAsync(["delete", id, "--force"], "dex delete", ct);

    public Task<DexResult<bool>> ArchiveAsync(string id, CancellationToken ct = default)
        => RunVoidAsync(["archive", id], "dex archive", ct);

    private async Task<DexResult<bool>> RunVoidAsync(IReadOnlyList<string> args, string label, CancellationToken ct)
    {
        var r = await _runner.RunAsync(_exe, args, _cwd, ct).ConfigureAwait(false);
        return r.Success ? DexResult<bool>.Ok(true) : DexResult<bool>.Fail(Describe(r, label));
    }

    /// <summary>dex writes real diagnostics to stderr; surface them rather than an exit code.</summary>
    private static string Describe(ProcessResult r, string label)
    {
        var msg = r.StdErr.Trim();
        if (msg.Length == 0)
        {
            msg = r.StdOut.Trim();
        }

        return msg.Length == 0 ? $"{label} failed (exit {r.ExitCode})" : msg;
    }
}
