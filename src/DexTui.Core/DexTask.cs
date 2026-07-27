using System.Text.Json.Serialization;

namespace DexTui.Core;

/// <summary>Derived lifecycle state. dex stores this as a bool plus timestamps.</summary>
public enum DexStatus
{
    Pending,
    InProgress,
    Completed,
}

/// <summary>
/// A single dex task, matching the shape emitted by `dex list --json`.
/// Note the mixed casing in the wire format: most keys are snake_case but
/// `blockedBy`/`blocks` are camelCase, so every property is mapped explicitly
/// rather than relying on a naming policy.
/// </summary>
public sealed record DexTask
{
    [JsonPropertyName("id")] public string Id { get; init; } = "";
    [JsonPropertyName("parent_id")] public string? ParentId { get; init; }
    [JsonPropertyName("name")] public string Name { get; init; } = "";
    [JsonPropertyName("description")] public string? Description { get; init; }
    [JsonPropertyName("priority")] public int Priority { get; init; } = 1;
    [JsonPropertyName("completed")] public bool Completed { get; init; }
    [JsonPropertyName("result")] public string? Result { get; init; }

    [JsonPropertyName("created_at")] public DateTimeOffset? CreatedAt { get; init; }
    [JsonPropertyName("updated_at")] public DateTimeOffset? UpdatedAt { get; init; }
    [JsonPropertyName("started_at")] public DateTimeOffset? StartedAt { get; init; }
    [JsonPropertyName("completed_at")] public DateTimeOffset? CompletedAt { get; init; }

    [JsonPropertyName("blockedBy")] public IReadOnlyList<string> BlockedBy { get; init; } = [];
    [JsonPropertyName("blocks")] public IReadOnlyList<string> Blocks { get; init; } = [];
    [JsonPropertyName("children")] public IReadOnlyList<string> Children { get; init; } = [];

    /// <summary>dex has no status field; it is implied by `completed` and `started_at`.</summary>
    public DexStatus Status =>
        Completed ? DexStatus.Completed
        : StartedAt is not null ? DexStatus.InProgress
        : DexStatus.Pending;

    public bool IsBlocked => BlockedBy.Count > 0;
}
