use openbitfun_runtime_ports::{
    DialogQueueAction as Action, DialogQueueMessage, DialogQueueRequest,
    DialogQueueStatus as Status,
};

fn request(epoch: Option<&str>, action: Action) -> DialogQueueRequest {
    DialogQueueRequest {
        session_id: "host-queue-session".into(),
        queue_epoch: epoch.map(str::to_owned),
        action,
    }
}
fn message(id: &str) -> DialogQueueMessage {
    DialogQueueMessage {
        turn_id: id.into(),
        content: "follow up while offline".into(),
        display_content: None,
        agent_type: "Standard".into(),
        attachments: Vec::new(),
        metadata: Default::default(),
    }
}
async fn fixture() -> (
    Arc<DialogScheduler>,
    Arc<SessionManager>,
    tempfile::TempDir,
    String,
) {
    let (scheduler, sessions, _, root) = test_scheduler();
    mark_session_processing(&sessions, &root, "host-queue-session", "active-turn").await;
    scheduler.active_turns.insert(
        "host-queue-session".into(),
        desktop_active_turn("active-turn"),
    );
    let snapshot = scheduler
        .manage_host_queue(request(None, Action::List))
        .await
        .unwrap();
    (scheduler, sessions, root, snapshot.queue_epoch)
}

#[tokio::test]
async fn host_queue_duplicate_and_conflicting_submissions() {
    let (scheduler, _, _root, epoch) = fixture().await;
    let submit = request(
        Some(&epoch),
        Action::Submit {
            message: message("queued-a"),
        },
    );
    let (a, b) = tokio::join!(
        scheduler.manage_host_queue(submit.clone()),
        scheduler.manage_host_queue(submit)
    );
    assert_eq!(a.unwrap().receipt.unwrap().status, Status::Queued);
    assert_eq!(b.unwrap().items.len(), 1);
    assert_eq!(scheduler.queue_depth("host-queue-session"), 1);
    let mut changed = message("queued-a");
    changed.content = "different".into();
    assert!(scheduler
        .manage_host_queue(request(Some(&epoch), Action::Submit { message: changed }))
        .await
        .unwrap_err()
        .message
        .contains("idempotency_conflict"));
}

#[tokio::test]
async fn host_queue_cancel_is_idempotent_and_never_cancels_active_turn() {
    let (scheduler, _, _root, epoch) = fixture().await;
    scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Submit {
                message: message("queued-a"),
            },
        ))
        .await
        .unwrap();
    let cancel = request(
        Some(&epoch),
        Action::Cancel {
            turn_id: "queued-a".into(),
            operation_id: "cancel-a".into(),
        },
    );
    for _ in 0..2 {
        assert_eq!(
            scheduler
                .manage_host_queue(cancel.clone())
                .await
                .unwrap()
                .receipt
                .unwrap()
                .status,
            Status::Cancelled
        );
    }
    assert!(scheduler
        .active_turns
        .matches_turn("host-queue-session", "active-turn"));
    assert_eq!(scheduler.queue_depth("host-queue-session"), 0);
}

#[tokio::test]
async fn host_queue_steering_retains_payload_until_consumption_and_rejects_cancel() {
    let (scheduler, _, _root, epoch) = fixture().await;
    scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Submit {
                message: message("queued-a"),
            },
        ))
        .await
        .unwrap();
    let promote = request(
        Some(&epoch),
        Action::Promote {
            turn_id: "queued-a".into(),
            operation_id: "promote-a".into(),
            expected_active_turn_id: Some("active-turn".into()),
        },
    );
    let snapshot = scheduler.manage_host_queue(promote.clone()).await.unwrap();
    assert_eq!(snapshot.receipt.unwrap().status, Status::SteeringPending);
    scheduler.manage_host_queue(promote).await.unwrap();
    let injections = scheduler
        .round_injection_source
        .take_pending("host-queue-session", "active-turn");
    assert_eq!(injections.len(), 1);
    assert_eq!(
        scheduler.queue_depth("host-queue-session"),
        1,
        "draining the buffer is not consumption"
    );
    assert!(scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Cancel {
                turn_id: "queued-a".into(),
                operation_id: "cancel-a".into()
            }
        ))
        .await
        .unwrap_err()
        .message
        .contains("too_late"));
    let injection = &injections[0];
    scheduler.round_injection_source.acknowledge_consumed(
        "host-queue-session",
        "active-turn",
        &injection.id,
        injection.kind,
    );
    let result = scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Get {
                turn_id: "queued-a".into(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(result.receipt.unwrap().status, Status::Steered);
    assert_eq!(result.used, 0);
}

#[tokio::test]
async fn host_queue_unconsumed_steering_and_failed_queue_remain_recoverable() {
    let (scheduler, sessions, _root, epoch) = fixture().await;
    for id in ["queued-a", "queued-b"] {
        scheduler
            .manage_host_queue(request(
                Some(&epoch),
                Action::Submit {
                    message: message(id),
                },
            ))
            .await
            .unwrap();
    }
    scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Promote {
                turn_id: "queued-a".into(),
                operation_id: "promote-a".into(),
                expected_active_turn_id: Some("active-turn".into()),
            },
        ))
        .await
        .unwrap();
    let _ = scheduler
        .round_injection_source
        .take_pending("host-queue-session", "active-turn");
    scheduler
        .outcome_sender()
        .send((
            "host-queue-session".into(),
            TurnOutcome::Failed {
                turn_id: "active-turn".into(),
                error: "provider unavailable".into(),
            },
        ))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let snapshot = scheduler
                .manage_host_queue(request(None, Action::List))
                .await
                .unwrap();
            if snapshot.items.len() == 2
                && snapshot
                    .items
                    .iter()
                    .all(|item| item.status == Status::Blocked)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(scheduler.queue_depth("host-queue-session"), 2);
    sessions
        .update_session_state("host-queue-session", SessionState::Idle)
        .await
        .unwrap();
    let before = scheduler
        .manage_host_queue(request(None, Action::List))
        .await
        .unwrap();
    let storage = sessions
        .storage_path_binding_for_test("host-queue-session")
        .unwrap();
    assert!(
        scheduler
            .begin_session_maintenance_with_policy(
                "host-queue-session",
                &storage,
                Duration::ZERO,
                true,
            )
            .await
            .is_err()
    );
    let after = scheduler
        .manage_host_queue(request(Some(&epoch), Action::List))
        .await
        .unwrap();
    assert_eq!(after.queue_epoch, before.queue_epoch);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.items, before.items);
}

#[tokio::test]
async fn host_queue_promote_fences_target_and_epoch() {
    let (scheduler, _, _root, epoch) = fixture().await;
    scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Submit {
                message: message("queued-a"),
            },
        ))
        .await
        .unwrap();
    assert!(scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Promote {
                turn_id: "queued-a".into(),
                operation_id: "promote-a".into(),
                expected_active_turn_id: None
            }
        ))
        .await
        .unwrap_err()
        .message
        .contains("queue_conflict"));
    scheduler
        .host_queue
        .lock()
        .unwrap()
        .retire("host-queue-session");
    assert!(scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Submit {
                message: message("queued-b")
            }
        ))
        .await
        .unwrap_err()
        .message
        .contains("queue_scope_expired"));
}

#[tokio::test]
async fn host_queue_request_survives_disconnected_caller() {
    let (scheduler, _, _root, epoch) = fixture().await;
    let guard = scheduler.lock_session_operation("host-queue-session").await;
    let owner = scheduler.clone();
    let task = tokio::spawn(async move {
        owner
            .manage_host_queue(request(
                Some(&epoch),
                Action::Submit {
                    message: message("queued-offline"),
                },
            ))
            .await
    });
    // Wait for host admission, then drop the caller while the host is locked.
    tokio::time::timeout(Duration::from_secs(3), async {
        while !scheduler
            .host_queue
            .lock()
            .unwrap()
            .contains("host-queue-session", "queued-offline")
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    drop(guard);
    tokio::time::timeout(Duration::from_secs(3), async {
        while scheduler.queue_depth("host-queue-session") != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(scheduler
        .active_turns
        .matches_turn("host-queue-session", "active-turn"));
}

#[tokio::test]
async fn host_queue_host_outcomes_start_followups_without_any_client() {
    let (scheduler, sessions, _root, epoch) = fixture().await;
    let id = "host-queue-session";
    let ai_config = AIConfig {
        models: vec![AIModelConfig {
            id: "queue-test-model".into(),
            name: "Queue test".into(),
            provider: "openai".into(),
            model_name: "test-model".into(),
            base_url: "http://127.0.0.1:1".into(),
            enabled: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            ai_config.clone(),
            sessions.update_session_model_id(id, "queue-test-model"),
        )
        .await
        .unwrap();
    for turn in ["offline-b", "offline-c"] {
        scheduler
            .manage_host_queue(request(
                Some(&epoch),
                Action::Submit {
                    message: message(turn),
                },
            ))
            .await
            .unwrap();
    }
    // No query, RPC or controller drives the following transitions. Feed real
    // scheduler outcomes; the real coordinator must create both follow-up turns.
    let (tx, rx) = mpsc::unbounded_channel();
    let owner = scheduler.clone();
    let handler = tokio::spawn(async move {
        TEST_MODEL_RESOLUTION_AI_CONFIG
            .scope(
                AIConfig {
                    models: vec![AIModelConfig {
                        id: "queue-test-model".into(),
                        name: "Queue test".into(),
                        provider: "openai".into(),
                        model_name: "test-model".into(),
                        base_url: "http://127.0.0.1:1".into(),
                        enabled: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                owner.run_outcome_handler(rx),
            )
            .await;
    });
    for (finished, started, count) in [
        ("active-turn", "offline-b", 1),
        ("offline-b", "offline-c", 2),
    ] {
        sessions
            .update_session_state(id, SessionState::Idle)
            .await
            .unwrap();
        tx.send((
            id.into(),
            TurnOutcome::Completed {
                turn_id: finished.into(),
                final_response: "done".into(),
            },
        ))
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if scheduler.active_turns.matches_turn(id, started) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("host dispatch must start the follow-up");
        assert_eq!(sessions.get_turn_count(id), count);
        let _ = scheduler.coordinator.cancel_dialog_turn(id, started).await;
    }
    handler.abort();
}

#[tokio::test]
async fn host_queue_capacity_counts_unconsumed_steering() {
    let (scheduler, _, _root, epoch) = fixture().await;
    for index in 0..scheduler.queues.max_depth() {
        scheduler
            .manage_host_queue(request(
                Some(&epoch),
                Action::Submit {
                    message: message(&format!("queued-{index}")),
                },
            ))
            .await
            .unwrap();
    }
    scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Promote {
                turn_id: "queued-0".into(),
                operation_id: "promote-first".into(),
                expected_active_turn_id: Some("active-turn".into()),
            },
        ))
        .await
        .unwrap();
    assert!(scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Submit {
                message: message("overflow")
            }
        ))
        .await
        .unwrap_err()
        .message
        .contains("queue is full"));
    assert_eq!(
        scheduler.queue_depth("host-queue-session"),
        scheduler.queues.max_depth()
    );
}

#[tokio::test]
async fn host_queue_concurrent_cancel_and_promote_has_one_winner() {
    let (scheduler, _, _root, epoch) = fixture().await;
    scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Submit {
                message: message("queued-a"),
            },
        ))
        .await
        .unwrap();
    let (cancel, promote) = tokio::join!(
        scheduler.manage_host_queue(request(
            Some(&epoch),
            Action::Cancel {
                turn_id: "queued-a".into(),
                operation_id: "cancel-a".into()
            }
        )),
        scheduler.manage_host_queue(request(
            Some(&epoch),
            Action::Promote {
                turn_id: "queued-a".into(),
                operation_id: "promote-a".into(),
                expected_active_turn_id: Some("active-turn".into())
            }
        )),
    );
    assert_ne!(cancel.is_ok(), promote.is_ok());
    assert!(scheduler
        .active_turns
        .matches_turn("host-queue-session", "active-turn"));
}

#[tokio::test]
async fn host_queue_cancel_during_terminal_transition_does_not_replace_active_owner() {
    let (scheduler, sessions, _root, epoch) = fixture().await;
    for id in ["queued-a", "queued-b"] {
        scheduler
            .manage_host_queue(request(
                Some(&epoch),
                Action::Submit {
                    message: message(id),
                },
            ))
            .await
            .unwrap();
    }
    sessions
        .update_session_state("host-queue-session", SessionState::Idle)
        .await
        .unwrap();
    scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Cancel {
                turn_id: "queued-a".into(),
                operation_id: "cancel-a".into(),
            },
        ))
        .await
        .unwrap();
    assert!(scheduler
        .active_turns
        .matches_turn("host-queue-session", "active-turn"));
    assert_eq!(scheduler.queue_depth("host-queue-session"), 1);
}

#[tokio::test]
async fn host_queue_interrupted_target_cannot_consume_a_blocked_injection_on_resume() {
    let (scheduler, _, _root, epoch) = fixture().await;
    for id in ["queued-a", "queued-b"] {
        scheduler
            .manage_host_queue(request(
                Some(&epoch),
                Action::Submit {
                    message: message(id),
                },
            ))
            .await
            .unwrap();
    }
    scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Promote {
                turn_id: "queued-a".into(),
                operation_id: "promote-a".into(),
                expected_active_turn_id: Some("active-turn".into()),
            },
        ))
        .await
        .unwrap();
    scheduler
        .outcome_sender()
        .send((
            "host-queue-session".into(),
            TurnOutcome::Interrupted {
                turn_id: "active-turn".into(),
                execution_generation: 0,
            },
        ))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let snapshot = scheduler
                .manage_host_queue(request(None, Action::List))
                .await
                .unwrap();
            if snapshot.items.len() == 2
                && snapshot
                    .items
                    .iter()
                    .all(|item| item.status == Status::Blocked)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(scheduler
        .round_injection_source
        .take_pending("host-queue-session", "active-turn")
        .is_empty());
    assert_eq!(scheduler.queue_depth("host-queue-session"), 2);
}

#[test]
fn host_queue_new_prompt_after_stop_supersedes_interruption() {
    // This fixture polls coordinator admission inline to retain the task-local
    // model configuration. Give its large debug future a dedicated test stack.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    for has_held_message in [false, true] {
                        let (scheduler, sessions, _, root) = test_scheduler_with_persistence(true);
                        let id = "host-queue-session";
                        let workspace =
                            fixture_workspace_dir(root.path().join("stopped-workspace"));
                        sessions
                            .create_session_with_id(
                                Some(id.into()),
                                "Stopped".into(),
                                "Standard".into(),
                                SessionConfig {
                                    workspace_path: Some(workspace.to_string_lossy().into_owned()),
                                    ..Default::default()
                                },
                            )
                            .await
                            .unwrap();
                        sessions
                            .start_dialog_turn(
                                id,
                                "Standard".into(),
                                "original work".into(),
                                Some("stopped-turn".into()),
                                None,
                                None,
                            )
                            .await
                            .unwrap();
                        let epoch = scheduler
                            .manage_host_queue(request(None, Action::List))
                            .await
                            .unwrap()
                            .queue_epoch;
                        if has_held_message {
                            scheduler
                                .manage_host_queue(request(
                                    Some(&epoch),
                                    Action::Submit {
                                        message: message("previously-queued"),
                                    },
                                ))
                                .await
                                .unwrap();
                            scheduler.hold_managed_queue(id, "Turn interrupted");
                        }
                        sessions
                            .mark_dialog_turn_interrupted(id, "stopped-turn")
                            .await
                            .unwrap();
                        sessions
                            .update_session_state_for_turn_if_processing(
                                id,
                                "stopped-turn",
                                SessionState::Idle,
                            )
                            .await
                            .unwrap();
                        assert!(sessions
                            .latest_dialog_turn_holds_dispatch(id)
                            .await
                            .unwrap());
                        assert!(scheduler.try_start_next_queued(id).await.unwrap().is_none());

                        let failed = TEST_MODEL_RESOLUTION_AI_CONFIG
                            .scope(
                                AIConfig::default(),
                                Box::pin(scheduler.execute_queue_request(request(
                                    Some(&epoch),
                                    Action::Submit {
                                        message: message("new-prompt"),
                                    },
                                ))),
                            )
                            .await;
                        assert!(failed.is_err(), "missing model must reject admission");
                        assert!(
                            sessions
                                .latest_dialog_turn_holds_dispatch(id)
                                .await
                                .unwrap(),
                            "failed submission must preserve the stopped turn for recovery"
                        );
                        assert_eq!(sessions.get_turn_count(id), 1);

                        let ai_config = AIConfig {
                            models: vec![AIModelConfig {
                                id: "queue-stop-test-model".into(),
                                name: "Queue stop test".into(),
                                provider: "openai".into(),
                                model_name: "test-model".into(),
                                base_url: "http://127.0.0.1:1".into(),
                                enabled: true,
                                ..Default::default()
                            }],
                            ..Default::default()
                        };
                        TEST_MODEL_RESOLUTION_AI_CONFIG
                            .scope(
                                ai_config.clone(),
                                sessions.update_session_model_id(id, "queue-stop-test-model"),
                            )
                            .await
                            .unwrap();
                        // Execute the same host-side request body inline so the model fixture's
                        // task-local scope covers admission; no model response is needed.
                        let submit = request(
                            Some(&epoch),
                            Action::Submit {
                                message: message("new-prompt"),
                            },
                        );
                        let snapshot = TEST_MODEL_RESOLUTION_AI_CONFIG
                            .scope(
                                ai_config,
                                Box::pin(scheduler.execute_queue_request(submit.clone())),
                            )
                            .await
                            .expect("a fresh user prompt must supersede the stopped turn");
                        assert_eq!(snapshot.receipt.unwrap().status, Status::Started);
                        assert!(!sessions
                            .latest_dialog_turn_holds_dispatch(id)
                            .await
                            .unwrap());
                        assert_eq!(sessions.get_turn_count(id), 2);
                        let storage = sessions.effective_session_storage_path(id).await.unwrap();
                        let stopped = sessions
                            .persistence_manager()
                            .load_dialog_turn(&storage, id, 0)
                            .await
                            .unwrap()
                            .unwrap();
                        assert_eq!(stopped.turn_id, "stopped-turn");
                        assert!(
                            stopped.recovery.is_none(),
                            "accepted prompt must retire old recovery"
                        );

                        if has_held_message {
                            assert!(snapshot
                                .items
                                .iter()
                                .any(|item| item.turn_id == "previously-queued"
                                    && item.status == Status::Blocked));
                        }
                        // Reconnect/retry must return the existing receipt, not start again.
                        scheduler.manage_host_queue(submit).await.unwrap();
                        assert_eq!(sessions.get_turn_count(id), 2);
                        let _ = scheduler
                            .coordinator
                            .cancel_dialog_turn(id, "new-prompt")
                            .await;
                    }
                });
        })
        .unwrap()
        .join()
        .unwrap();
}

fn run_host_queue_lifecycle_test<F: std::future::Future<Output = ()>>(
    test: impl FnOnce() -> F + Send + 'static,
) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(test());
        })
        .unwrap()
        .join()
        .unwrap();
}

fn host_queue_lifecycle_model() -> AIConfig {
    AIConfig {
        models: vec![AIModelConfig {
            id: "queue-lifecycle-model".into(),
            name: "Queue lifecycle".into(),
            provider: "openai".into(),
            model_name: "test-model".into(),
            base_url: "http://127.0.0.1:1".into(),
            enabled: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

async fn stopped_host_queue_fixture(
    retiring: bool,
) -> (
    Arc<DialogScheduler>,
    Arc<SessionManager>,
    tempfile::TempDir,
    String,
) {
    let (scheduler, sessions, _, root) = test_scheduler_with_persistence(true);
    let id = "host-queue-session";
    let workspace = fixture_workspace_dir(root.path().join("stopped-lifecycle"));
    sessions
        .create_session_with_id(
            Some(id.into()),
            "Stopped".into(),
            "Standard".into(),
            SessionConfig {
                workspace_path: Some(workspace.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    sessions
        .start_dialog_turn(
            id,
            "Standard".into(),
            "original".into(),
            Some("stopped-turn".into()),
            None,
            None,
        )
        .await
        .unwrap();
    let epoch = scheduler
        .manage_host_queue(request(None, Action::List))
        .await
        .unwrap()
        .queue_epoch;
    for turn_id in ["old-a", "old-b"] {
        scheduler
            .manage_host_queue(request(
                Some(&epoch),
                Action::Submit {
                    message: message(turn_id),
                },
            ))
            .await
            .unwrap();
    }
    sessions
        .mark_dialog_turn_interrupted(id, "stopped-turn")
        .await
        .unwrap();
    sessions
        .update_session_state_for_turn_if_processing(id, "stopped-turn", SessionState::Idle)
        .await
        .unwrap();
    if retiring {
        scheduler
            .active_turns
            .insert(id.into(), desktop_active_turn("stopped-turn"));
    } else {
        scheduler.hold_managed_queue(id, "Turn interrupted");
    }
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            host_queue_lifecycle_model(),
            sessions.update_session_model_id(id, "queue-lifecycle-model"),
        )
        .await
        .unwrap();
    (scheduler, sessions, root, epoch)
}

async fn process_host_queue_outcome(scheduler: &Arc<DialogScheduler>, outcome: TurnOutcome) {
    let (tx, rx) = mpsc::unbounded_channel();
    tx.send(("host-queue-session".into(), outcome)).unwrap();
    drop(tx);
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            host_queue_lifecycle_model(),
            scheduler.run_outcome_handler(rx),
        )
        .await;
}

#[test]
fn host_queue_prompt_during_stop_retirement_is_not_parked_by_old_outcome() {
    run_host_queue_lifecycle_test(|| async {
        let (scheduler, sessions, _root, epoch) = stopped_host_queue_fixture(true).await;
        let id = "host-queue-session";
        let mut background = standard_queued_turn("background-result");
        background.policy = DialogSubmissionPolicy::for_source(DialogTriggerSource::AgentSession);
        scheduler
            .queues
            .enqueue(id, background, DialogQueuePriority::Low)
            .unwrap();
        let submitted = scheduler
            .manage_host_queue(request(
                Some(&epoch),
                Action::Submit {
                    message: message("after-stop"),
                },
            ))
            .await
            .unwrap();
        assert_eq!(submitted.receipt.unwrap().status, Status::Queued);
        assert!(!sessions
            .latest_dialog_turn_holds_dispatch(id)
            .await
            .unwrap());
        process_host_queue_outcome(
            &scheduler,
            TurnOutcome::Interrupted {
                turn_id: "stopped-turn".into(),
                execution_generation: 0,
            },
        )
        .await;
        assert_eq!(
            sessions.get_turn_count(id),
            2,
            "new prompt must start without another client request"
        );
        let snapshot = scheduler
            .manage_host_queue(request(None, Action::List))
            .await
            .unwrap();
        assert!(snapshot
            .items
            .iter()
            .all(|item| item.turn_id != "after-stop"));
        assert_eq!(
            snapshot
                .items
                .iter()
                .filter(|item| item.status == Status::Blocked)
                .count(),
            2
        );
        // A second explicit prompt must also run while the older held messages
        // remain intact, including when submitted before the current turn ends.
        scheduler
            .manage_host_queue(request(
                Some(&epoch),
                Action::Submit {
                    message: message("another-prompt"),
                },
            ))
            .await
            .unwrap();
        sessions
            .update_session_state(id, SessionState::Idle)
            .await
            .unwrap();
        process_host_queue_outcome(
            &scheduler,
            TurnOutcome::Completed {
                turn_id: "after-stop".into(),
                final_response: "done".into(),
            },
        )
        .await;
        assert_eq!(sessions.get_turn_count(id), 3);
        assert_eq!(
            scheduler.queues.depth(id),
            1,
            "background work must remain parked behind held messages"
        );
        let _ = scheduler
            .coordinator
            .cancel_dialog_turn(id, "another-prompt")
            .await;
    });
}

#[test]
fn host_queue_promote_after_stop_preserves_recovery_on_failure_and_starts_once() {
    run_host_queue_lifecycle_test(|| async {
        let (scheduler, sessions, _root, epoch) = stopped_host_queue_fixture(false).await;
        let id = "host-queue-session";
        let promote = |op: &str| {
            request(
                Some(&epoch),
                Action::Promote {
                    turn_id: "old-a".into(),
                    operation_id: op.into(),
                    expected_active_turn_id: None,
                },
            )
        };
        let failed = TEST_MODEL_RESOLUTION_AI_CONFIG
            .scope(
                AIConfig::default(),
                scheduler.execute_queue_request(promote("failed-promotion")),
            )
            .await
            .unwrap();
        assert_eq!(failed.receipt.unwrap().status, Status::Blocked);
        assert!(sessions
            .latest_dialog_turn_holds_dispatch(id)
            .await
            .unwrap());
        assert_eq!(sessions.get_turn_count(id), 1);
        let accepted = TEST_MODEL_RESOLUTION_AI_CONFIG
            .scope(
                host_queue_lifecycle_model(),
                scheduler.execute_queue_request(promote("retry-promotion")),
            )
            .await
            .unwrap();
        assert_eq!(accepted.receipt.unwrap().status, Status::Started);
        assert!(!sessions
            .latest_dialog_turn_holds_dispatch(id)
            .await
            .unwrap());
        scheduler
            .manage_host_queue(promote("retry-promotion"))
            .await
            .unwrap();
        assert_eq!(sessions.get_turn_count(id), 2);
        assert!(accepted
            .items
            .iter()
            .any(|item| item.turn_id == "old-b" && item.status == Status::Blocked));
        let _ = scheduler.coordinator.cancel_dialog_turn(id, "old-a").await;
    });
}

#[test]
fn host_queue_cancel_after_stop_does_not_resume_interrupted_or_background_work() {
    run_host_queue_lifecycle_test(|| async {
        let (scheduler, sessions, _root, epoch) = stopped_host_queue_fixture(false).await;
        let id = "host-queue-session";
        let mut background = standard_queued_turn("background-result");
        background.policy = DialogSubmissionPolicy::for_source(DialogTriggerSource::AgentSession);
        scheduler
            .queues
            .enqueue(id, background, DialogQueuePriority::Low)
            .unwrap();
        for turn in ["old-a", "old-b"] {
            let cancel = request(
                Some(&epoch),
                Action::Cancel {
                    turn_id: turn.into(),
                    operation_id: format!("cancel-{turn}"),
                },
            );
            for _ in 0..2 {
                let result = scheduler.manage_host_queue(cancel.clone()).await.unwrap();
                assert_eq!(result.receipt.unwrap().status, Status::Cancelled);
            }
        }
        assert!(sessions
            .latest_dialog_turn_holds_dispatch(id)
            .await
            .unwrap());
        assert!(scheduler.try_start_next_queued(id).await.unwrap().is_none());
        assert_eq!(scheduler.queues.depth(id), 1);
        assert_eq!(sessions.get_turn_count(id), 1);
    });
}

#[test]
fn host_queue_new_prompt_after_error_survives_delayed_failure_cleanup() {
    run_host_queue_lifecycle_test(|| async {
        let (scheduler, sessions, _root, epoch) = stopped_host_queue_fixture(true).await;
        let id = "host-queue-session";
        sessions
            .abandon_interrupted_dialog_turn(id, Some("stopped-turn"))
            .await
            .unwrap();
        sessions
            .update_session_state(
                id,
                SessionState::Error {
                    error: "provider failed".into(),
                    recoverable: true,
                },
            )
            .await
            .unwrap();
        let submitted = scheduler
            .manage_host_queue(request(
                Some(&epoch),
                Action::Submit {
                    message: message("after-error"),
                },
            ))
            .await
            .unwrap();
        assert_eq!(submitted.receipt.unwrap().status, Status::Queued);
        process_host_queue_outcome(
            &scheduler,
            TurnOutcome::Failed {
                turn_id: "stopped-turn".into(),
                error: "provider failed".into(),
            },
        )
        .await;
        assert_eq!(sessions.get_turn_count(id), 2);
        let snapshot = scheduler
            .manage_host_queue(request(None, Action::List))
            .await
            .unwrap();
        assert_eq!(snapshot.items.len(), 2);
        assert!(snapshot
            .items
            .iter()
            .all(|item| item.status == Status::Blocked && item.turn_id != "after-error"));
        let _ = scheduler
            .coordinator
            .cancel_dialog_turn(id, "after-error")
            .await;
    });
}

#[tokio::test]
async fn thread_goal_host_queue_promote_activates_once() {
    let (scheduler, sessions, _, root) = test_scheduler_with_persistence(true);
    mark_session_processing(&sessions, &root, "host-queue-session", "active-turn").await;
    scheduler
        .active_turns
        .insert("host-queue-session", desktop_active_turn("active-turn"));
    let epoch = scheduler
        .manage_host_queue(request(None, Action::List))
        .await
        .unwrap()
        .queue_epoch;
    let mut goal_message = message("queued-goal");
    goal_message.content = "/goal finish queued work".into();
    scheduler
        .manage_host_queue(request(
            Some(&epoch),
            Action::Submit {
                message: goal_message,
            },
        ))
        .await
        .unwrap();
    let promote = request(
        Some(&epoch),
        Action::Promote {
            turn_id: "queued-goal".into(),
            operation_id: "promote-goal".into(),
            expected_active_turn_id: Some("active-turn".into()),
        },
    );
    scheduler.manage_host_queue(promote.clone()).await.unwrap();
    scheduler.manage_host_queue(promote).await.unwrap();
    let storage = sessions
        .effective_session_storage_path("host-queue-session")
        .await
        .unwrap();
    let goal = scheduler
        .coordinator
        .get_thread_goal("host-queue-session", &storage)
        .await
        .unwrap()
        .unwrap();
    assert!(goal.is_active());
    assert_eq!(goal.objective, "finish queued work");
    let injections = scheduler
        .round_injection_source
        .take_pending("host-queue-session", "active-turn");
    assert_eq!(injections.len(), 1);
    assert_eq!(injections[0].display_content, "/goal finish queued work");
    assert!(injections[0]
        .content
        .contains("<untrusted_objective>\nfinish queued work"));
}
