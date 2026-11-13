//! What a method's options mean to every agent face, once (CONTRACT.md §6; D, C, H, B, 2026-11-10):
//! whether an agent sees it at all, and what its call asks by default. Agora's MCP face and
//! Substrate's Lua mount both call these, so the two surfaces never disagree on what is exposed or
//! what asks. Pure: no clock, no store.

use crate::options::{Callers, MethodOptions, Permission, Presence};

/// Whether an agent face (MCP tools, a Lua namespace) offers a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Offered to an agent: an MCP tool, a Lua function.
    Agent,
    /// Never listed or callable through an agent face: a product's or an owner's method, or one
    /// whose presence is hidden. No row or grant changes this.
    Hidden,
}

/// What an agent face makes of a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exposure {
    pub face: Face,
    /// The method's DEFAULT consent: what a call asks when nothing of the caller's own says more. A
    /// floor, never a ceiling: the product's per-call check may raise a call to Ask (a post to
    /// another user's member) and never lowers one (CONTRACT.md a-methods-default-is-a-floor).
    pub consent: Permission,
}

/// A method's exposure from its options: an agent face for `callers: agent` methods that are not
/// hidden; the consent default is the method's `permission`, and a `person_act` method never answers
/// Allow (an Ask at least).
#[must_use]
pub fn exposure(options: &MethodOptions) -> Exposure {
    let face = if options.callers == Callers::Agent && options.presence != Presence::Hidden {
        Face::Agent
    } else {
        Face::Hidden
    };
    let consent = match options.permission {
        Permission::Allow if options.person_act => Permission::Ask,
        p => p,
    };
    Exposure { face, consent }
}

/// A person's signed grant that loosens one consent (an Iris answer, verified by the caller against
/// the person's device key before it builds this). Only its existence matters here: the kernel never
/// sees a signature, and nothing a Lua or MCP caller sends can make one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersonsGrant {
    _verified: (),
}

impl PersonsGrant {
    /// A grant whose signed answer the caller has verified (`signed::verify`, `signed::fresh`).
    #[must_use]
    pub fn verified() -> Self {
        Self { _verified: () }
    }
}

fn strictness(p: Permission) -> u8 {
    match p {
        Permission::Allow => 0,
        Permission::Ask => 1,
        Permission::Deny => 2,
    }
}

/// A method's default raised by the product's own per-call answer (a post to another user's member
/// asks): the stricter of the two, so a per-call check raises a call and never lowers it (CONTRACT.md
/// a-methods-default-is-a-floor).
#[must_use]
pub fn raised(default: Permission, per_call: Permission) -> Permission {
    if strictness(per_call) > strictness(default) { per_call } else { default }
}

/// The consent a call is held to: the product's default, resolved through the caller's own row (a
/// person's setting: Substrate's affordance stores; none on Agora's face). A row may tighten the
/// default freely; it loosens it only with a person's signed grant, and without one the default
/// stands.
#[must_use]
pub fn effective(default: Permission, row: Option<Permission>, grant: Option<PersonsGrant>) -> Permission {
    match row {
        None => default,
        Some(row) if strictness(row) >= strictness(default) => row,
        Some(row) if grant.is_some() => row,
        Some(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::{Exposure, Face, PersonsGrant, effective, exposure, raised};
    use crate::options::{Callers, Effect, MethodOptions, Permission, Presence, ResultShape};

    fn method(callers: Callers, presence: Presence, permission: Permission, person_act: bool) -> MethodOptions {
        let mut o = MethodOptions::new("a method", ResultShape::One, Effect::Write, presence, permission, callers);
        o.person_act = person_act;
        o
    }

    /// CONTRACT.md one-exposure-for-every-face: only an agent method that is not hidden has an agent
    /// face; its consent is its permission, and a person's act never answers Allow.
    #[test]
    fn one_exposure_for_every_face() {
        let agent = |presence, permission| exposure(&method(Callers::Agent, presence, permission, false));
        assert_eq!(
            agent(Presence::Full, Permission::Allow),
            Exposure { face: Face::Agent, consent: Permission::Allow }
        );
        assert_eq!(agent(Presence::Index, Permission::Ask).consent, Permission::Ask);
        assert_eq!(agent(Presence::Hidden, Permission::Allow).face, Face::Hidden, "hidden is hidden");
        for callers in [Callers::Product, Callers::Owner] {
            assert_eq!(exposure(&method(callers, Presence::Full, Permission::Allow, false)).face, Face::Hidden);
        }
        let act = exposure(&method(Callers::Agent, Presence::Full, Permission::Allow, true));
        assert_eq!(act.consent, Permission::Ask, "a person's act asks at least");
    }

    /// CONTRACT.md a-row-tightens-freely-and-loosens-only-by-a-persons-grant: a row may tighten any
    /// default; loosening one (an Ask to Allow, a Deny to Ask) holds only with a person's signed grant,
    /// and without one the default stands.
    #[test]
    fn a_row_tightens_freely_and_loosens_only_by_a_persons_grant() {
        use Permission::{Allow, Ask, Deny};
        assert_eq!(effective(Ask, None, None), Ask, "no row: the default");
        assert_eq!(effective(Allow, Some(Ask), None), Ask, "tightened freely");
        assert_eq!(effective(Ask, Some(Deny), None), Deny, "tightened freely");
        assert_eq!(effective(Ask, Some(Allow), None), Ask, "a loosening without a grant is refused");
        assert_eq!(effective(Deny, Some(Ask), None), Deny, "a loosening without a grant is refused");
        let grant = Some(PersonsGrant::verified());
        assert_eq!(effective(Ask, Some(Allow), grant), Allow, "a person's signed grant loosens it");
        assert_eq!(effective(Allow, Some(Deny), grant), Deny, "a grant never loosens past a row that tightens");
    }

    /// CONTRACT.md a-methods-default-is-a-floor: a per-call answer raises an Allow default to Ask or
    /// Deny, and never lowers an Ask or a Deny.
    #[test]
    fn a_methods_default_is_a_floor() {
        use Permission::{Allow, Ask, Deny};
        assert_eq!(raised(Allow, Ask), Ask, "a post to another user's member asks, though Post allows");
        assert_eq!(raised(Allow, Deny), Deny);
        assert_eq!(raised(Ask, Allow), Ask, "never lowered");
        assert_eq!(raised(Deny, Allow), Deny, "never lowered");
        assert_eq!(raised(Allow, Allow), Allow);
    }
}
