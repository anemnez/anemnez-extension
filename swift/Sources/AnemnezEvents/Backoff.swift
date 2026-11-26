// The Swift faces' one retry backoff (house rule 2): 1 s after the first miss, doubling to 60 s. AnemnezEvents' watch
// link takes it, and ErgonCore's glass folds its own copy onto it when it takes AnemnezEvents (ergon:K/swift-events).
// Time is handed in (rule 3): this only says how long, never reads a clock.

/// How long to wait before trying again, after `misses` unanswered tries in a row.
public enum Backoff {
    /// The wait after the first miss.
    public static let first: Duration = .seconds(1)
    /// The longest wait, however many misses.
    public static let atMost: Duration = .seconds(60)

    /// The wait after `misses` misses (1 or more): `first`, doubling each miss, never past `atMost`.
    public static func after(misses: Int) -> Duration {
        let doublings = max(0, min(misses - 1, 6))
        return min(first * (1 << doublings), atMost)
    }
}
