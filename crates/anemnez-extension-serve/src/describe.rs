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
//! is shown, decided by its own engine. -serve never decides it, but it makes the answer whole: one entry per
//! offered method, in the descriptor's order, any method the function didn't name going out hidden
//! ([`NOT_STATED`]) and any entry for a path the descriptor doesn't offer dropped. So a per-caller reply is empty
//! only when nothing is offered, and empty never stands for "show everything" (D m/15074, E m/15072). The digest
//! stays the descriptor's alone, and fabricd's peer, which has no principal, always gets the list empty, without
//! the function being asked.

use std::sync::Arc;

use anemnez_extension::describe::{MethodShown, Seen};
use anemnez_extension::descriptor::DescriptorRefusal;
use anemnez_extension::peer::Role;

use crate::{judged, pb};

/// What a caller is shown of each offered method, decided by the product's own engine from the call (its judged
/// peer and its principal are on the request). It answers from what the product already holds: no I/O here.
pub type Shown = Arc<dyn Fn(&tonic::Request<pb::DescribeRequest>) -> Vec<MethodShown> + Send + Sync>;

/// Why an offered method the product's function didn't name is hidden.
pub const NOT_STATED: &str = "not stated by the product";

/// The `Describe` service over one descriptor.
#[derive(Clone)]
pub struct Describe {
    reply: pb::DescribeReply,
    shown: Option<Shown>,
    /// The admitted descriptor's offered methods, in its order: set by [`Describe::per_caller`], which refuses a
    /// descriptor `check_descriptor` doesn't admit.
    offered: Arc<Vec<String>>,
}

impl Describe {
    /// The encoded `FileDescriptorSet` it serves.
    #[must_use]
    pub fn descriptor(&self) -> &[u8] {
        &self.reply.file_descriptor_set
    }

    /// The service for an encoded `FileDescriptorSet`, answering every caller alike (an empty `shown`).
    pub fn new(file_descriptor_set: Vec<u8>) -> Describe {
        let reply = anemnez_extension::describe::reply(file_descriptor_set);
        let reply = pb::DescribeReply {
            file_descriptor_set: reply.file_descriptor_set,
            digest: reply.digest,
            shown: Vec::new(),
            registry: None,
        };
        Describe { reply, shown: None, offered: Arc::new(Vec::new()) }
    }

    /// The same service, answering each caller what `shown` says it is shown, made whole over the descriptor's
    /// offered methods. fabricd's peer is never asked about: it gets the list empty.
    ///
    /// # Errors
    /// The descriptor's refusals, when `check_descriptor` doesn't admit it: a per-caller answer is made whole
    /// over the offered methods, so one with no admitted descriptor would come out empty and read as "show
    /// everything" (D m/15110). It is refused here instead, never defaulted.
    pub fn per_caller(self, shown: Shown) -> Result<Describe, Vec<DescriptorRefusal>> {
        let methods = anemnez_extension::descriptor::check_descriptor(&self.reply.file_descriptor_set)?;
        let offered = Arc::new(methods.into_iter().map(|m| m.path).collect());
        Ok(Describe { shown: Some(shown), offered, ..self })
    }

    /// The descriptor's SHA-256, lowercase hex.
    pub fn digest(&self) -> &str {
        &self.reply.digest
    }
}

/// One entry per `offered` method, in its order: the product's own where it named the method (its first, if it
/// named one twice), else hidden as [`NOT_STATED`]. An entry for a path not offered is dropped.
fn whole(offered: &[String], said: Vec<MethodShown>) -> Vec<MethodShown> {
    let mut said: Vec<Option<MethodShown>> = said.into_iter().map(Some).collect();
    offered
        .iter()
        .map(|path| {
            said.iter_mut()
                .find(|entry| entry.as_ref().is_some_and(|e| &e.path == path))
                .and_then(Option::take)
                .unwrap_or_else(|| MethodShown::hidden(path.clone(), NOT_STATED))
        })
        .collect()
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
            reply.shown = whole(&self.offered, shown(&request)).into_iter().map(wire).collect();
        }
        Ok(tonic::Response::new(reply))
    }
}
