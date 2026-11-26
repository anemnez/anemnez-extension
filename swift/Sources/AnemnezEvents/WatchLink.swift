// The watch link (ergon docs/plans/ERGON-UI.md, the-facts-watch-wakes-the-glass-by-cursor; D m/38130): one product's
// Events/Watch held open for a face, as a pure machine on a handed clock (house rules 1, 3, 4, 5). Its host opens
// and reads the stream, decodes each frame through hydramq-message-ffi's frame_reply (the Rust Watcher's own reader,
// never a Swift copy: D m/33118), and hands the frame here. The link knows nothing of what an event means: its
// consumer maps what it hands.
//
//   idle ──start──▶ latest ──answer──▶ opening ──opened──▶ up ──beat / events──▶ up
//                     │                   │                 │ renew: opening from next, never said down
//                     │                   │                 │ compacted: said down, latest again (the gap read by value)
//                     ▼                   ▼                 ▼ silent past its bound, ended, or failed
//                   down ◀────────────────┴─────────────────┘
//                     │ retryAt passes: opening from the cursor, or latest when it has none
//                   refused (a typed refusal: Events unserved): retried at the longest backoff
//
// Invariants: at most one read or stream out, numbered past every earlier one; a frame or failure counts only for the
// one out; the cursor never moves back, but to a Latest's asOf after a compaction; no event below the cursor is
// handed; `up` is said only while a stream is open, and once per open.

/// A place in a producer's log: the log's name, and an offset in it.
public struct Position: Equatable, Sendable {
    public var log: String
    public var offset: UInt64

    public init(log: String, offset: UInt64) {
        self.log = log
        self.offset = offset
    }
}

/// One event, as the codec decoded it: its offset, subject, kind, and its body as the codec's JSON.
public struct WatchedEvent: Equatable, Sendable {
    public var offset: UInt64
    public var subject: String
    public var kind: String
    /// The body as hydramq-message-ffi says it (MESSAGE.md §8), for the consumer to read; never read here.
    public var body: String

    public init(offset: UInt64, subject: String, kind: String, body: String = "") {
        self.offset = offset
        self.subject = subject
        self.kind = kind
        self.body = body
    }
}

/// One watch frame, as frame_reply reads it.
public enum Frame: Equatable, Sendable {
    /// No event: the producer reached `next`.
    case beat(next: Position)
    /// Events, and where the next begin.
    case events([WatchedEvent], next: Position)
    /// The cursor is at or below the log's floor: what lies below was compacted.
    case compacted(floor: Position)
    /// The watch ended by design (its token's life): open it again from `next`.
    case renew(next: Position)
}

/// What the link hands its consumer, and asks its host to do.
public enum LinkAction: Equatable, Sendable {
    /// Read the latest event of each subject under `prefix`, and the position they're as of.
    case latest(prefix: String, seq: UInt64)
    /// Open the watch from `from`, a heartbeat every `beat`.
    case open(from: Position, beat: Duration, seq: UInt64)
    /// Cancel stream `seq`.
    case close(seq: UInt64)
    /// The watch is up: events will be handed as they come.
    case up
    /// The watch is down, said by name: nothing is handed until it's up again.
    case down(String)
    /// Events, in order, each once.
    case events([WatchedEvent])
}

/// What happens to the link.
public enum LinkEvent: Equatable, Sendable {
    case start
    /// Latest read `seq` answered: each subject's latest event, and the position they're as of.
    case latestAnswered(seq: UInt64, [WatchedEvent], asOf: Position)
    /// Stream `seq` opened.
    case opened(seq: UInt64)
    /// Stream `seq` brought a frame.
    case frame(seq: UInt64, Frame)
    /// Read or stream `seq` failed: refused (typed, such as Events unserved) or not.
    case failed(seq: UInt64, refused: Bool, why: String)
    /// Stream `seq` ended.
    case ended(seq: UInt64)
    /// The clock reached `deadline()`.
    case tick
}

/// Where the link stands.
public enum LinkState: Equatable, Sendable {
    case idle
    /// Latest read `seq` is out, due by `due`.
    case latest(due: Duration, seq: UInt64, misses: Int)
    /// Stream `seq` is opening from `from`, due by `due`.
    case opening(from: Position, due: Duration, seq: UInt64, misses: Int)
    /// Stream `seq` is up at `at`, and down if nothing comes by `silentBy`.
    case up(at: Position, silentBy: Duration, seq: UInt64)
    /// Down, said `why`, tried again at `retryAt`, from `at` when it has a cursor.
    case down(why: String, retryAt: Duration, misses: Int, at: Position?)
    /// Refused, said `why`, tried again at `retryAt`.
    case refused(why: String, retryAt: Duration, at: Position?)
}

/// One product's watch, held for a face.
public struct WatchLink: Equatable, Sendable {
    /// How long a Latest read or an open may take before it's down by name (a face's read bound).
    public static let within: Duration = .seconds(10)
    /// hydramq's `BEAT_MIN`, `BEAT_MAX` and `HOP_MARGIN` (hydramq-events wire.rs): the bound the Rust Watcher keeps.
    static let beatMin: Duration = .seconds(1)
    static let beatMax: Duration = .seconds(60)
    static let hopMargin: Duration = .seconds(5)

    /// The subjects a Latest read asks for when the link has no cursor.
    public let prefix: String
    /// The heartbeat the watch asks for.
    public let beat: Duration
    public private(set) var state: LinkState = .idle
    /// The number of the last read or stream asked for.
    public private(set) var seq: UInt64 = 0

    public init(prefix: String, beat: Duration = .seconds(10)) {
        self.prefix = prefix
        self.beat = beat
    }

    /// How long a stream may stay silent before it's down: the beat as the producer keeps it, plus the hop.
    public var silence: Duration { min(max(beat, Self.beatMin), Self.beatMax) + Self.hopMargin }

    /// The number of the read or stream out, if one is.
    public var out: UInt64? {
        switch state {
        case let .latest(_, seq, _), let .opening(_, _, seq, _), let .up(_, _, seq): seq
        case .idle, .down, .refused: nil
        }
    }

    /// The cursor: where the next events begin, if the link has one.
    public var at: Position? {
        switch state {
        case .idle, .latest: nil
        case let .opening(from, _, _, _): from
        case let .up(at, _, _): at
        case let .down(_, _, _, at), let .refused(_, _, at): at
        }
    }

    /// When the machine next acts on its own.
    public func deadline() -> Duration? {
        switch state {
        case .idle: nil
        case let .latest(due, _, _), let .opening(_, due, _, _): due
        case let .up(_, silentBy, _): silentBy
        case let .down(_, retryAt, _, _), let .refused(_, retryAt, _): retryAt
        }
    }

    public mutating func step(_ event: LinkEvent, now: Duration) -> [LinkAction] {
        var actions: [LinkAction] = []
        switch event {
        case .start:
            if case .idle = state { latest(misses: 0, now: now, into: &actions) }
        case let .latestAnswered(seq, events, asOf):
            guard case let .latest(_, out, misses) = state, out == seq else { break }
            if !events.isEmpty { actions.append(.events(events)) }
            open(from: asOf, misses: misses, now: now, into: &actions)
        case let .opened(seq):
            guard case let .opening(from, _, out, _) = state, out == seq else { break }
            state = .up(at: from, silentBy: now + silence, seq: seq)
            actions.append(.up)
        case let .frame(seq, frame):
            let at: Position
            switch state {
            case let .opening(from, _, out, _) where out == seq:
                at = from
                // A beat or events is the stream's answer as surely as its open: it's up. A renewal or a compaction
                // ends it before it was ever up, so neither says up.
                switch frame {
                case .beat, .events: actions.append(.up)
                case .renew, .compacted: break
                }
            case let .up(cursor, _, out) where out == seq:
                at = cursor
            case .idle, .latest, .opening, .up, .down, .refused:
                return actions
            }
            handle(frame, at: at, seq: seq, now: now, into: &actions)
        case let .failed(seq, refused, why):
            guard out == seq else { break }
            lost(why: why, refused: refused, now: now, into: &actions)
        case let .ended(seq):
            guard out == seq else { break }
            lost(why: "the producer ended the watch", refused: false, now: now, into: &actions)
        case .tick:
            tick(now: now, into: &actions)
        }
        assert(check() == nil, check() ?? "")
        return actions
    }

    /// The invariants a walk checks after every step.
    public func check() -> String? {
        if let out, out > seq { return "stream or read \(out) is numbered past the last asked, \(seq)" }
        return nil
    }

    private mutating func handle(
        _ frame: Frame, at: Position, seq: UInt64, now: Duration, into actions: inout [LinkAction]
    ) {
        switch frame {
        case let .beat(next):
            state = .up(at: Self.ahead(at, next), silentBy: now + silence, seq: seq)
        case let .events(events, next):
            // Only events at or past the cursor are handed: none twice, and none below it.
            let fresh = events.filter { $0.offset >= at.offset }
            if !fresh.isEmpty { actions.append(.events(fresh)) }
            state = .up(at: Self.ahead(at, next), silentBy: now + silence, seq: seq)
        case let .renew(next):
            actions.append(.close(seq: seq))
            open(from: Self.ahead(at, next), misses: 0, now: now, into: &actions)
        case .compacted:
            // What lay below the floor is gone: say so, and read each subject's latest by value, never skip silently.
            actions.append(.close(seq: seq))
            actions.append(.down("compacted past this watch"))
            latest(misses: 0, now: now, into: &actions)
        }
    }

    /// The cursor moved to `next`, unless that's behind it: it never moves back.
    private static func ahead(_ at: Position, _ next: Position) -> Position {
        next.log == at.log && next.offset >= at.offset ? next : at
    }

    private mutating func latest(misses: Int, now: Duration, into actions: inout [LinkAction]) {
        seq += 1
        state = .latest(due: now + Self.within, seq: seq, misses: misses)
        actions.append(.latest(prefix: prefix, seq: seq))
    }

    private mutating func open(from: Position, misses: Int, now: Duration, into actions: inout [LinkAction]) {
        seq += 1
        state = .opening(from: from, due: now + Self.within, seq: seq, misses: misses)
        actions.append(.open(from: from, beat: beat, seq: seq))
    }

    private mutating func lost(why: String, refused: Bool, now: Duration, into actions: inout [LinkAction]) {
        let (misses, stream): (Int, UInt64?) =
            switch state {
            case let .latest(_, _, misses): (misses, nil)
            case let .opening(_, _, seq, misses): (misses, seq)
            case let .up(_, _, seq): (0, seq)
            case .idle, .down, .refused: (0, nil)
            }
        if let stream { actions.append(.close(seq: stream)) }
        actions.append(.down(why))
        state = refused
            ? .refused(why: why, retryAt: now + Backoff.atMost, at: at)
            : .down(why: why, retryAt: now + Backoff.after(misses: misses + 1), misses: misses + 1, at: at)
    }

    private mutating func tick(now: Duration, into actions: inout [LinkAction]) {
        switch state {
        case let .latest(due, _, _) where due <= now:
            lost(why: "the producer didn't answer the latest read within \(Self.within)", refused: false, now: now,
                into: &actions)
        case let .opening(_, due, _, _) where due <= now:
            lost(why: "the watch didn't open within \(Self.within)", refused: false, now: now, into: &actions)
        case let .up(_, silentBy, _) where silentBy <= now:
            lost(why: "no event or heartbeat within \(silence): the producer is gone", refused: false, now: now,
                into: &actions)
        case let .down(_, retryAt, misses, at) where retryAt <= now:
            if let at { open(from: at, misses: misses, now: now, into: &actions) }
            else { latest(misses: misses, now: now, into: &actions) }
        case let .refused(_, retryAt, at) where retryAt <= now:
            if let at { open(from: at, misses: 0, now: now, into: &actions) }
            else { latest(misses: 0, now: now, into: &actions) }
        case .idle, .latest, .opening, .up, .down, .refused:
            break
        }
    }
}
