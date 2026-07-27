namespace DexTui.Core.Tests;

/// <summary>
/// Exercises the real FileSystemWatcher against a real directory. These touch the
/// filesystem and involve timing, so the waits are deliberately generous.
/// </summary>
public class DexStoreWatcherTests : IDisposable
{
    private readonly string _dir = Path.Combine(
        Path.GetTempPath(),
        "dextui-watch-" + Guid.NewGuid().ToString("N")[..8]);

    public DexStoreWatcherTests() => Directory.CreateDirectory(_dir);

    public void Dispose()
    {
        try
        {
            Directory.Delete(_dir, recursive: true);
        }
        catch (IOException)
        {
            // Best effort; a leftover temp dir must not fail the suite.
        }

        GC.SuppressFinalize(this);
    }

    private static bool WaitForSignal(SemaphoreSlim signal, TimeSpan timeout)
        => signal.Wait(timeout);

    [Fact]
    public void Fires_when_the_store_file_changes()
    {
        var signal = new SemaphoreSlim(0);
        using var watcher = new DexStoreWatcher(
            _dir,
            debounce: TimeSpan.FromMilliseconds(100),
            safetyInterval: TimeSpan.FromMinutes(5)); // long, so only the FS event can fire

        watcher.Changed += () => signal.Release();
        watcher.Start();

        Assert.True(watcher.IsWatchingFileSystem);
        File.WriteAllText(Path.Combine(_dir, "tasks.jsonl"), """{"id":"a"}""");

        Assert.True(WaitForSignal(signal, TimeSpan.FromSeconds(5)), "watcher did not fire on a file write");
    }

    [Fact]
    public void Collapses_a_burst_of_writes_into_few_notifications()
    {
        var count = 0;
        using var watcher = new DexStoreWatcher(
            _dir,
            debounce: TimeSpan.FromMilliseconds(300),
            safetyInterval: TimeSpan.FromMinutes(5));

        watcher.Changed += () => Interlocked.Increment(ref count);
        watcher.Start();

        // A single dex write touches the file several times; without debouncing
        // this would trigger a `dex list` spawn per event.
        var file = Path.Combine(_dir, "tasks.jsonl");
        for (var i = 0; i < 10; i++)
        {
            File.WriteAllText(file, $$"""{"id":"a","n":{{i}}}""");
            Thread.Sleep(20);
        }

        Thread.Sleep(1500);

        Assert.InRange(Volatile.Read(ref count), 1, 3);
    }

    [Fact]
    public void Stays_silent_while_the_store_is_idle()
    {
        var count = 0;
        using var watcher = new DexStoreWatcher(
            _dir,
            debounce: TimeSpan.FromMilliseconds(100),
            safetyInterval: TimeSpan.FromMinutes(5));

        watcher.Changed += () => Interlocked.Increment(ref count);
        watcher.Start();

        Thread.Sleep(800);

        // Idle cost is the whole point of watching instead of polling.
        Assert.Equal(0, Volatile.Read(ref count));
    }

    [Fact]
    public void Falls_back_to_the_safety_poll_when_the_store_does_not_exist_yet()
    {
        // A brand-new project has no store directory until the first task exists.
        var missing = Path.Combine(_dir, "not-created-yet");
        var signal = new SemaphoreSlim(0);

        using var watcher = new DexStoreWatcher(
            missing,
            debounce: TimeSpan.FromMilliseconds(50),
            safetyInterval: TimeSpan.FromMilliseconds(300));

        watcher.Changed += () => signal.Release();
        watcher.Start();

        Assert.False(watcher.IsWatchingFileSystem);
        Assert.True(WaitForSignal(signal, TimeSpan.FromSeconds(5)), "safety poll did not fire");
    }

    [Fact]
    public void Does_not_fire_after_disposal()
    {
        var count = 0;
        var watcher = new DexStoreWatcher(
            _dir,
            debounce: TimeSpan.FromMilliseconds(50),
            safetyInterval: TimeSpan.FromMilliseconds(100));

        watcher.Changed += () => Interlocked.Increment(ref count);
        watcher.Start();
        watcher.Dispose();

        File.WriteAllText(Path.Combine(_dir, "tasks.jsonl"), "{}");
        Thread.Sleep(600);

        Assert.Equal(0, Volatile.Read(ref count));
    }
}
