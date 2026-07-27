using DexTui.App;
using DexTui.Core;
using Terminal.Gui.App;

// Preflight before taking over the terminal: a failure here should print a
// plain message rather than a half-initialised TUI.
var client = new DexClient(new ProcessRunner());

var storeDir = await client.GetStoreDirAsync();
if (!storeDir.Success)
{
    Console.Error.WriteLine($"dex-tui: {storeDir.Error}");
    Console.Error.WriteLine("dex-tui: is `dex` installed and on your PATH?");
    return 1;
}

var initial = await client.ListAsync();
if (!initial.Success)
{
    Console.Error.WriteLine($"dex-tui: {initial.Error}");
    return 1;
}

// --selftest exercises the whole data path (dex -> parse -> tree -> detail)
// and prints it as plain text. Useful for debugging, and the only way to verify
// the pipeline where no interactive terminal is available.
if (args.Contains("--selftest"))
{
    return SelfTest(storeDir.Value!, initial.Value!);
}

using IApplication app = Application.Create();
app.Init();

using var window = new MainWindow(app, client, storeDir.Value!, initial.Value!);
app.Run(window);

return 0;

static int SelfTest(string storeDir, IReadOnlyList<DexTask> tasks)
{
    Console.WriteLine($"store   {storeDir}");
    Console.WriteLine($"label   {TaskDetail.StoreLabel(storeDir)}");
    Console.WriteLine($"tasks   {tasks.Count}");
    Console.WriteLine();

    var byId = tasks.ToDictionary(t => t.Id, StringComparer.Ordinal);

    foreach (var filter in new[] { StatusFilter.All, StatusFilter.Pending, StatusFilter.InProgress })
    {
        var roots = TaskTree.Build(tasks, filter: filter);
        var count = TaskTree.Flatten(roots).Count();
        Console.WriteLine($"--- filter: {filter} ({count} visible) ---");
        foreach (var r in roots)
        {
            Print(r, 0);
        }

        Console.WriteLine();
    }

    var first = TaskTree.Flatten(TaskTree.Build(tasks, filter: StatusFilter.All)).FirstOrDefault();
    if (first is not null)
    {
        Console.WriteLine("--- detail pane for first task ---");
        Console.WriteLine(TaskDetail.Render(first.Task, byId));
    }

    return 0;

    static void Print(TaskNode n, int depth)
    {
        var indent = new string(' ', depth * 2);
        var scaffold = n.IsMatch ? "" : "  (scaffold)";
        Console.WriteLine($"{indent}{TaskDetail.Glyph(n.Task)} {n.Task.Name}{scaffold}");
        foreach (var c in n.Children)
        {
            Print(c, depth + 1);
        }
    }
}
