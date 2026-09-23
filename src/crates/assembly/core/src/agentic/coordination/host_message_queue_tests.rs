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
