// A product's Events/Call and Events/Watch over grpc-swift: the Swift sibling of anemnez-extension-client's EventsCall
// and EventsFrames (CONTRACT.md `#a-products-events-are-read-through-one-call`, `...-pushed-through-one-watch`). It
// carries the wire's bytes in the envelope and reads nothing of them. A product that doesn't serve the face answers
// Unimplemented: refused by name, never Unclear. A watch ended for renewal is read by its typed trailer
// (`anemnez-end: watch-renew`), never its message (A m/25170, D m/25187).

import Foundation
import GRPCCore

/// Events over `client`, each Call within `callWithin`.
public struct GRPCEventsTransport<Transport: ClientTransport>: EventsTransport {
    private let events: Anemnez_Extension_V1_Events.Client<Transport>
    private let callWithin: Duration

    /// `callWithin` is the transport's own bound, inside the link's (WatchLink.within): nested, inner shorter (rule 4).
    public init(client: GRPCClient<Transport>, callWithin: Duration = WatchLink.within - .seconds(1)) {
        events = Anemnez_Extension_V1_Events.Client(wrapping: client)
        self.callWithin = callWithin
    }

    public func call(_ request: Data) async throws(EventsFailure) -> Data {
        var ask = Anemnez_Extension_V1_CallRequest()
        ask.request = request
        var options = CallOptions.defaults
        options.timeout = callWithin
        do {
            return try await events.call(ask, options: options).reply
        } catch {
            throw EventsFace.failure("Events/Call", EventsFace.callUnserved, error)
        }
    }

    public func watch(
        _ request: Data, opened: @escaping @Sendable () -> Void, frame: @escaping @Sendable (Data) -> Void
    ) async throws(EventsFailure) -> WatchEnd {
        var ask = Anemnez_Extension_V1_WatchRequest()
        ask.request = request
        do {
            try await events.watch(ask) { response in
                let contents = try response.accepted.get()
                opened()
                for try await part in contents.bodyParts {
                    switch part {
                    case let .message(message): frame(message.frame)
                    case .trailingMetadata: break
                    }
                }
            }
            return .ended
        } catch let error as RPCError where EventsFace.renews(error.metadata) {
            return .renew
        } catch {
            throw EventsFace.failure("Events/Watch", EventsFace.watchUnserved, error)
        }
    }
}

/// What the transport judges of a gRPC answer, by value: the face's names (anemnez-extension `refusal`).
enum EventsFace {
    /// The trailer that ends a watch by design, and its one value.
    static let endKey = "anemnez-end"
    static let renewValue = "watch-renew"
    static let callUnserved = "events-call-unserved"
    static let watchUnserved = "watch-unserved"

    /// Whether the trailer ends the watch for renewal: by value, never by its message.
    static func renews(_ metadata: Metadata) -> Bool {
        metadata[stringValues: endKey].contains(renewValue)
    }

    /// Unimplemented is a known answer, refused by `name`; anything else is Unclear, said with its code.
    static func failure(_ method: String, _ name: String, _ error: any Error) -> EventsFailure {
        guard let rpc = error as? RPCError else { return .unclear("\(method): \(error)") }
        if rpc.code == .unimplemented { return .refused("\(name): \(method) is not served here: \(rpc.message)") }
        return .unclear("\(method): \(rpc.code): \(rpc.message)")
    }
}
