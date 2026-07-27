using DexTui.Core;
using Terminal.Gui.App;
using Terminal.Gui.Drawing;
using Terminal.Gui.Input;
using Terminal.Gui.ViewBase;
using Terminal.Gui.Views;

// Terminal.Gui 2.4 marks TextView obsolete in favour of the separate tui-cs/Editor
// package. We only need a read-only detail pane and a plain multi-line input box,
// which TextView still does correctly, so taking on another dependency is not
// warranted yet. Revisit if TextView is actually removed.
#pragma warning disable CS0618

namespace DexTui.App;

/// <summary>
/// Two-pane task browser: filter bar on top, tree on the left, detail on the right.
///
/// The invariant this class exists to protect is that a background refresh never
/// disturbs the user. All the tricky decisions about that live in
/// <see cref="Reconciler"/>; this type just applies them and honours the modal
/// interlock so a refresh can never land while a dialog is open.
/// </summary>
public sealed class MainWindow : Window
{
    private readonly IApplication _app;
    private readonly DexClient _client;
    private readonly DexStoreWatcher _watcher;

    private readonly TextField _queryField;
    private readonly Label _filterLabel;
    private readonly TreeView<TaskNode> _tree;
    private readonly TextView _detail;
    private readonly Label _status;

    private IReadOnlyList<DexTask> _tasks;
    private Dictionary<string, DexTask> _tasksById;
    private ViewState _viewState = ViewState.Empty;
    private StatusFilter _filter = StatusFilter.Pending;

    private bool _modalOpen;
    private bool _pendingRefresh;
    private bool _suppressSelectionEvents;

    public MainWindow(IApplication app, DexClient client, string storeDir, IReadOnlyList<DexTask> initial)
    {
        _app = app;
        _client = client;
        _tasks = initial;
        _tasksById = Index(initial);

        Title = $"dex-tui — {TaskDetail.StoreLabel(storeDir)} (Esc/q to quit, ? for help)";
        BorderStyle = LineStyle.Rounded;

        _queryField = new TextField
        {
            X = 3,
            Y = 0,
            Width = Dim.Fill(24),
            Height = 1,
        };

        var slash = new Label { X = 1, Y = 0, Text = "/" };

        _filterLabel = new Label
        {
            X = Pos.AnchorEnd(22),
            Y = 0,
            Width = 21,
            Height = 1,
        };

        _tree = new TreeView<TaskNode>
        {
            X = 0,
            Y = 1,
            Width = Dim.Percent(45),
            Height = Dim.Fill(1),
            BorderStyle = LineStyle.Single,
            TreeBuilder = new DelegateTreeBuilder<TaskNode>(
                childGetter: n => n.Children,
                canExpand: n => n.Children.Count > 0),
            AspectGetter = Describe,
        };

        _detail = new TextView
        {
            X = Pos.Right(_tree),
            Y = 1,
            Width = Dim.Fill(),
            Height = Dim.Fill(1),
            ReadOnly = true,
            WordWrap = true,
            BorderStyle = LineStyle.Single,
        };

        _status = new Label
        {
            X = 0,
            Y = Pos.AnchorEnd(1),
            Width = Dim.Fill(),
            Height = 1,
            Text = " s start  c complete  e edit  n new  a subtask  d delete  f filter  / find  ? help",
        };

        Add(slash, _queryField, _filterLabel, _tree, _detail, _status);

        _queryField.TextChanged += (_, _) => Rebuild(preserveSelection: true);
        _tree.SelectionChanged += (_, _) =>
        {
            if (!_suppressSelectionEvents)
            {
                _viewState = _viewState with { SelectedId = _tree.SelectedObject?.Id };
                RenderDetail();
            }
        };

        Rebuild(preserveSelection: false);

        // The store is not always ./.dex -- outside a git repo dex uses a global
        // store -- so we watch whatever `dex dir` reported.
        _watcher = new DexStoreWatcher(storeDir);
        _watcher.Changed += OnStoreChanged;
        _watcher.Start();
    }

    private static Dictionary<string, DexTask> Index(IEnumerable<DexTask> tasks)
        => tasks.ToDictionary(t => t.Id, StringComparer.Ordinal);

    private static string Describe(TaskNode n)
    {
        var blocked = n.Task.IsBlocked ? " ⊘" : "";
        // Scaffolding nodes survived only to lead to a match; mark them so the
        // filter result is not misread as a direct hit.
        var dim = n.IsMatch ? "" : " ·";
        return $"{TaskDetail.Glyph(n.Task)} {n.Task.Name}{blocked}{dim}";
    }

    // ---------- refresh ----------

    /// <summary>Fires on a watcher thread; never touch views from here.</summary>
    private void OnStoreChanged() => _app.Invoke(() =>
    {
        if (_modalOpen)
        {
            // Deferred rather than dropped: applied when the dialog closes.
            _pendingRefresh = true;
            return;
        }

        _ = RefreshAsync();
    });

    private async Task RefreshAsync()
    {
        var result = await _client.ListAsync().ConfigureAwait(false);

        _app.Invoke(() =>
        {
            if (!result.Success)
            {
                // Keep the last good model rather than blanking the view.
                SetStatus($"refresh failed: {Flatten(result.Error)}");
                return;
            }

            ApplyTasks(result.Value!);
        });
    }

    private void ApplyTasks(IReadOnlyList<DexTask> next)
    {
        _viewState = Reconciler.Reconcile(_viewState, _tasksById, next);
        _tasks = next;
        _tasksById = Index(next);
        Rebuild(preserveSelection: true);
    }

    /// <summary>
    /// Rebuilds the visible tree. Selection and expansion are restored by task id,
    /// never by row index, because indices shift the moment anything is added.
    /// </summary>
    private void Rebuild(bool preserveSelection)
    {
        var roots = TaskTree.Build(_tasks, _queryField.Text, _filter);
        var byId = TaskTree.Flatten(roots).ToDictionary(n => n.Id, StringComparer.Ordinal);

        _suppressSelectionEvents = true;
        try
        {
            _tree.ClearObjects();
            foreach (var r in roots)
            {
                _tree.AddObject(r);
            }

            foreach (var id in _viewState.ExpandedIds)
            {
                if (byId.TryGetValue(id, out var node))
                {
                    _tree.Expand(node);
                }
            }

            var targetId = preserveSelection ? _viewState.SelectedId : null;
            if (targetId is not null && byId.TryGetValue(targetId, out var selected))
            {
                RevealAncestors(selected, byId);
                _tree.SelectedObject = selected;
            }
            else
            {
                var first = roots.FirstOrDefault();
                _tree.SelectedObject = first;
                _viewState = _viewState with { SelectedId = first?.Id };
            }
        }
        finally
        {
            _suppressSelectionEvents = false;
        }

        _filterLabel.Text = _filter switch
        {
            StatusFilter.All => "[ all | pending | ◐ ]".Replace("all", "ALL"),
            StatusFilter.InProgress => "[ all | pending | ◐* ]",
            _ => "[ all | PENDING | ◐ ]",
        };

        RenderDetail();
    }

    /// <summary>A selected task is useless if its parents are collapsed around it.</summary>
    private void RevealAncestors(TaskNode node, Dictionary<string, TaskNode> byId)
    {
        var chain = new List<TaskNode>();
        var cursor = node.Task.ParentId;
        var guard = new HashSet<string>(StringComparer.Ordinal);

        while (cursor is not null && guard.Add(cursor) && byId.TryGetValue(cursor, out var parent))
        {
            chain.Add(parent);
            cursor = parent.Task.ParentId;
        }

        chain.Reverse();
        foreach (var p in chain)
        {
            _tree.Expand(p);
        }
    }

    private void RenderDetail()
    {
        var node = _tree.SelectedObject;
        if (node is null)
        {
            _detail.Text = _tasks.Count == 0
                ? "No tasks yet.\n\nPress n to create one."
                : "No tasks match the current filter.\n\nPress f to change it, or clear the search box.";
            return;
        }

        _detail.Text = TaskDetail.Render(node.Task, _tasksById);
    }

    private void SetStatus(string message)
        => _status.Text = $" {message}";

    private void ResetStatus()
        => _status.Text = " s start  c complete  e edit  n new  a subtask  d delete  f filter  / find  ? help";

    private static string Flatten(string s)
        => s.Replace('\n', ' ').Replace('\r', ' ').Trim();

    // ---------- keys ----------

    protected override bool OnKeyDown(Key key)
    {
        // The search box owns ordinary typing; only Esc gets us back out of it.
        if (_queryField.HasFocus)
        {
            if (key == Key.Esc)
            {
                _tree.SetFocus();
                return true;
            }

            return base.OnKeyDown(key);
        }

        var selected = _tree.SelectedObject?.Task;

        if (key == Key.Q || key == Key.Esc)
        {
            _app.RequestStop();
            return true;
        }

        // Key has named constants only for letters, digits and control keys;
        // punctuation is matched through the printable rune.
        if (key.AsRune.Value == '/')
        {
            _queryField.SetFocus();
            return true;
        }

        if (key == Key.F)
        {
            _filter = _filter switch
            {
                StatusFilter.Pending => StatusFilter.InProgress,
                StatusFilter.InProgress => StatusFilter.All,
                _ => StatusFilter.Pending,
            };
            Rebuild(preserveSelection: true);
            return true;
        }

        if (key == Key.F5 || key == Key.R.WithCtrl)
        {
            _ = RefreshAsync();
            return true;
        }

        if (key.AsRune.Value == '?')
        {
            ShowHelp();
            return true;
        }

        if (key == Key.N)
        {
            CreateTask(parent: null);
            return true;
        }

        if (selected is null)
        {
            return base.OnKeyDown(key);
        }

        if (key == Key.A)
        {
            CreateTask(parent: selected);
            return true;
        }

        if (key == Key.S)
        {
            RunAction(() => _client.StartAsync(selected.Id), $"started {selected.Name}");
            return true;
        }

        if (key == Key.C)
        {
            CompleteTask(selected);
            return true;
        }

        if (key == Key.E)
        {
            EditTask(selected);
            return true;
        }

        if (key == Key.D)
        {
            DeleteTask(selected);
            return true;
        }

        return base.OnKeyDown(key);
    }

    // ---------- actions ----------

    private void RunAction(Func<Task<DexResult<bool>>> action, string successMessage)
    {
        SetStatus("working…");

        _ = Task.Run(async () =>
        {
            var result = await action().ConfigureAwait(false);
            _app.Invoke(() =>
            {
                if (result.Success)
                {
                    SetStatus(successMessage);
                    _ = RefreshAsync();
                }
                else
                {
                    ShowError(result.Error);
                }
            });
        });
    }

    private void CompleteTask(DexTask task)
    {
        var result = Prompt($"Complete: {task.Name}", "Result", "", multiline: true);
        if (result is null)
        {
            return;
        }

        SetStatus("working…");
        _ = Task.Run(async () =>
        {
            var r = await _client.CompleteAsync(task.Id, result).ConfigureAwait(false);
            _app.Invoke(() =>
            {
                if (r.Success)
                {
                    SetStatus($"completed {task.Name}");
                    _ = RefreshAsync();
                    return;
                }

                // dex refuses to complete a task with unfinished subtasks unless
                // forced; offer that directly instead of making the user retype.
                if (r.Error.Contains("subtask", StringComparison.OrdinalIgnoreCase))
                {
                    _modalOpen = true;
                    var choice = MessageBox.Query(_app, "Incomplete subtasks", Flatten(r.Error), "Force", "Cancel");
                    _modalOpen = false;
                    DrainPendingRefresh();

                    if (choice == 0)
                    {
                        RunAction(() => _client.CompleteAsync(task.Id, result, force: true), $"completed {task.Name}");
                    }

                    return;
                }

                ShowError(r.Error);
            });
        });
    }

    private void EditTask(DexTask task)
    {
        var name = Prompt($"Edit: {task.Name}", "Name", task.Name, multiline: false);
        if (name is null)
        {
            return;
        }

        var description = Prompt($"Edit: {task.Name}", "Description", task.Description ?? "", multiline: true);
        if (description is null)
        {
            return;
        }

        RunAction(() => _client.EditAsync(task.Id, name, description), $"updated {name}");
    }

    private void CreateTask(DexTask? parent)
    {
        var title = parent is null ? "New task" : $"New subtask of: {parent.Name}";

        var name = Prompt(title, "Name", "", multiline: false);
        if (string.IsNullOrWhiteSpace(name))
        {
            return;
        }

        var description = Prompt(title, "Description", "", multiline: true);
        if (description is null)
        {
            return;
        }

        RunAction(() => _client.CreateAsync(name, description, parent?.Id), $"created {name}");
    }

    private void DeleteTask(DexTask task)
    {
        var childCount = _tasks.Count(t => t.ParentId == task.Id);
        var warning = childCount > 0
            ? $"\"{task.Name}\" and its {childCount} subtask(s) will be deleted."
            : $"\"{task.Name}\" will be deleted.";

        _modalOpen = true;
        var choice = MessageBox.Query(_app, "Delete task", warning, "Cancel", "Delete");
        _modalOpen = false;
        DrainPendingRefresh();

        if (choice == 1)
        {
            RunAction(() => _client.DeleteAsync(task.Id), $"deleted {task.Name}");
        }
    }

    private void ShowError(string message)
    {
        _modalOpen = true;
        MessageBox.ErrorQuery(_app, "dex error", Flatten(message), "OK");
        _modalOpen = false;
        ResetStatus();
        DrainPendingRefresh();
    }

    private void ShowHelp()
    {
        _modalOpen = true;
        MessageBox.Query(
            _app,
            "dex-tui",
            """
            ↑ ↓        move          s   start task
            → ←        expand        c   complete (prompts for result)
            /          search        e   edit name and description
            f          cycle filter  n   new top-level task
            F5         refresh       a   new subtask of selection
            q / Esc    quit          d   delete (with confirmation)

            The view refreshes itself when the dex store changes.
            Your selection, expansion and any open dialog are never disturbed.
            """,
            "OK");
        _modalOpen = false;
        DrainPendingRefresh();
    }

    /// <summary>Applies any refresh that arrived while a dialog was open.</summary>
    private void DrainPendingRefresh()
    {
        if (!_pendingRefresh)
        {
            return;
        }

        _pendingRefresh = false;
        _ = RefreshAsync();
    }

    /// <summary>Returns null when cancelled, which is distinct from an empty string.</summary>
    private string? Prompt(string title, string label, string initial, bool multiline)
    {
        _modalOpen = true;
        try
        {
            string? result = null;

            var dialog = new Dialog
            {
                Title = title,
                Width = Dim.Percent(70),
                Height = multiline ? 14 : 8,
            };

            var lbl = new Label { X = 1, Y = 0, Text = label };

            View input;
            if (multiline)
            {
                var tv = new TextView
                {
                    X = 1,
                    Y = 1,
                    Width = Dim.Fill(1),
                    Height = Dim.Fill(2),
                    Text = initial,
                    WordWrap = true,
                };
                input = tv;
            }
            else
            {
                var tf = new TextField
                {
                    X = 1,
                    Y = 1,
                    Width = Dim.Fill(1),
                    Height = 1,
                    Text = initial,
                };
                input = tf;
            }

            var ok = new Button { Text = "OK", IsDefault = true };
            var cancel = new Button { Text = "Cancel" };

            ok.Accepting += (_, e) =>
            {
                result = input is TextView tv ? tv.Text : ((TextField)input).Text;
                e.Handled = true;
                dialog.RequestStop();
            };

            cancel.Accepting += (_, e) =>
            {
                result = null;
                e.Handled = true;
                dialog.RequestStop();
            };

            dialog.AddButton(ok);
            dialog.AddButton(cancel);
            dialog.Add(lbl, input);

            input.SetFocus();
            _app.Run(dialog);
            dialog.Dispose();

            return result;
        }
        finally
        {
            _modalOpen = false;
            DrainPendingRefresh();
        }
    }

    protected override void Dispose(bool disposing)
    {
        if (disposing)
        {
            _watcher.Changed -= OnStoreChanged;
            _watcher.Dispose();
        }

        base.Dispose(disposing);
    }
}
