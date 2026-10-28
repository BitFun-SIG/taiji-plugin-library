//! Integration test against the REAL request shape observed in production.
//!
//! The unit tests use hand-built fixtures. This one is different in kind: it
//! encodes what the live runtime actually sends, so a change that satisfies the
//! fixtures but breaks the real shape is still caught.
//!
//! Live facts this file is derived from (measured 2026-09-17):
//!   * provider path       OpenAI-compatible endpoint, model `deepseek-v4.1-flash`
//!   * tools               OpenAI shape: {"type":"function","function":{...}}
//!   * system prompt       OpenAI shape: messages[0].role == "system"
//!   * cache behaviour     automatic, no cache_control marker, hitting 88-99.9%

use serde_json::{Value, json};
use token_shaper::{PrefixAction, stabilize_prefix, volatile_regions};

/// One tool in the exact nested shape the OpenAI converter emits.
fn live_tool(name: &str, description: &str) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }
        }
    })
}

/// A request body shaped the way the live OpenAI-compatible path builds it.
fn live_body(tool_order: &[&str]) -> Value {
    let tools: Vec<Value> = tool_order
        .iter()
        .map(|n| live_tool(n, &format!("tool {n}")))
        .collect();
    json!({
        "model": "deepseek-v4.1-flash",
        "stream": true,
        "messages": [
            {"role": "system", "content": "You are a helpful coding agent."},
            {"role": "user", "content": "inspect the repo"}
        ],
        "tools": tools
    })
}

#[test]
fn same_tool_set_arriving_in_different_orders_serializes_identically() {
    // The registry can hand tools over in any order. On the wire they must not.
    let mut a = live_body(&["read_file", "apply_patch", "run_command", "search"]);
    let mut b = live_body(&["run_command", "search", "apply_patch", "read_file"]);
    stabilize_prefix(&mut a);
    stabilize_prefix(&mut b);
    assert_eq!(
        a.to_string(),
        b.to_string(),
        "identical request content must serialize to identical bytes, or the \\
         provider prefix cache misses on every alternating turn"
    );
}

#[test]
fn the_live_body_is_already_stable_and_flagged_clean() {
    // Baseline assertion: if EITHER of these fails on the live shape, the guard
    // is either doing nothing useful or crying wolf on healthy traffic.
    // This order is deliberately NOT lexicographic ("search" < "read_file" is
    // false: 'r' < 's', so search must move to the back).
    let mut body = live_body(&["search", "apply_patch", "read_file", "run_command"]);
    assert_eq!(
        stabilize_prefix(&mut body),
        PrefixAction::ToolsReordered,
        "unsorted live tools must be reordered"
    );
    let names: Vec<&str> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["apply_patch", "read_file", "run_command", "search"]
    );
    assert!(
        volatile_regions(&body).is_empty(),
        "the real system prompt and tool schemas must not be flagged as volatile"
    );
}

#[test]
fn stable_rerequest_is_a_true_no_op() {
    // Steady state: the same turn sent again must produce byte-identical output
    // AND be reported as untouched. Anything else means we are churning the
    // prefix once per turn, which is the exact failure mode this crate prevents.
    let mut first = live_body(&["search", "apply_patch", "read_file", "run_command"]);
    stabilize_prefix(&mut first);
    let bytes_after_first = first.to_string();

    let mut second = first.clone();
    assert_eq!(stabilize_prefix(&mut second), PrefixAction::Unchanged);
    assert_eq!(second.to_string(), bytes_after_first);
}

#[test]
fn appended_turns_leave_the_prefix_intact() {
    // Append-only is the core invariant: adding a turn must not perturb any byte
    // that came before it, so the tools+system+history prefix stays cacheable.
    let mut turn1 = live_body(&["read_file", "search"]);
    stabilize_prefix(&mut turn1);
    let prefix_of_turn1 = serde_json::to_string(&turn1["tools"]).unwrap();
    let system_of_turn1 = serde_json::to_string(&turn1["messages"][0]).unwrap();

    let mut turn2 = turn1.clone();
    turn2["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role": "assistant", "content": "done"}));
    stabilize_prefix(&mut turn2);

    assert_eq!(
        serde_json::to_string(&turn2["tools"]).unwrap(),
        prefix_of_turn1,
        "tools must be byte-identical across turns"
    );
    assert_eq!(
        serde_json::to_string(&turn2["messages"][0]).unwrap(),
        system_of_turn1,
        "system message must be byte-identical across turns"
    );
}
