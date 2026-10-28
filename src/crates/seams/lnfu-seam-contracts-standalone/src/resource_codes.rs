//! The `E-RES-` error code set, shared by every face that carries it.
//!
//! Contract reference: CONTRACT.md section 6.4.5 fixes the seven
//! external-resource codes (`E-RES-001` to `E-RES-007`) and section 6.1 fixes
//! the segment as 001-007; the closing note of section 6.4.5 fixes their domain
//! tag to `"resource"`.
//!
//! The set lives here, in the lowest layer of the seam module tree, because two
//! faces that must not depend on each other both carry it: the library ingest
//! layer (`lnfu-library`) and the seam body (`lnfu-host-seam`). One code, one
//! definition, one meaning (CONTRACT.md section 6.3 rule 4): a second literal
//! copy would let the two drift apart, and neither face may reach into the
//! other to import it -- the seam body is deliberately free of every other LNFU
//! module (section 6.4.2).
//!
//! The codes are wire identifiers, not an interface of their own: they name
//! refusals, so they sit beside [`crate::error::ContractError`], the carrier
//! every LNFU layer already shares.

/// Error code: the resource's own manifest is missing or unparsable.
///
/// Contract reference: section 6.4.5 line 1560. Owned by position one, the
/// resource registration position: none of the four fields (identity,
/// address face, launch mode, and the capability face read alongside them) can
/// be taken from it, so there is nothing to register.
pub const E_RES_001: &str = "E-RES-001";
/// Error code: the node's capability face holds no entry to take.
///
/// Contract reference: section 6.4.5 line 1561. Shared by position two (the
/// resource discovery position) and position five (the capability projection
/// position) because one root cause carries one code: an empty entry set is
/// empty whether it is being reported or projected.
pub const E_RES_002: &str = "E-RES-002";
/// Error code: the called capability key is not in the node's capability list.
///
/// Contract reference: section 6.4.5 line 1562. Owned by position three, the
/// resource call position: the key did not appear in position two's listing.
pub const E_RES_003: &str = "E-RES-003";
/// Error code: the seam this call would pass through is unavailable.
///
/// Contract reference: section 6.4.5 line 1563. Owned by position three: either
/// the seam entry does not exist or it does not expose this capability.
pub const E_RES_004: &str = "E-RES-004";
/// Error code: the call was delivered but no result was obtained.
///
/// Contract reference: section 6.4.5 line 1564. Owned by position three: there
/// is no payload and no failure site, so the outcome is neither success nor a
/// diagnosable refusal.
pub const E_RES_005: &str = "E-RES-005";
/// Error code: the `library_id` currently has no hosted instance to release.
///
/// Contract reference: section 6.4.5 line 1565. Owned by position four, the
/// instance host position. The launch-failure face is not this code: it stays
/// with `E-LIB-006`, which position four shares with the carrier launch port
/// (section 1.4.6.4 same-face rule).
pub const E_RES_006: &str = "E-RES-006";
/// Error code: the projection result is not reference closed.
///
/// Contract reference: section 6.4.5 line 1566. Owned by position five, the
/// capability projection position: at least one edge end falls outside the
/// node set, so the graph is not closed and the dangling end is reported rather
/// than silently dropped.
pub const E_RES_007: &str = "E-RES-007";

/// Domain tag carried by every error of the `E-RES-` segment.
///
/// Contract reference: section 6.4.5 closing note fixes `domain` to
/// `"resource"` for the seven external-resource codes, parallel to `"library"`,
/// `"scope"` and `"VIZ"` rather than folded into the `E-LIB-` segment. The
/// segment covers the five external resource positions, which sit outside the
/// library ingest layer even though they are carried by this shared layer.
pub const DOMAIN_RESOURCE: &str = "resource";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_segment_holds_the_seven_codes_the_contract_fixes() {
        assert_eq!(
            [
                E_RES_001, E_RES_002, E_RES_003, E_RES_004, E_RES_005, E_RES_006, E_RES_007,
            ],
            [
                "E-RES-001",
                "E-RES-002",
                "E-RES-003",
                "E-RES-004",
                "E-RES-005",
                "E-RES-006",
                "E-RES-007",
            ]
        );
    }

    #[test]
    fn the_segment_carries_the_resource_domain() {
        assert_eq!(DOMAIN_RESOURCE, "resource");
    }
}
