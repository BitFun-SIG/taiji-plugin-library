//! Contract tests for the eight capability seam service definitions.
//!
//! Each test name follows the seam it exercises, so the seam registration suite
//! of T-06 can assert existence per seam. Assertions follow the test predicates
//! written in CONTRACT.md sections 2.1 to 2.5.

use std::path::PathBuf;

use lnfu_seam_contracts_standalone::availability::Availability;
use lnfu_seam_contracts_standalone::config::{
    ConfigPort, ConfigSource, ENV_GRAPH_PAGE_BYTES, ENV_KERNEL_ROOT, ENV_PANEL_PORT,
    ENV_UNIVERSE_ROOT, EnvConfigProvider, StartupArgs,
};
use lnfu_seam_contracts_standalone::descriptor::{SeamDescriptor, SeamId};
use lnfu_seam_contracts_standalone::event_bus::{
    BoundedEventBus, EventDomain, RuntimeEventEnvelope, RuntimeEventSink, RuntimeEventType,
};
use lnfu_seam_contracts_standalone::model_provider::{
    Context, LlmFactory, LlmMockProvider, RoleConfigRef, Task,
};
use lnfu_seam_contracts_standalone::registry::SharedRegistry;
use lnfu_seam_contracts_standalone::storage::{JsonFileStorage, StoragePort};
use lnfu_seam_contracts_standalone::subagent::{
    NULL_SUBAGENT_REASON, NullSubagentRuntime, SubagentPort, SubagentSpec,
};
use lnfu_seam_contracts_standalone::system_prompt::{
    PromptSection, StaticPromptProvider, SystemPromptPort,
};
use lnfu_seam_contracts_standalone::tool_registry::{
    IndexMapToolRegistry, RegisteredTool, ToolOutcome, ToolRegistryPort, ToolSchema,
};
use lnfu_seam_contracts_standalone::web_panel::{
    EmptyWebPanel, ROOT_PATH, TOPOLOGY_CONTENT_TYPE, TOPOLOGY_PATH, WebPanelPort,
};

/// Eight seam ids are declared and each descriptor carries all three roles.
#[test]
fn seam_registration_descriptor_set_is_complete() {
    assert_eq!(SeamId::ALL.len(), 8, "first phase registers eight seams");
    let ids: Vec<&str> = SeamId::ALL.iter().map(|id| id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "tool_registry",
            "model_provider",
            "subagent",
            "system_prompt",
            "event_bus",
            "storage",
            "config",
            "web_panel",
        ],
        "seam ids follow CONTRACT.md section 2.1 order"
    );
    for id in SeamId::ALL {
        let descriptor = SeamDescriptor::new(id, "provider", Availability::available());
        assert!(
            descriptor.is_complete(),
            "seam {id} carries all three roles"
        );
    }
}

/// Registration keeps order, rejects duplicates and revokes on drop.
#[test]
fn seam_registry_registration_is_ordered_and_revocable() {
    let registry: SharedRegistry<u32> = SharedRegistry::new("tests/seam_contracts.rs");
    let first = registry
        .register("a", 1)
        .expect("first registration succeeds");
    let _second = registry
        .register("b", 2)
        .expect("second registration succeeds");
    assert_eq!(registry.keys(), vec!["a".to_string(), "b".to_string()]);
    assert_eq!(registry.get("a", |value| *value), Some(1));

    let duplicate = registry
        .register("a", 9)
        .expect_err("duplicate key is rejected");
    assert_eq!(duplicate.code, "E-REG-001");

    let missing = registry
        .require("c", |value| *value)
        .expect_err("missing required key is rejected");
    assert_eq!(missing.code, "E-REG-002");

    drop(first);
    assert_eq!(registry.keys(), vec!["b".to_string()]);
}

/// Tool registry: schema assembly order, handler invocation and revocation.
#[test]
fn seam_tool_registry_registers_and_invokes() {
    let registry = IndexMapToolRegistry::new();
    let schema = ToolSchema::new(
        "echo",
        "echo the argument",
        serde_json::json!({"type": "object"}),
    )
    .expect("object schema is accepted");
    let tool = RegisteredTool::new(schema, |args| ToolOutcome::ok(args.clone()));
    let disposer = registry.register_tool(tool).expect("registration succeeds");

    let names: Vec<String> = registry.schemas().into_iter().map(|s| s.name).collect();
    assert_eq!(names, vec!["echo".to_string()]);

    let outcome = registry
        .invoke_tool("echo", &serde_json::json!({"text": "hi"}))
        .expect("registered tool is invocable");
    assert!(outcome.ok);
    assert_eq!(outcome.payload["text"], "hi");

    drop(disposer);
    assert!(registry.is_empty());
    let missing = registry
        .invoke_tool("echo", &serde_json::json!({}))
        .expect_err("revoked tool is not invocable");
    assert_eq!(missing.code, "E-TOOL-002");
}

/// Non-object tool parameters are rejected.
#[test]
fn seam_tool_registry_rejects_non_object_parameters() {
    let error = ToolSchema::new("bad", "bad", serde_json::json!(["not", "an", "object"]))
        .expect_err("non-object parameters are rejected");
    assert_eq!(error.code, "E-TOOL-001");
}

/// Model provider: factory selection key, mock echo and instance isolation.
#[test]
fn seam_model_provider_factory_selects_by_config_key() {
    let mut factory = LlmFactory::new();
    factory
        .register_provider(std::sync::Arc::new(LlmMockProvider::new()))
        .expect("mock provider registers");
    assert_eq!(factory.provider_names(), vec!["mock".to_string()]);

    let config = RoleConfigRef {
        default_model: "mock-model".to_string(),
        max_concurrent_tasks: 2,
        task_timeout: 30,
    };
    let first = factory
        .create_instance("mock", &config)
        .expect("instance created from configuration key");
    let second = factory
        .create_instance("mock", &config)
        .expect("second instance created");
    assert_ne!(
        first.instance_id(),
        second.instance_id(),
        "instances are isolated from each other"
    );

    let task = Task {
        task_id: "t-1".to_string(),
        prompt: "hello".to_string(),
    };
    let context = Context {
        worktree_path: "worktrees/alpha".to_string(),
        git_identity: "alpha-agent <alpha@fractal.local>".to_string(),
        task_history: Vec::new(),
    };
    let result = first.execute(&task, &context).expect("mock executes");
    assert!(result.output.contains("mock-model"));
    assert!(result.output.contains("hello"));

    let unknown = factory
        .create_instance("absent", &config)
        .err()
        .expect("unregistered model name fails");
    assert_eq!(unknown.code, "E-LLM-001");
    assert!(unknown.detail.contains("absent"));
}

/// Subagent seam: the null runtime reports unavailable and fails loud.
#[test]
fn seam_subagent_null_runtime_fails_loud() {
    let runtime = NullSubagentRuntime::new();
    let availability = runtime.availability();
    assert!(!availability.is_available());
    assert!(availability.as_token().contains(NULL_SUBAGENT_REASON));

    let spec = SubagentSpec {
        agent_id: "child".to_string(),
        instruction: "do work".to_string(),
    };
    let error = runtime
        .run(&spec)
        .expect_err("unavailable runtime fails loud");
    assert_eq!(error.code, "E-SEAM-001");
    assert!(error.detail.contains("child"));
}

/// System prompt seam: static sections keep assembly order.
#[test]
fn seam_system_prompt_static_provider_keeps_order() {
    let provider = StaticPromptProvider::from_sections(vec![
        PromptSection {
            name: "identity".to_string(),
            body: "identity body".to_string(),
        },
        PromptSection {
            name: "policy".to_string(),
            body: "policy body".to_string(),
        },
    ]);
    let sections = provider.sections("default").expect("sections resolve");
    let names: Vec<String> = sections.into_iter().map(|s| s.name).collect();
    assert_eq!(names, vec!["identity".to_string(), "policy".to_string()]);

    let error = provider
        .sections("")
        .expect_err("empty assembly key is rejected");
    assert_eq!(error.code, "E-SEAM-001");
}

/// Event bus: bounded fan-out, durable journaling and back pressure.
#[test]
fn seam_event_bus_publishes_and_bounds() {
    let (bus, receiver) = BoundedEventBus::new(2);
    let event = RuntimeEventEnvelope::new(
        "session-1",
        RuntimeEventType::TaskCompleted,
        serde_json::json!({"role_id": "r", "task_id": "t", "result": "ok"}),
    )
    .expect("envelope built");
    bus.publish_runtime_event(event.clone())
        .expect("first publish fits the queue");
    bus.publish_runtime_event(event.clone())
        .expect("second publish fits the queue");
    let overflow = bus
        .publish_runtime_event(event.clone())
        .expect_err("third publish hits back pressure");
    assert_eq!(overflow.code, "E-EVENT-001");
    assert_eq!(bus.published_count(), 2);

    let drained = BoundedEventBus::drain(&receiver);
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].event_type.domain(), EventDomain::Durable);

    let missing =
        RuntimeEventEnvelope::new("", RuntimeEventType::TaskCompleted, serde_json::json!({}))
            .expect_err("envelope without session id is rejected");
    assert_eq!(missing.code, "E-EVENT-003");
}

/// Storage seam: JSON write and read snapshot round trip.
#[test]
fn seam_storage_writes_and_reads_json() {
    let storage = JsonFileStorage::new();
    let root = std::env::temp_dir().join("lnfu-seam-storage-test");
    let path = root.join("snapshot.json");
    let _ = std::fs::remove_dir_all(&root);

    let document = serde_json::json!({"nodes": 2, "edges": 1});
    let written = storage
        .write_json(&path, &document)
        .expect("document is written");
    assert!(written > 0);
    assert!(storage.exists(&path));

    let read = storage.read_json(&path).expect("document is read back");
    assert_eq!(read, document);

    let error = storage
        .read_json(&root.join("absent.json"))
        .expect_err("missing document fails");
    assert_eq!(error.code, "E-STORAGE-002");

    let _ = std::fs::remove_dir_all(&root);
}

/// Config seam: environment first, argument second, dynamic discovery last.
#[test]
fn seam_config_resolves_from_env_then_argument() {
    let mut env = std::collections::BTreeMap::new();
    env.insert(ENV_KERNEL_ROOT.to_string(), "k-root".to_string());
    env.insert(ENV_UNIVERSE_ROOT.to_string(), "u-root".to_string());
    env.insert(ENV_PANEL_PORT.to_string(), "41000".to_string());
    env.insert(ENV_GRAPH_PAGE_BYTES.to_string(), "1024".to_string());
    let provider = EnvConfigProvider::new(StartupArgs::default(), env);

    assert_eq!(provider.kernel_root().unwrap(), PathBuf::from("k-root"));
    assert_eq!(provider.universe_root().unwrap(), PathBuf::from("u-root"));
    assert_eq!(
        provider.worktree_dir().unwrap(),
        PathBuf::from("u-root").join("worktrees")
    );
    assert_eq!(
        provider.audit_file().unwrap(),
        PathBuf::from("u-root").join("audit.log")
    );
    assert_eq!(provider.result_file("executor"), "result-executor.json");
    let port = provider.panel_port().unwrap();
    assert_eq!(port.value, 41000);
    assert_eq!(port.source, ConfigSource::Environment);
    assert_eq!(provider.graph_page_bytes(), 1024);
    assert_eq!(provider.panel_origin().unwrap(), "http://127.0.0.1:41000");

    let args = StartupArgs::parse(["--universe-root", "arg-root", "--panel-port=42000"])
        .expect("arguments parse");
    assert_eq!(args.universe_root, Some(PathBuf::from("arg-root")));
    assert_eq!(args.panel_port, Some(42000));
    let from_args = EnvConfigProvider::new(args, std::collections::BTreeMap::new());
    assert_eq!(
        from_args.universe_root().unwrap(),
        PathBuf::from("arg-root")
    );
    let arg_port = from_args.panel_port().unwrap();
    assert_eq!(arg_port.value, 42000);
    assert_eq!(arg_port.source, ConfigSource::Argument);

    let malformed = StartupArgs::parse(["--panel-port", "not-a-port"])
        .expect_err("malformed argument is rejected");
    assert_eq!(malformed.code, "E-CONFIG-002");
}

/// Config seam: the terminal source is dynamic port discovery.
#[test]
fn seam_config_discovers_a_free_port_last() {
    let provider =
        EnvConfigProvider::new(StartupArgs::default(), std::collections::BTreeMap::new());
    let port = provider
        .panel_port()
        .expect("dynamic discovery yields a port");
    assert!(port.value > 0);
    assert_eq!(port.source, ConfigSource::Discovered);

    let missing = provider
        .universe_root()
        .expect_err("universe root is required");
    assert_eq!(missing.code, "E-CONFIG-001");
}

/// Web panel seam: empty page plus same-origin topology endpoint.
#[test]
fn seam_web_panel_serves_page_and_topology() {
    let panel = EmptyWebPanel::with_topology("graph TD\n  A --> B").expect("topology accepted");
    let page = panel.handle(ROOT_PATH, None).expect("root page served");
    assert_eq!(page.status, 200);
    assert!(page.body.contains("<!doctype html>"));

    let topology = panel.handle(TOPOLOGY_PATH, None).expect("topology served");
    assert_eq!(topology.status, 200);
    assert_eq!(topology.content_type, TOPOLOGY_CONTENT_TYPE);
    assert!(topology.body.starts_with("graph"));

    let not_found = panel
        .handle("/absent", None)
        .expect("unknown route answered");
    assert_eq!(not_found.status, 404);

    let rejected = panel
        .handle(TOPOLOGY_PATH, Some(""))
        .expect_err("empty origin is rejected");
    assert_eq!(rejected.code, "E-PANEL-002");

    let invalid = EmptyWebPanel::with_topology("not a graph").expect_err("bad topology rejected");
    assert_eq!(invalid.code, "E-PANEL-003");
}
