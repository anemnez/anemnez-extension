//! Describe: the extension's descriptor, served by the extension itself
//! (CONTRACT.md `#a-product-describes-itself-from-its-idl`).
//!
//! The descriptor is the extension's own `FileDescriptorSet`, as its build emits it
//! (`tonic_build::configure().file_descriptor_set_path(..)`), with its SHA-256 so a caller
//! re-fetches only when it changes.

use sha2::{Digest, Sha256};

use crate::pb;

/// The `Describe` service over one descriptor.
#[derive(Clone)]
pub struct Describe {
    reply: pb::DescribeReply,
}

impl Describe {
    /// The service for an encoded `FileDescriptorSet`.
    pub fn new(file_descriptor_set: Vec<u8>) -> Describe {
        let digest = Sha256::digest(&file_descriptor_set).iter().map(|b| format!("{b:02x}")).collect();
        Describe { reply: pb::DescribeReply { file_descriptor_set, digest } }
    }

    /// The descriptor's SHA-256, lowercase hex.
    pub fn digest(&self) -> &str {
        &self.reply.digest
    }
}

#[tonic::async_trait]
impl pb::describe_server::Describe for Describe {
    async fn get(
        &self,
        _request: tonic::Request<pb::DescribeRequest>,
    ) -> Result<tonic::Response<pb::DescribeReply>, tonic::Status> {
        Ok(tonic::Response::new(self.reply.clone()))
    }
}
