//! LNFU host-side capability entrance -- the seam body.
//!
//! This crate carries the seam body fixed by `docs/CHARTER.md` section 6.4.4:
//! the minimal entrance through which an accessed software hands one of its own
//! capabilities back to LNFU.
//!
//! Position and shape (section 6.4.4, "the seam specification"):
//!
//! | item | ruling |
//! |---|---|
//! | position | on the accessed software's side; never a detour inside LNFU |
//! | shape | an entrance exposing the capability face (a pure-function import seam here) |
//! | volume | minimal -- it exposes, it does not change the accessed software's own logic |
//! | persistence | exactly one custom commit, replayable by `upstream-sync` |
//!
//! The body is a pure-function signature face and holds nothing else. Every
//! entrance takes its inputs by reference and returns one owned value; none of
//! them keeps state, opens a port, reads a file or enumerates a directory. That
//! is what makes the seam minimal, and that is why it can be dropped onto the
//! accessed software's side without touching that software's own logic.
//!
//! The entrance is deliberately free of any other LNFU module. The accessed
//! software's tree is a separate compilation unit, so a seam body that reached
//! back into LNFU internals would couple two trees that must stay independent
//! (section 6.4.2, "the accessed software is upgradeable on its own"). The only
//! shared type here is the one error carrier every LNFU layer already shares.
//!
//! Nothing here invents a coordinate. A caller supplies the node it is talking
//! about, the capability key it wants and what that software itself reported;
//! the entrance converts them into the one reply shape below. No path literal,
//! no port literal and no accessed software's name or version appears (section
//! 3.2, "zero hard coding").

#![forbid(unsafe_code)]

use lnfu_seam_contracts_standalone::{ContractError, DOMAIN_RESOURCE, E_RES_005};

/// Domain tag carried by an entrance refusal.
///
/// The entrance stands on the resource call face, so a refusal it raises is
/// carried with that face's domain -- one code, one meaning (`docs/CONTRACT.md`
/// sections 6.3 rule 4 and 6.4.5). The tag is read from the shared code set
/// rather than restated here, so the entrance and the library ingest layer
/// carry one `"resource"` and cannot drift apart (P2-2b in
/// `docs/D-20260914-39` section 2).
pub const DOMAIN_HOST_SEAM: &str = DOMAIN_RESOURCE;

/// Locus of the seam body, for a caller naming the entrance that refused.
///
/// The name follows the `LOCUS_*` convention the library ingest layer uses for
/// the same purpose (`crates/library/src/error.rs`), so one naming rule covers
/// every locus constant in the workspace (P3-11 in `docs/D-20260914-39`
/// section 2). It is a repo-relative constant and not a literal at each
/// producing site, so a caller names one place instead of repeating it.
pub const LOCUS_HOST_SEAM: &str = "src/crates/seams/lnfu-host-seam/src/lib.rs";

/// Error code: the call was delivered to the entrance but produced no result.
///
/// `docs/CONTRACT.md` section 6.4.5 fixes this code as "delivered but no result".
/// The entrance raises it rather than reading an empty delivery as a success,
/// which is the contract's "fail loud" rule (section 6.3 rule 1). The code is
/// read from the shared code set rather than restated here, so the entrance and
/// the library ingest layer carry one `E-RES-005` (P2-2b in `docs/D-20260914-39`
/// section 2).
pub const E_HOST_SEAM_EMPTY: &str = E_RES_005;

/// Number of fields an entrance reply carries.
///
/// The reply is the three-field outcome face: a status, an optional payload and
/// an optional failure site. The entrance states its result in exactly this
/// shape and in no second shape.
pub const ENTRANCE_REPLY_FIELDS: usize = 3;

/// The entrance identifier this seam body exposes.
///
/// It is the one name the seam states about itself, and it is what the seam
/// coordinate records so a delivered call can be traced back to this entrance.
/// The name is neutral: it carries no accessed software's name and no version
/// literal (section 3.1).
pub const ENTRANCE_ENTRY: &str = "lnfu_host_seam::deliver_capability_result";

/// The one custom-commit ordinal every seam carries.
///
/// Contract reference: `docs/CHARTER.md` section 6.4.4 and `docs/CONTRACT.md`
/// section 1.4.6.3. The seam is minimal and single, so its ordinal is a
/// constant rather than a value any caller supplies.
pub const SEAM_COMMIT_ORDINAL: u32 = 1;

/// The statement the entrance makes about its own shape.
///
/// The two variants name how the entrance is shaped and match the seam kinds
/// `docs/CONTRACT.md` section 1.4.6.3 fixes. The enumeration is open, so an
/// entrance of a shape not listed here is still representable as `Unspecified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeamKind {
    /// An imported function the accessed software exposes.
    ImportSeam,
    /// A trait the accessed software implements.
    TraitSeam,
    /// A seam whose shape is not stated.
    Unspecified,
}

/// The coordinate of this seam: its shape, the entrance it exposes and the
/// custom-commit ordinal of one.
///
/// The seam lives on the accessed software's side. This value records only its
/// position and identity and never its contents, so that software's own logic
/// stays untouched. Both fields are constants of this body, which is the
/// interface-side proof that the seam is minimal and single (section 6.4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeamCoordinate {
    /// The shape of the seam.
    pub seam_kind: SeamKind,
    /// The entrance the seam exposes.
    pub seam_entry: String,
    /// The custom-commit ordinal on the accessed software's side (always one).
    pub commit_ordinal: u32,
}

/// The status of one call.
///
/// The three variants are the result face `docs/CONTRACT.md` section 1.4.6.3
/// fixes: the capability ran, the seam refused it, or it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    /// The capability ran and produced a result.
    Succeeded,
    /// The seam refused the call.
    Refused,
    /// The call failed on the accessed software's side.
    Failed,
}

/// The outcome face of one call: a status, an optional payload and an optional
/// failure site.
///
/// The three fields are the reply shape this entrance states. A reply that
/// carries neither a payload nor a failure site is not a success and is refused
/// by [`CallOutcome::validate`] rather than read as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOutcome {
    /// The result status.
    pub status: CallStatus,
    /// The result payload; `None` when there is none.
    pub payload: Option<String>,
    /// The failure site; `None` when the call succeeded.
    pub failure_detail: Option<String>,
}

impl CallOutcome {
    /// Check that the outcome states a result.
    ///
    /// Contract reference: `docs/CONTRACT.md` section 6.4.5. An outcome that
    /// neither produced a payload nor named a failure site means the call was
    /// delivered and no result was obtained, so it is refused loudly rather
    /// than read as a success.
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.payload.is_none() && self.failure_detail.is_none() {
            return Err(ContractError::new(
                E_HOST_SEAM_EMPTY,
                DOMAIN_HOST_SEAM,
                "the call was delivered to the entrance but produced neither a payload nor a \
                 failure site",
                LOCUS_HOST_SEAM,
            ));
        }
        Ok(())
    }
}

/// The statement this entrance makes about its own shape.
///
/// The coordinate is built from the two constants above and never from an
/// argument, so a caller cannot widen or narrow the seam by what it passes in.
pub fn entrance_coordinate() -> SeamCoordinate {
    SeamCoordinate {
        seam_kind: SeamKind::ImportSeam,
        seam_entry: ENTRANCE_ENTRY.to_string(),
        commit_ordinal: SEAM_COMMIT_ORDINAL,
    }
}

/// The one entrance: turn what an accessed software reported into the reply
/// shape above.
///
/// The signature is pure -- three borrowed inputs in, one owned outcome out, no
/// state, no side effect, no coordinate of its own. `library_id` names the node
/// so the reply can be attributed; `capability_key` names the capability that
/// software ran; `payload` is that software's own result text.
///
/// A caller that has no payload is refused with `E-RES-005`, because a call that
/// was delivered and produced nothing is not a success (`docs/CONTRACT.md`
/// section 6.4.5). The entrance does not substitute an invented value for the
/// missing payload (section 3.2 rule 6, "fail loud").
///
/// # Examples
///
/// ```ignore
/// let outcome = lnfu_host_seam::deliver_capability_result("node-key", "cap-key", "rows=42")?;
/// ```
pub fn deliver_capability_result(
    library_id: &str,
    capability_key: &str,
    payload: &str,
) -> Result<CallOutcome, ContractError> {
    if payload.trim().is_empty() {
        return Err(ContractError::new(
            E_HOST_SEAM_EMPTY,
            DOMAIN_HOST_SEAM,
            format!(
                "capability {capability_key} on {library_id} was delivered to the entrance but \
                 reported no payload: the entrance looked for a payload and found none"
            ),
            LOCUS_HOST_SEAM,
        ));
    }
    Ok(CallOutcome {
        status: CallStatus::Succeeded,
        payload: Some(payload.to_string()),
        failure_detail: None,
    })
}

/// The entrance reporting a failure: that software ran the capability, it
/// failed, and this is the site it reported.
///
/// This is the second half of the same pure-function face as
/// [`deliver_capability_result`]. A capability that ran and failed states its
/// failure site rather than falling back to an invented value (section 3.2 rule
/// 6, "fail loud"). A failure with no site is itself refused, because a reply
/// that states neither a payload nor a site is not a result.
pub fn deliver_capability_failure(
    library_id: &str,
    capability_key: &str,
    failure_site: &str,
) -> Result<CallOutcome, ContractError> {
    let outcome = CallOutcome {
        status: CallStatus::Failed,
        payload: None,
        failure_detail: Some(failure_site.to_string()),
    };
    if failure_site.trim().is_empty() {
        return Err(ContractError::new(
            E_HOST_SEAM_EMPTY,
            DOMAIN_HOST_SEAM,
            format!(
                "capability {capability_key} on {library_id} failed with no site: the entrance \
                 looked for a failure site and found none"
            ),
            LOCUS_HOST_SEAM,
        ));
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reported_payload_becomes_a_success_outcome() {
        let outcome = deliver_capability_result("node-key", "cap-key", "rows=42")
            .expect("a reported payload is an outcome");
        assert_eq!(outcome.status, CallStatus::Succeeded);
        assert_eq!(outcome.payload.as_deref(), Some("rows=42"));
        assert_eq!(outcome.failure_detail, None);
    }

    #[test]
    fn an_empty_delivery_is_refused() {
        let failure = deliver_capability_result("node-key", "cap-key", "   ")
            .expect_err("an empty delivery is refused");
        assert_eq!(failure.code, E_HOST_SEAM_EMPTY);
    }

    #[test]
    fn the_entrance_coordinate_carries_ordinal_one() {
        let coordinate = entrance_coordinate();
        assert_eq!(coordinate.commit_ordinal, SEAM_COMMIT_ORDINAL);
        assert_eq!(coordinate.commit_ordinal, 1);
        assert_eq!(coordinate.seam_entry, ENTRANCE_ENTRY);
        assert_eq!(coordinate.seam_kind, SeamKind::ImportSeam);
    }

    #[test]
    fn a_failure_site_becomes_a_failed_outcome() {
        let outcome = deliver_capability_failure("node-key", "cap-key", "backend refused")
            .expect("a reported failure site is an outcome");
        assert_eq!(outcome.status, CallStatus::Failed);
        assert_eq!(outcome.failure_detail.as_deref(), Some("backend refused"));
    }

    /// P2-2b: the entrance carries the shared code, not a copy of it.
    ///
    /// The refusal this entrance raises has to carry the very identifiers the
    /// shared code set fixes, because a second literal copy is exactly what let
    /// the two faces drift apart. Comparing against the shared constants (not
    /// against fresh literals) is what makes the assertion able to fail: a seam
    /// that kept its own string would stop matching the moment the shared set
    /// changed.
    #[test]
    fn the_entrance_carries_the_shared_resource_code() {
        assert_eq!(E_HOST_SEAM_EMPTY, lnfu_seam_contracts_standalone::E_RES_005);
        assert_eq!(
            DOMAIN_HOST_SEAM,
            lnfu_seam_contracts_standalone::DOMAIN_RESOURCE
        );
        let failure = deliver_capability_failure("node-key", "cap-key", "  ")
            .expect_err("a failure with no site is refused");
        assert_eq!(failure.code, lnfu_seam_contracts_standalone::E_RES_005);
        assert_eq!(
            failure.domain,
            lnfu_seam_contracts_standalone::DOMAIN_RESOURCE
        );
    }

    /// P3-11: the locus names this file, read from a path and not restated.
    ///
    /// The constant has to point at the producing file, and the ordinary
    /// `file!()` of this module says where that is. Written as a literal in
    /// both places the assertion would be a tautology; read from `file!()` it
    /// fails as soon as the constant and the file part company.
    #[test]
    fn the_locus_names_this_file() {
        let here = file!();
        let named: &str = LOCUS_HOST_SEAM;
        assert!(
            named.ends_with(&here.replace('\\', "/")),
            "the locus {named:?} names this file ({here:?})"
        );
        assert_eq!(named.rsplit('/').next(), Some("lib.rs"));
    }
}
