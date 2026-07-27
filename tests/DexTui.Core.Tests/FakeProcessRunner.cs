using DexTui.Core;

namespace DexTui.Core.Tests;

/// <summary>
/// Records the argv it is handed and replays canned output, so we can assert
/// exactly what would be executed without ever spawning dex.
/// </summary>
public sealed class FakeProcessRunner : IProcessRunner
{
    private readonly ProcessResult _result;

    public FakeProcessRunner(string stdout = "", string stderr = "", int exitCode = 0)
        => _result = new ProcessResult(exitCode, stdout, stderr);

    public List<IReadOnlyList<string>> Calls { get; } = [];

    public string? LastFileName { get; private set; }

    public IReadOnlyList<string> LastArgs => Calls[^1];

    public Task<ProcessResult> RunAsync(
        string fileName,
        IReadOnlyList<string> args,
        string? workingDirectory,
        CancellationToken ct = default)
    {
        LastFileName = fileName;
        Calls.Add(args.ToList());
        return Task.FromResult(_result);
    }
}
