//! `Describe/Get`'s answer, made once (CONTRACT.md `#one-describe-reply-for-every-product`; D, E, L,
//! H, 2026-11-14): the descriptor and its digest, so every product, the serving face, Prova and
//! Archetect alike, answer with the same bytes and a caller's re-fetch rule reads the same digest.
//! Pure: a product mounts it with a three-line adapter onto its own generated `Describe` service.

use prost::Message;
use sha2::{Digest, Sha256};

/// `anemnez.extension.v1.DescribeReply`, wire for wire (a parity test holds it to the proto).
#[derive(Clone, PartialEq, Eq, Message)]
pub struct DescribeReply {
    /// The extension's `FileDescriptorSet`, encoded, as its build emitted it.
    #[prost(bytes = "vec", tag = "1")]
    pub file_descriptor_set: Vec<u8>,
    /// Its SHA-256, lowercase hex, so a caller re-fetches only when it changes.
    #[prost(string, tag = "2")]
    pub digest: String,
}

/// The descriptor's digest: SHA-256 of its encoded bytes, lowercase hex.
#[must_use]
pub fn digest(file_descriptor_set: &[u8]) -> String {
    Sha256::digest(file_descriptor_set).iter().map(|b| format!("{b:02x}")).collect()
}

/// The reply `Describe/Get` answers for an encoded `FileDescriptorSet`.
#[must_use]
pub fn reply(file_descriptor_set: Vec<u8>) -> DescribeReply {
    let digest = digest(&file_descriptor_set);
    DescribeReply { file_descriptor_set, digest }
}

impl DescribeReply {
    /// The reply's wire bytes, for a product whose server answers raw bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.encode_to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CONTRACT.md one-describe-reply-for-every-product: the descriptor comes back as given, with
    /// its SHA-256 in lowercase hex, and the bytes on the wire are the proto's.
    #[test]
    fn one_describe_reply_for_every_product() {
        let reply = reply(b"abc".to_vec());
        assert_eq!(reply.file_descriptor_set, b"abc");
        assert_eq!(reply.digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // Field 1 (bytes) then field 2 (string), each length-delimited.
        let mut want = vec![0x0a, 3];
        want.extend_from_slice(b"abc");
        want.extend_from_slice(&[0x12, 64]);
        want.extend_from_slice(reply.digest.as_bytes());
        assert_eq!(reply.to_bytes(), want);
        assert_eq!(DescribeReply::decode(want.as_slice()).expect("decodes"), reply);
    }
}
