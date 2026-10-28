//! Determinism / differential tests for the prefix guard.
//!
//! These exist to prove the guard has DISCRIMINATING POWER, not merely that it
//! returns `Ok`. A guard that always returns `Unchanged` would pass every
//! idempotency test ever written, so the tests below assert both directions:
//! shuffled input must be detected as reordered, and sorted input must be
//! reported untouched.
//!
//! Kept in a separate module so `lib.rs` stays the readable contract surface.

#[cfg(test)]
mod tests {
    use crate::{PrefixAction, stabilize_prefix, volatile_regions};
    use serde_json::json;

    /// Build a body whose tools are in a deliberately adversarial order: the
    /// same set appears twice with different shuffles, so a naive "sort by
    /// array position" implementation would still differ between the two.
    fn adversarial_pair() -> (serde_json::Value, serde_json::Value) {
        let names = ["search", "read_file", "apply_patch", "run_command"];
        let encode = |order: [usize; 4]| {
            let tools: Vec<serde_json::Value> = order
                .iter()
                .map(|i| json!({"type": "function", "function": {"name": names[*i]}}))
                .collect();
            json!({"model": "m", "tools": tools, "messages": [{"role": "user", "content": "x"}]})
        };
        (encode([0, 1, 2, 3]), encode([3, 1, 0, 2]))
    }

    #[test]
    fn two_shuffles_of_the_same_tools_converge_to_identical_json() {
        // This is the property the whole guard exists for: same tool set, any
        // arrival order, one byte-identical prefix on the wire.
        let (mut a, mut b) = adversarial_pair();
        stabilize_prefix(&mut a);
        stabilize_prefix(&mut b);
        assert_eq!(
            a["tools"], b["tools"],
            "different arrival orders must converge to the same tools bytes"
        );
    }

    #[test]
    fn reorder_detection_is_two_way() {
        // Direction 1: shuffled in -> reorder reported.
        let (_, mut b) = adversarial_pair();
        assert_eq!(stabilize_prefix(&mut b), PrefixAction::ToolsReordered);

        // Direction 2: already canonical in -> untouched reported.
        stabilize_prefix(&mut b);
        assert_eq!(stabilize_prefix(&mut b), PrefixAction::Unchanged);
    }

    #[test]
    fn canonical_order_is_by_tool_name_not_by_length() {
        // Guards against an implementation that sorts by serialized length,
        // which would look correct on same-length fixtures and be wrong here.
        // Lexicographic order: "aaaaaaaa..." < "m" < "zz" — the long name sorts
        // FIRST, which is the opposite of what a length-based sort would do.
        let mut body = json!({
            "tools": [
                {"function": {"name": "zz"}},
                {"function": {"name": "aaaaaaaaaaaaaaaaaaaaaaaa"}},
                {"function": {"name": "m"}}
            ]
        });
        stabilize_prefix(&mut body);
        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["aaaaaaaaaaaaaaaaaaaaaaaa", "m", "zz"]);
    }

    #[test]
    fn a_clean_live_body_reports_no_volatility() {
        // The real shape: stable system prompt, stable tools, volatile user turn.
        let body = json!({
            "model": "deepseek-v4.1-flash",
            "system": "You are a helpful coding agent.",
            "tools": [{"type": "function", "function": {"name": "read_file"}}],
            "messages": [
                {"role": "user", "content": "what changed in this repo today"}
            ]
        });
        assert!(
            volatile_regions(&body).is_empty(),
            "a healthy prefix must not be flagged, or the guard is pure noise"
        );
    }

    #[test]
    fn a_leaked_timestamp_is_flagged_in_every_region_it_can_hide() {
        for body in [
            json!({"system": "now 2026-09-17"}),
            json!({"tools": [{"function": {"name": "t", "description": "at 2026-09-17"}}]}),
            json!({"messages": [{"role": "system", "content": "at 2026-09-17"}]}),
        ] {
            assert!(
                !volatile_regions(&body).is_empty(),
                "a leaked date must be caught, got none for {body}"
            );
        }
    }

    #[test]
    fn messages_after_a_system_message_are_never_flagged() {
        // Only messages[0]-when-system is a cached prefix. Flagging later turns
        // would make the guard cry wolf on every request.
        let body = json!({
            "messages": [
                {"role": "system", "content": "stable"},
                {"role": "user", "content": "ran at 2026-09-17"}
            ]
        });
        assert!(volatile_regions(&body).is_empty());
    }
}
