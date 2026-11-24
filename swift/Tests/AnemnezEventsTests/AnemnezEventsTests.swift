// AnemnezEvents' rows (ERGON-UI.md §4a; M m/32875, m/33097): the envelope's proto is the crate's own, and the Swift
// side carries the wire's bytes exactly as -serve's prost does, read from the one shared vector.

import Foundation
import Testing

@testable import AnemnezEvents

private let swiftDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
    .deletingLastPathComponent()

private func unhex(_ text: String) -> Data {
    var data = Data()
    var index = text.startIndex
    while index < text.endIndex {
        let next = text.index(index, offsetBy: 2)
        data.append(UInt8(text[index..<next], radix: 16)!)
        index = next
    }
    return data
}

@Suite struct ProtoTests {
    /// There is one proto: the package's is a link to the crate's, never a copy.
    @Test func theProtoIsTheCratesOwn() throws {
        let rel = "anemnez/extension/v1/events.proto"
        let link = swiftDir.appendingPathComponent("Sources/AnemnezEvents/Proto/\(rel)")
        let original = swiftDir.deletingLastPathComponent().appendingPathComponent("crates/anemnez-extension/proto/\(rel)")
        let attrs = try FileManager.default.attributesOfItem(atPath: link.path)
        #expect(attrs[.type] as? FileAttributeType == .typeSymbolicLink, "the Swift proto must stay a symlink")
        #expect(link.resolvingSymlinksInPath().path == original.resolvingSymlinksInPath().path)
    }
}

@Suite struct EnvelopeTests {
    /// The shared vector (testdata/events-envelope.json), which a Rust row holds to prost's encoding: Swift encodes
    /// each request to those exact bytes, and decodes each reply and frame from them to the same payload.
    @Test func theEnvelopeCarriesTheBytesAsProstDoes() throws {
        let path = swiftDir.deletingLastPathComponent().appendingPathComponent("testdata/events-envelope.json")
        let vector = try JSONSerialization.jsonObject(with: Data(contentsOf: path)) as! [String: String]
        let payload = unhex(vector["payload_hex"]!)
        #expect(try Envelope.callRequest(payload) == unhex(vector["call_request_hex"]!))
        #expect(try Envelope.watchRequest(payload) == unhex(vector["watch_request_hex"]!))
        #expect(try Envelope.callReply(unhex(vector["call_reply_hex"]!)) == payload)
        #expect(try Envelope.watchFrame(unhex(vector["watch_frame_hex"]!)) == payload)
        #expect(try Envelope.callRequest(Data()) == unhex(vector["empty_payload_hex"]!), "an empty payload")
    }
}
