// The watch link's thin host (house rule 1; ergon ERGON-UI.md the-facts-watch-wakes-the-glass-by-cursor): it waits for
// the next event (a Latest answer, a stream's open, frame, end or failure, or the machine's deadline), steps the link
// with `now` from the handed clock, starts each action as a task of its own, and re-arms the one deadline. It never
// awaits an action before its next event, and it decides nothing: what a frame means is the link's, and what an event
// means is its consumer's. The wire's bytes are read and written by the codec (hydramq-message-ffi's, D m/33118), and
// carried by the transport (Events/Call and Events/Watch on the product's channel).

import Foundation

/// Why a call or a stream failed: refused by name (a typed refusal, such as Events unserved), or not answered.
public enum EventsFailure: Error, Equatable, Sendable {
    case refused(String)
    case unclear(String)
}

/// How a watch's stream ended without failing.
public enum WatchEnd: Equatable, Sendable {
    /// The producer ended it.
    case ended
    /// The producer ended it for renewal, by its typed trailer (`anemnez-end: watch-renew`): open it again.
    case renew
}

/// The codec, as a face reads and writes the Events wire: hydramq-message-ffi's, never a Swift copy (D m/33118).
public protocol EventsCodec: Sendable {
    /// A Latest request for every subject under `prefix`.
    func latestRequest(prefix: String) throws(EventsFailure) -> Data
    /// A Latest reply: each subject's latest event, and the position they're as of.
    func latestReply(_ reply: Data) throws(EventsFailure) -> (events: [WatchedEvent], asOf: Position)
    /// A watch request from `from`, a heartbeat every `beat`.
    func watchRequest(from: Position, beat: Duration) throws(EventsFailure) -> Data
    /// One watch frame, as frame_reply reads it.
    func frame(_ bytes: Data) throws(EventsFailure) -> Frame
}

/// The transport: a product's Events/Call and Events/Watch.
public protocol EventsTransport: Sendable {
    /// One Events/Call: the wire request in, the producer's wire reply out, within the transport's own call bound.
    func call(_ request: Data) async throws(EventsFailure) -> Data
    /// Events/Watch, held until it ends or the task is cancelled: `opened` once the producer accepts it, then each
    /// frame's wire bytes to `frame`, in order.
    func watch(
        _ request: Data, opened: @escaping @Sendable () -> Void, frame: @escaping @Sendable (Data) -> Void
    ) async throws(EventsFailure) -> WatchEnd
}

/// The host: the link, its clock, and what it says to its consumer.
public actor LinkHost<C: Clock<Duration>> {
    /// What the host hands itself: the link's events, or a stream's renewal (stepped with the link's own cursor).
    private enum Input: Sendable {
        case event(LinkEvent)
        case renewed(seq: UInt64)
    }

    private var link: WatchLink
    private let codec: any EventsCodec
    private let transport: any EventsTransport
    private let clock: C
    private let origin: C.Instant
    private let inbox: AsyncStream<Input>
    private let inputs: AsyncStream<Input>.Continuation
    /// What the link says: `up`, `down(why)` and `events`, in order, for the consumer.
    public nonisolated let said: AsyncStream<LinkAction>
    private let saying: AsyncStream<LinkAction>.Continuation
    /// The read or stream out, by its number: a close cancels it, a newer one cancels any older (only the one out is
    /// in flight), and run()'s end cancels all (M m/38357).
    private var inFlight: [UInt64: Task<Void, Never>] = [:]

    public init(link: WatchLink, codec: any EventsCodec, transport: any EventsTransport, clock: C) {
        self.link = link
        self.codec = codec
        self.transport = transport
        self.clock = clock
        origin = clock.now
        (said, saying) = AsyncStream.makeStream(of: LinkAction.self)
        (inbox, inputs) = AsyncStream.makeStream(of: Input.self)
        inputs.yield(.event(.start))
    }

    /// Run until cancelled: start, then each event in the order it came.
    public func run() async {
        var deadline: Task<Void, Never>?
        for await input in inbox {
            let event: LinkEvent
            switch input {
            case let .event(given): event = given
            case let .renewed(seq):
                // A renewal steps with the link's own cursor. Only the stream out counts, and one out always has a
                // cursor; without one, there is nothing to resume, so nothing is made up (D m/38356).
                guard let at = link.at else { continue }
                event = .frame(seq: seq, .renew(next: at))
            }
            for action in link.step(event, now: now()) { start(action) }
            deadline?.cancel()
            deadline = link.deadline().map(arm)
            if Task.isCancelled { break }
        }
        deadline?.cancel()
        for task in inFlight.values { task.cancel() }
        saying.finish()
    }

    private func now() -> Duration { origin.duration(to: clock.now) }

    private func arm(_ at: Duration) -> Task<Void, Never> {
        let (clock, origin, inputs) = (clock, origin, inputs)
        return Task {
            do { try await clock.sleep(until: origin.advanced(by: at), tolerance: nil) } catch { return }
            inputs.yield(.event(.tick))
        }
    }

    private func start(_ action: LinkAction) {
        let (codec, transport, inputs) = (codec, transport, inputs)
        switch action {
        case .up, .down, .events:
            saying.yield(action)
        case let .close(seq):
            inFlight.removeValue(forKey: seq)?.cancel()
        case let .latest(prefix, seq):
            supersede(below: seq)
            inFlight[seq] = Task {
                do throws(EventsFailure) {
                    let reply = try await transport.call(try codec.latestRequest(prefix: prefix))
                    let (events, asOf) = try codec.latestReply(reply)
                    inputs.yield(.event(.latestAnswered(seq: seq, events, asOf: asOf)))
                } catch {
                    inputs.yield(.event(Self.failed(seq, error)))
                }
            }
        case let .open(from, beat, seq):
            supersede(below: seq)
            inFlight[seq] = Task {
                do throws(EventsFailure) {
                    let end = try await transport.watch(
                        try codec.watchRequest(from: from, beat: beat),
                        opened: { inputs.yield(.event(.opened(seq: seq))) },
                        frame: { bytes in
                            do throws(EventsFailure) {
                                inputs.yield(.event(.frame(seq: seq, try codec.frame(bytes))))
                            } catch {
                                inputs.yield(.event(Self.failed(seq, error)))
                            }
                        })
                    switch end {
                    case .ended: inputs.yield(.event(.ended(seq: seq)))
                    case .renew: inputs.yield(.renewed(seq: seq))
                    }
                } catch {
                    inputs.yield(.event(Self.failed(seq, error)))
                }
            }
        }
    }

    /// Cancel every read or stream numbered below `seq`: the link awaits only the one it just asked.
    private func supersede(below seq: UInt64) {
        for (older, task) in inFlight where older < seq {
            task.cancel()
            inFlight[older] = nil
        }
    }

    private static func failed(_ seq: UInt64, _ error: EventsFailure) -> LinkEvent {
        switch error {
        case let .refused(why): .failed(seq: seq, refused: true, why: why)
        case let .unclear(why): .failed(seq: seq, refused: false, why: why)
        }
    }
}
