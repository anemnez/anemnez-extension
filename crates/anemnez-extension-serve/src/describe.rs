//! Describe: the extension's descriptor, served by the extension itself
//! (CONTRACT.md `#a-product-describes-itself-from-its-idl`), and what each caller is shown of it
//! (`describe-answers-per-caller`).
//!
//! The descriptor is the extension's own `FileDescriptorSet`, as its build emits it
//! (`tonic_build::configure().file_descriptor_set_path(..)`), with its SHA-256 so a caller
//! re-fetches only when it changes. The reply is the kernel's `describe::reply`, so this face and
//! every product serving Describe itself answer alike (CONTRACT.md `#one-describe-reply-for-every-product`).
//!
//! A product that answers per caller hands [`Describe::per_caller`] a function from the call to what that caller
//! is shown, decided by its own engine. -serve never decides it. The digest stays the descriptor's alone, and
//! fabricd's peer, which has no principal, always gets the list empty, without the function being asked.

use std::sync::Arc;

use anemnez_extension::describe::{MethodShown, Seen};
use anemnez_extension::peer::Role;

use crate::{judged, pb};

/// What a caller is shown of each offered method, decided by the product's own engine from the call (its judged
/// peer and its principal are on the request). It answers from what the product already holds: no I/O here.
pub type Shown = Arc<dyn Fn(&tonic::Request<pb::DescribeRequest>) -> Vec<MethodShown> + Send + Sync>;

/// The `Describe` service over one descriptor.
#[derive(Clone)]
pub struct Describe {
    reply: pb::DescribeReply,
    shown: Option<Shown>,
}

impl Describe {
    /// The service for an encoded `FileDescriptorSet`, answering every caller alike (an empty `shown`).
    pub fn new(file_descriptor_set: Vec<u8>) -> Describe {
        let reply = anemnez_extension::describe::reply(file_descriptor_set);
        let reply = pb::DescribeReply {
            file_descriptor_set: reply.file_descriptor_set,
            digest: reply.digest,
            shown: Vec::new(),
        };
        Describe { reply, shown: None }
    }

    /// The same service, answering each caller what `shown` says it is shown. fabricd's peer is never asked
    /// about: it gets the list empty.
    #[must_use]
    pub fn per_caller(self, shown: Shown) -> Describe {
        Describe { shown: Some(shown), ..self }
    }

    /// The descriptor's SHA-256, lowercase hex.
    pub fn digest(&self) -> &str {
        &self.reply.digest
    }
}

/// The kernel's entry as the generated type, field for field.
fn wire(entry: MethodShown) -> pb::MethodShown {
    let seen = entry.seen.map(|seen| match seen {
        Seen::Hidden(_) => pb::method_shown::Seen::Hidden(pb::Hidden {}),
        Seen::Shown(s) => pb::method_shown::Seen::Shown(pb::Shown { presence: s.presence, permission: s.permission }),
    });
    pb::MethodShown { path: entry.path, seen, why: entry.why }
}

#[tonic::async_trait]
impl pb::describe_server::Describe for Describe {
    async fn get(
        &self,
        request: tonic::Request<pb::DescribeRequest>,
    ) -> Result<tonic::Response<pb::DescribeReply>, tonic::Status> {
        let mut reply = self.reply.clone();
        let fabricd = judged(&request).is_some_and(|j| j.role == Role::Fabricd);
        if let (Some(shown), false) = (&self.shown, fabricd) {
            reply.shown = shown(&request).into_iter().map(wire).collect();
        }
        Ok(tonic::Response::new(reply))
    }
}
