// The watch link's host (ergon ERGON-UI.md the-facts-watch-wakes-the-glass-by-cursor): it runs each action as a task,
// hands each answer back as the link's event, arms the one deadline on the handed clock, and decides nothing. Time
// moves only on a ManualClock; nothing here sleeps on a product timer (rule 3).

import AnemnezClockTesting
import GRPCCore
import Foundation
import Synchronization
import Testing

@testable import AnemnezEvents

private func p(_ offset: UInt64) -> Position { Position(log: "facts", offset: offset) }
private func e(_ offset: UInt64) -> WatchedEvent { WatchedEvent(offset: offset, subject: "book/K", kind: "moved") }

/// A codec over a toy wire, so the host's rows read no real bytes: a frame `beat:<n>` or `events:<n>` (one event at
/// n, next n+1), and a watch request that is its `from` offset.
private struct Toy: EventsCodec {
    func latestRequest(prefix: String) throws(EventsFailure) -> Data { Data(prefix.utf8) }
    func latestReply(_ reply: Data) throws(EventsFailure) -> (events: [WatchedEvent], asOf: Position) {
        let at = UInt64(String(decoding: reply, as: UTF8.self))!
        return ([e(at - 1)], p(at))
    }
    func watchRequest(from: Position, beat: Duration) throws(EventsFailure) -> Data { Data("\(from.offset)".utf8) }
    func frame(_ bytes: Data) throws(EventsFailure) -> Frame {
        let parts = String(decoding: bytes, as: UTF8.self).split(separator: ":")
        let n = UInt64(parts[1])!
        return parts[0] == "beat" ? .beat(next: p(n)) : .events([e(n)], next: p(n + 1))
    }
}

/// A transport that answers Latest with `asOf`, and each open in turn with its scripted frames, then its end: held
/// open until cancelled (nil), or ended for renewal.
private final class Scripted: EventsTransport, Sendable {
    let asOf: UInt64
    let opens: [(frames: [String], end: WatchEnd?)]
    /// Each open's `from`, in order, and how many were cancelled.
    let seen = Mutex<(from: [String], cancelled: Int)>(([], 0))

    init(asOf: UInt64, opens: [(frames: [String], end: WatchEnd?)]) {
        self.asOf = asOf
        self.opens = opens
    }

    func call(_ request: Data) async throws(EventsFailure) -> Data { Data("\(asOf)".utf8) }

    func watch(
        _ request: Data, opened: @escaping @Sendable () -> Void, frame: @escaping @Sendable (Data) -> Void
    ) async throws(EventsFailure) -> WatchEnd {
        let index = seen.withLock { seen in
            seen.from.append(String(decoding: request, as: UTF8.self))
            return seen.from.count - 1
        }
        opened()
        let script = index < opens.count ? opens[index] : (frames: [], end: nil)
        for bytes in script.frames { frame(Data(bytes.utf8)) }
        if let end = script.end { return end }
        // Held open, as a quiet stream is, until the host closes it.
        let (quiet, held) = AsyncStream.makeStream(of: Never.self)
        defer { held.finish() }
        for await _ in quiet {}
        seen.withLock { $0.cancelled += 1 }
        throw .unclear("cancelled")
    }
}

/// The rows' hang guard: whichever comes first, the work or 30 s of real time.
private func guarded<T: Sendable>(_ what: String, _ work: @escaping @Sendable () async -> T?) async -> T? {
    let once = Mutex(false)
    let got: T? = await withCheckedContinuation { (done: CheckedContinuation<T?, Never>) in
        let finish = { @Sendable (r: T?) in
            if once.withLock({ (fired: inout Bool) -> Bool in defer { fired = true }; return !fired }) { done.resume(returning: r) }
        }
        Task { finish(await work()) }
        Task {
            try? await ContinuousClock().sleep(for: .seconds(30))
            finish(nil)
        }
    }
    if got == nil { Issue.record("the row's hang guard: \(what)") }
    return got
}

/// What the host said, in order, until `n` things were said.
private func said<C: Clock<Duration>>(_ host: LinkHost<C>, _ n: Int) async -> [LinkAction] {
    await guarded("\(n) said") {
        var got: [LinkAction] = []
        for await action in host.said {
            got.append(action)
            if got.count == n { return got }
        }
        return nil
    } ?? []
}

/// Run a host over `transport` until four things are said, and answer them with each open's `from`.
private func ran(_ transport: Scripted) async -> (said: [LinkAction], from: [String]) {
    let host = LinkHost(link: WatchLink(prefix: "book/"), codec: Toy(), transport: transport, clock: ManualClock())
    let running = Task { await host.run() }
    defer { running.cancel() }
    let got = await said(host, 4)
    return (got, transport.seen.withLock { $0.from })
}

@Suite struct LinkHostRows {
    /// Start reads Latest, hands its events, opens from its asOf, says up, and hands each frame's events in order.
    @Test func startReadsLatestThenWatchesAndHands() async {
        let run = await ran(Scripted(asOf: 4, opens: [(frames: ["beat:4", "events:4", "events:5"], end: nil)]))
        #expect(run.said == [.events([e(3)]), .up, .events([e(4)]), .events([e(5)])])
        #expect(run.from == ["4"])
    }

    /// A renewal reopens from the cursor at once, saying neither down nor up again.
    @Test func aRenewalReopensFromTheCursorUnsaid() async {
        let run = await ran(
            Scripted(asOf: 4, opens: [(frames: ["events:4"], end: .renew), (frames: ["events:5"], end: nil)]))
        #expect(run.said == [.events([e(3)]), .up, .events([e(4)]), .events([e(5)])])
        #expect(run.from == ["4", "5"])
    }

    /// Rule 4 through the host: a stream silent past the beat plus the hop is down by name at the link's own deadline,
    /// on the handed clock, and the host cancels it.
    @Test func aSilentStreamIsDownAndClosed() async {
        let clock = ManualClock()
        let transport = Scripted(asOf: 4, opens: [])
        let host = LinkHost(link: WatchLink(prefix: "book/"), codec: Toy(), transport: transport, clock: clock)
        let running = Task { await host.run() }
        defer { running.cancel() }
        #expect(await said(host, 2) == [.events([e(3)]), .up])
        _ = await guarded("the host's deadline timer parked") { await clock.parked(1); return true }
        clock.advance(by: WatchLink(prefix: "book/").silence)
        #expect(await said(host, 1) == [.down("no event or heartbeat within 15.0 seconds: the producer is gone")])
        _ = await guarded("the silent stream cancelled") {
            while transport.seen.withLock({ $0.cancelled }) == 0 { await Task.yield() }
            return true
        }
    }
}

@Suite struct GRPCTransportRows {
    /// A watch ends for renewal only by its typed trailer's value, never its message; Unimplemented is refused by the
    /// face's name, anything else Unclear with its code.
    @Test func renewalAndRefusalAreReadByValue() {
        #expect(EventsFace.renews(["anemnez-end": "watch-renew"]))
        #expect(!EventsFace.renews(["anemnez-end": "expired"]))
        #expect(!EventsFace.renews([:]))
        let unserved = EventsFace.failure("Events/Watch", EventsFace.watchUnserved, RPCError(code: .unimplemented, message: "no"))
        #expect(unserved == .refused("watch-unserved: Events/Watch is not served here: no"))
        let down = EventsFace.failure("Events/Call", EventsFace.callUnserved, RPCError(code: .unavailable, message: "gone"))
        #expect(down == .unclear("Events/Call: unavailable: gone"))
    }

    /// The four names are the crate's own, read from the shared vector a Rust row holds to its constants (D m/38356).
    @Test func theFaceNamesAreTheCratesOwn() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let data = try Data(contentsOf: root.appendingPathComponent("testdata/events-envelope.json"))
        let vector = try JSONSerialization.jsonObject(with: data) as! [String: String]
        #expect(vector["face_end_key"] == EventsFace.endKey)
        #expect(vector["face_watch_renew"] == EventsFace.renewValue)
        #expect(vector["face_events_call_unserved"] == EventsFace.callUnserved)
        #expect(vector["face_watch_unserved"] == EventsFace.watchUnserved)
    }
}
