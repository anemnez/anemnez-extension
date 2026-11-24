// The Events face's gRPC envelope (anemnez/extension/v1/events.proto), generated from the crate's own proto.
//
// Call and Watch carry one hydramq-events wire message each way as opaque bytes: a request in, a reply or a frame
// out. The envelope only carries them. What the bytes say is the Rust codec's, through hydramq-message-ffi (A's),
// never decoded here by hand (D m/33118). This file is the envelope alone: what crosses the gRPC boundary, held to
// -serve's own prost encoding by a shared vector (testdata/events-envelope.json).

import Foundation
import SwiftProtobuf

/// The envelope's three carriers, as plain values a face holds.
public enum Envelope {
    /// A Call's request: one hydramq-events wire request (a read, or a latest), as the codec encoded it.
    public static func callRequest(_ request: Data) throws -> Data {
        var message = Anemnez_Extension_V1_CallRequest()
        message.request = request
        return try message.serializedData()
    }

    /// A Call's reply: the producer's wire answer (a refusal included), for the codec to decode.
    public static func callReply(_ serialized: Data) throws -> Data {
        try Anemnez_Extension_V1_CallReply(serializedBytes: serialized).reply
    }

    /// A Watch's request: one hydramq-events wire watch request.
    public static func watchRequest(_ request: Data) throws -> Data {
        var message = Anemnez_Extension_V1_WatchRequest()
        message.request = request
        return try message.serializedData()
    }

    /// One Watch frame: the producer's wire frame, for the codec to decode.
    public static func watchFrame(_ serialized: Data) throws -> Data {
        try Anemnez_Extension_V1_WatchFrame(serializedBytes: serialized).frame
    }
}
