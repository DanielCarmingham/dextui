namespace DexTui.Core;

/// <summary>
/// Signals that the dex store changed. Fires on a background thread.
///
/// The watcher only says *that* something changed, never *what* -- the caller
/// re-reads through `dex list --json`. That keeps us off dex's private on-disk
/// format while still costing nothing at all when the store is idle.
/// </summary>
public sealed class DexStoreWatcher : IDisposable
{
    private readonly string _storeDir;
    private readonly TimeSpan _debounce;
    private readonly Lock _gate = new();

    private FileSystemWatcher? _fsw;
    private Timer? _debounceTimer;
    private Timer? _safetyTimer;
    private bool _disposed;

    public event Action? Changed;

    public DexStoreWatcher(string storeDir, TimeSpan? debounce = null, TimeSpan? safetyInterval = null)
    {
        _storeDir = storeDir;
        _debounce = debounce ?? TimeSpan.FromMilliseconds(250);
        SafetyInterval = safetyInterval ?? TimeSpan.FromSeconds(10);
    }

    public TimeSpan SafetyInterval { get; }

    /// <summary>True when we are relying on the safety poll alone (store dir absent).</summary>
    public bool IsWatchingFileSystem => _fsw is not null;

    public void Start()
    {
        // A brand-new project has no store directory until the first task is
        // created, and FileSystemWatcher throws on a missing path. Fall back to
        // the safety poll, which will pick the store up once it appears.
        if (Directory.Exists(_storeDir))
        {
            _fsw = new FileSystemWatcher(_storeDir)
            {
                NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.Size,
                IncludeSubdirectories = false,
            };

            _fsw.Changed += OnFsEvent;
            _fsw.Created += OnFsEvent;
            _fsw.Deleted += OnFsEvent;
            _fsw.Renamed += OnFsEvent;
            _fsw.EnableRaisingEvents = true;
        }

        // Writers often replace tasks.jsonl via a temp file plus rename, which on
        // macOS can surface as an event we miss. The slow poll bounds staleness.
        _safetyTimer = new Timer(_ => Raise(), null, SafetyInterval, SafetyInterval);
    }

    private void OnFsEvent(object sender, FileSystemEventArgs e)
    {
        // A single dex write touches the file several times; collapse the burst.
        lock (_gate)
        {
            if (_disposed)
            {
                return;
            }

            _debounceTimer ??= new Timer(_ => Raise(), null, Timeout.InfiniteTimeSpan, Timeout.InfiniteTimeSpan);
            _debounceTimer.Change(_debounce, Timeout.InfiniteTimeSpan);
        }
    }

    private void Raise()
    {
        lock (_gate)
        {
            if (_disposed)
            {
                return;
            }
        }

        Changed?.Invoke();
    }

    public void Dispose()
    {
        lock (_gate)
        {
            if (_disposed)
            {
                return;
            }

            _disposed = true;
        }

        if (_fsw is not null)
        {
            _fsw.EnableRaisingEvents = false;
            _fsw.Dispose();
            _fsw = null;
        }

        _debounceTimer?.Dispose();
        _safetyTimer?.Dispose();
    }
}
