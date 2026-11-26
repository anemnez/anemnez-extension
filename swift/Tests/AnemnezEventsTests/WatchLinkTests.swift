// The watch link (ergon ERGON-UI.md, the-facts-watch-wakes-the-glass-by-cursor): one row per transition on handed
// instants, and a walk of event orders checking that one read or stream is out, numbered past every earlier one; that a
// frame or failure counts only for it; that the cursor never moves back and nothing below it is handed; and that `up`
// is said only as a stream opens.

import Testing

@testable import AnemnezEvents

private func s(_ n: Int64) -> Duration { .seconds(n) }
private func p(_ offset: UInt64) -> Position { Position(log: "facts", offset: offset) }
private func e(_ offset: UInt64) -> WatchedEvent { WatchedEvent(offset: offset, subject: "book/K", kind: "moved") }

/// A link that's up at `at`, stream number returned.
private func upAt(_ at: UInt64, now: Duration = s(0)) -> (WatchLink, UInt64) {
    var link = WatchLink(prefix: "book/")
    _ = link.step(.start, now: now)
    _ = link.step(.latestAnswered(seq: 1, [], asOf: p(at)), now: now)
    _ = link.step(.opened(seq: 2), now: now)
    return (link, 2)
}

@Suite struct WatchLinkTransitions {
    /// Start reads Latest; its answer hands each subject's latest and opens from its asOf; the open says up once.
    @Test func startReadsLatestThenWatchesFromItsAsOf() {
        var link = WatchLink(prefix: "book/")
        #expect(link.step(.start, now: s(0)) == [.latest(prefix: "book/", seq: 1)])
        #expect(link.step(.start, now: s(0)) == [], "one start")
        #expect(link.deadline() == WatchLink.within)
        let opened = link.step(.latestAnswered(seq: 1, [e(3)], asOf: p(4)), now: s(1))
        #expect(opened == [.events([e(3)]), .open(from: p(4), beat: .seconds(10), seq: 2)])
        #expect(link.step(.opened(seq: 2), now: s(2)) == [.up])
        #expect(link.step(.opened(seq: 2), now: s(2)) == [], "up is said once per open")
        #expect(link.deadline() == s(2) + link.silence)
        #expect(link.silence == .seconds(15), "the beat asked plus hydramq's HOP_MARGIN")
    }

    /// Beats and events move the cursor and the silence bound; only events at or past the cursor are handed.
    @Test func framesMoveTheCursorAndHandEachEventOnce() {
        var (link, seq) = upAt(4)
        #expect(link.step(.frame(seq: seq, .beat(next: p(4))), now: s(5)) == [])
        #expect(link.deadline() == s(5) + link.silence)
        #expect(link.step(.frame(seq: seq, .events([e(4), e(5)], next: p(6))), now: s(6)) == [.events([e(4), e(5)])])
        #expect(link.at == p(6))
        #expect(link.step(.frame(seq: seq, .events([e(5), e(6)], next: p(7))), now: s(7)) == [.events([e(6)])])
        #expect(link.step(.frame(seq: seq, .beat(next: p(3))), now: s(8)) == [])
        #expect(link.at == p(7), "the cursor never moves back")
    }

    /// A renewal reopens from its next at once, saying neither down nor up again (M m/38173): a consumer that re-lists
    /// on up does so once per return, never once per token life. A frame of the old stream then moves nothing.
    @Test func aRenewalReopensWithoutSayingDown() {
        var (link, seq) = upAt(4)
        let renewed = link.step(.frame(seq: seq, .renew(next: p(9))), now: s(3))
        #expect(renewed == [.close(seq: 2), .open(from: p(9), beat: .seconds(10), seq: 3)])
        #expect(link.step(.frame(seq: seq, .events([e(9)], next: p(10))), now: s(3)) == [], "the old stream's frame")
        #expect(link.step(.frame(seq: 3, .events([e(9)], next: p(10))), now: s(4)) == [.events([e(9)])])
        _ = link.step(.frame(seq: 3, .renew(next: p(10))), now: s(5))
        #expect(link.step(.opened(seq: 4), now: s(5)) == [], "renewed and opened: still no second up")
    }

    /// A down, then a reopen, says up once again.
    @Test func aReturnAfterDownSaysUpOnce() {
        var (link, seq) = upAt(4)
        _ = link.step(.ended(seq: seq), now: s(1))
        #expect(link.step(.tick, now: s(1) + Backoff.first) == [.open(from: p(4), beat: .seconds(10), seq: 3)])
        #expect(link.step(.opened(seq: 3), now: s(3)) == [.up])
    }

    /// A compaction says down by name and reads Latest again: the gap is read by value, never skipped.
    @Test func aCompactionSaysDownAndReadsLatest() {
        var (link, seq) = upAt(4)
        let compacted = link.step(.frame(seq: seq, .compacted(floor: p(20))), now: s(3))
        #expect(compacted == [.close(seq: 2), .down("compacted past this watch"), .latest(prefix: "book/", seq: 3)])
        #expect(link.step(.latestAnswered(seq: 3, [e(21)], asOf: p(22)), now: s(4))
            == [.events([e(21)]), .open(from: p(22), beat: .seconds(10), seq: 4)])
    }

    /// Silence past the beat plus the hop is down by name; it reopens from the cursor on the backoff, which grows.
    @Test func silenceIsDownAndReopensFromTheCursor() {
        var (link, _) = upAt(4)
        _ = link.step(.frame(seq: 2, .events([e(4)], next: p(5))), now: s(0))
        let silent = link.step(.tick, now: link.silence)
        #expect(silent == [.close(seq: 2), .down("no event or heartbeat within 15.0 seconds: the producer is gone")])
        #expect(link.deadline() == link.silence + Backoff.first)
        #expect(link.step(.tick, now: link.silence + Backoff.first) == [.open(from: p(5), beat: .seconds(10), seq: 3)])
        let unopened = link.step(.tick, now: link.silence + Backoff.first + WatchLink.within)
        #expect(unopened == [.close(seq: 3), .down("the watch didn't open within 10.0 seconds")])
        #expect(link.state == .down(
            why: "the watch didn't open within 10.0 seconds", retryAt: link.silence + Backoff.first + WatchLink.within
                + .seconds(2), misses: 2, at: p(5)))
    }

    /// A typed refusal (Events unserved) waits the longest backoff; an end or a failure is down; a Latest that fails
    /// is retried as Latest, having no cursor.
    @Test func refusalsEndsAndFailuresAreSaid() {
        var link = WatchLink(prefix: "book/")
        _ = link.step(.start, now: s(0))
        #expect(link.step(.failed(seq: 1, refused: true, why: "events-call-unserved"), now: s(1)) == [.down("events-call-unserved")])
        #expect(link.deadline() == s(1) + Backoff.atMost)
        #expect(link.step(.tick, now: s(61)) == [.latest(prefix: "book/", seq: 2)])
        var (up, seq) = upAt(4)
        #expect(up.step(.ended(seq: seq), now: s(1)) == [.close(seq: 2), .down("the producer ended the watch")])
        #expect(up.step(.failed(seq: seq, refused: false, why: "late"), now: s(2)) == [], "for a stream no longer out")
    }

    /// The backoff doubles from 1 s and stops at 60 s.
    @Test func theBackoffDoublesToItsMost() {
        #expect((1...8).map(Backoff.after(misses:)) == [1, 2, 4, 8, 16, 32, 60, 60].map { Duration.seconds($0) })
    }
}

@Suite struct WatchLinkWalk {
    /// Every order of these events to depth 6, streams numbered 1 to 4 so stale ones are walked too, checking from
    /// outside after each step: at most one read or open asked, numbered past every earlier one, the one before it
    /// closed; anything for a number not out moves nothing; the cursor never moves back but to a Latest's asOf;
    /// nothing below the cursor is handed from a frame; `up` is said only as a stream opens.
    @Test func everyOrderKeepsOneStreamAndTheCursor() {
        let alphabet: [LinkEvent] = [
            .start, .latestAnswered(seq: 1, [e(2)], asOf: p(3)), .latestAnswered(seq: 3, [], asOf: p(9)),
            .opened(seq: 2), .opened(seq: 4),
            .frame(seq: 2, .events([e(2), e(3), e(4)], next: p(5))), .frame(seq: 2, .beat(next: p(1))),
            .frame(seq: 2, .renew(next: p(6))), .frame(seq: 2, .compacted(floor: p(8))),
            .frame(seq: 3, .beat(next: p(7))), .frame(seq: 4, .events([e(5)], next: p(6))),
            .failed(seq: 2, refused: false, why: "x"), .failed(seq: 1, refused: true, why: "no"), .ended(seq: 3), .tick,
        ]
        var walked = 0
        func walk(_ link: WatchLink, depth: Int, now: Duration) {
            guard depth > 0 else { return }
            for event in alphabet {
                var next = link
                let at = event == .tick ? (link.deadline() ?? now) : now
                let actions = next.step(event, now: at)
                walked += 1
                let asked = actions.compactMap { action -> UInt64? in
                    switch action {
                    case let .latest(_, seq), let .open(_, _, seq): seq
                    case .close, .up, .down, .events: nil
                    }
                }
                if asked.count > 1 { Issue.record("two asked in one step, after \(event)"); return }
                if let seq = asked.first {
                    if seq <= link.seq { Issue.record("\(seq) not past \(link.seq), after \(event)"); return }
                    if let was = link.out, case .opening = link.state, !actions.contains(.close(seq: was)) {
                        Issue.record("stream \(was) left open under \(seq), after \(event)"); return
                    }
                    if let was = link.out, case .up = link.state, !actions.contains(.close(seq: was)) {
                        Issue.record("stream \(was) left open under \(seq), after \(event)"); return
                    }
                }
                let numbered: UInt64? =
                    switch event {
                    case let .latestAnswered(seq, _, _), let .opened(seq), let .frame(seq, _), let .failed(seq, _, _),
                        let .ended(seq):
                        seq
                    case .start, .tick: nil
                    }
                if let numbered, numbered != link.out, next != link || !actions.isEmpty {
                    Issue.record("\(numbered) isn't out (\(String(describing: link.out))) but moved the link"); return
                }
                if case .latestAnswered = event {} else if let was = link.at, let now = next.at, now.offset < was.offset {
                    Issue.record("the cursor moved back from \(was.offset) to \(now.offset), after \(event)"); return
                }
                if case .frame = event, let was = link.at {
                    for case let .events(handed) in actions where handed.contains(where: { $0.offset < was.offset }) {
                        Issue.record("handed an event below the cursor \(was.offset), after \(event)"); return
                    }
                }
                if actions.contains(.up) {
                    let wasUp = if case .up = link.state { true } else { false }
                    let isUp = if case .up = next.state { true } else { false }
                    if wasUp || !isUp { Issue.record("up said outside an open, after \(event)"); return }
                    if link.saidUp { Issue.record("up said twice with no down between, after \(event)"); return }
                }
                if let error = next.check() { Issue.record("\(error), after \(event)"); return }
                walk(next, depth: depth - 1, now: at)
            }
        }
        walk(WatchLink(prefix: "book/"), depth: 6, now: .zero)
        #expect(walked > 1_000_000)
    }
}
