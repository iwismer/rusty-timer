use receiver::Db;
use receiver::control_api::{
    self, AppState, ConnectionState, EarliestEpochRequest, ProfileRequest, StreamRef,
    UpdatePortRequest,
};
use receiver::db::{EventType, StreamEarliestEpoch, StreamSubscription};
use receiver::stream_key::LocalStreamKey;
use std::sync::Arc;

const TEST_RACE_ID: &str = "11111111-1111-1111-1111-111111111111";

fn setup() -> Arc<AppState> {
    let db = Db::open_in_memory().unwrap();
    let (state, _rx) = AppState::new(db, "test-receiver".to_owned());
    state
}

#[tokio::test]
async fn profile_round_trip() {
    let state = setup();
    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin.test".to_owned(),
            token: "tok".to_owned(),
            receiver_id: None,
        },
    )
    .await
    .unwrap();

    let profile = control_api::get_profile(&state).await.unwrap();
    assert_eq!(profile.server_url, "https://thin.test");
    assert_eq!(profile.token, "tok");
    assert_eq!(profile.receiver_id, "test-receiver");
}

#[tokio::test]
async fn put_profile_with_receiver_id_updates_state() {
    let state = setup();
    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin.test".to_owned(),
            token: "tok".to_owned(),
            receiver_id: Some("recv-new".to_owned()),
        },
    )
    .await
    .unwrap();

    let profile = control_api::get_profile(&state).await.unwrap();
    assert_eq!(profile.receiver_id, "recv-new");

    let status = control_api::get_status(&state).await;
    assert_eq!(status.receiver_id, "recv-new");
}

#[tokio::test]
async fn put_profile_with_whitespace_receiver_id_keeps_original() {
    let state = setup();
    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin.test".to_owned(),
            token: "tok".to_owned(),
            receiver_id: Some("  ".to_owned()),
        },
    )
    .await
    .unwrap();

    let profile = control_api::get_profile(&state).await.unwrap();
    assert_eq!(profile.receiver_id, "test-receiver");
}

#[tokio::test]
async fn mode_endpoints_round_trip() {
    let state = setup();
    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin.test".to_owned(),
            token: "tok".to_owned(),
            receiver_id: None,
        },
    )
    .await
    .unwrap();

    let mode_result = control_api::get_mode(&state).await;
    assert!(mode_result.is_err());

    control_api::put_mode(
        &state,
        rt_domain::ReceiverMode::Live {
            streams: vec![rt_domain::StreamRef {
                forwarder_id: "f1".to_owned(),
                reader_ip: "10.0.0.1:10000".to_owned(),
            }],
            earliest_epochs: vec![],
        },
    )
    .await
    .unwrap();

    let mode = control_api::get_mode(&state).await.unwrap();
    if let rt_domain::ReceiverMode::Live { streams, .. } = &mode {
        assert_eq!(streams[0].forwarder_id, "f1");
    } else {
        panic!("expected live mode");
    }
}

#[tokio::test]
async fn put_mode_requires_profile() {
    let state = setup();
    let result = control_api::put_mode(
        &state,
        rt_domain::ReceiverMode::Live {
            streams: vec![],
            earliest_epochs: vec![],
        },
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn put_mode_rejects_invalid_race_id_format() {
    let state = setup();
    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin.test".to_owned(),
            token: "tok".to_owned(),
            receiver_id: None,
        },
    )
    .await
    .unwrap();

    let result = control_api::put_mode(
        &state,
        rt_domain::ReceiverMode::Race {
            race_id: "not-a-uuid".to_owned(),
        },
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn put_earliest_epoch_persists_to_db() {
    let state = setup();
    control_api::put_earliest_epoch(
        &state,
        EarliestEpochRequest {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            earliest_epoch: 7,
        },
    )
    .await
    .unwrap();

    let db = state.storage.db.lock().await;
    // Canonical view is keyed by the encoded local stream key with the
    // forwarder endpoint id.
    assert_eq!(
        db.load_stream_earliest_epochs().unwrap(),
        vec![StreamEarliestEpoch {
            stream_id: LocalStreamKey::new("endpoint-1", "11111111-1111-1111-1111-111111111111")
                .as_str()
                .to_owned(),
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            earliest_epoch: 7,
        }]
    );
}

#[tokio::test]
async fn put_earliest_epoch_rejects_negative_values() {
    let state = setup();
    let result = control_api::put_earliest_epoch(
        &state,
        EarliestEpochRequest {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            earliest_epoch: -1,
        },
    )
    .await;
    assert!(result.is_err());

    let rows = state
        .storage
        .db
        .lock()
        .await
        .load_stream_earliest_epochs()
        .unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn put_mode_emits_mode_changed_event() {
    let state = setup();
    let mut ui_rx = state.ui.ui_tx.subscribe();

    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin.test".to_owned(),
            token: "tok".to_owned(),
            receiver_id: None,
        },
    )
    .await
    .unwrap();

    control_api::put_mode(
        &state,
        rt_domain::ReceiverMode::Race {
            race_id: TEST_RACE_ID.to_owned(),
        },
    )
    .await
    .unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, ui_rx.recv())
            .await
            .expect("timed out waiting for ModeChanged event")
            .unwrap();
        if let receiver::ui_events::ReceiverUiEvent::ModeChanged { mode } = event {
            assert_eq!(
                mode,
                rt_domain::ReceiverMode::Race {
                    race_id: TEST_RACE_ID.to_owned()
                }
            );
            break;
        }
    }
}

#[tokio::test]
async fn put_profile_without_receiver_id_preserves_db_value() {
    let state = setup();

    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin.test".to_owned(),
            token: "tok".to_owned(),
            receiver_id: Some("recv-original".to_owned()),
        },
    )
    .await
    .unwrap();

    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin2.test".to_owned(),
            token: "tok2".to_owned(),
            receiver_id: None,
        },
    )
    .await
    .unwrap();

    let db = state.storage.db.lock().await;
    let profile = db.load_profile().unwrap().unwrap();
    assert_eq!(profile.receiver_id, Some("recv-original".to_owned()));
}

#[tokio::test]
async fn put_profile_rejects_too_long_receiver_id() {
    let state = setup();
    let long_id = "a".repeat(65);
    let result = control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin.test".to_owned(),
            token: "tok".to_owned(),
            receiver_id: Some(long_id),
        },
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn put_profile_rejects_receiver_id_with_special_chars() {
    let state = setup();
    let result = control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin.test".to_owned(),
            token: "tok".to_owned(),
            receiver_id: Some("recv/bad@id".to_owned()),
        },
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn put_profile_accepts_valid_receiver_id() {
    let state = setup();
    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "https://thin.test".to_owned(),
            token: "tok".to_owned(),
            receiver_id: Some("my-recv-01".to_owned()),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn admin_reset_all_cursors_deletes_all() {
    let state = setup();
    {
        let db = state.storage.db.lock().await;
        db.jump_stream_cursor(
            LocalStreamKey::new("endpoint-1", "10.0.0.1:10000").as_str(),
            10,
        )
        .unwrap();
        db.jump_stream_cursor(
            LocalStreamKey::new("endpoint-2", "10.0.0.2:10000").as_str(),
            20,
        )
        .unwrap();
    }
    let result = control_api::admin_reset_all_cursors(&state).await.unwrap();
    assert_eq!(result["deleted"], 2);
    let remaining = state.storage.db.lock().await.load_stream_cursors().unwrap();
    assert!(remaining.is_empty(), "all stream cursors must be deleted");
}

#[tokio::test]
async fn admin_reset_all_earliest_epochs_deletes_all() {
    let state = setup();
    {
        let db = state.storage.db.lock().await;
        db.save_stream_earliest_epoch("endpoint-1", "10.0.0.1", 7)
            .unwrap();
    }
    let result = control_api::admin_reset_all_earliest_epochs(&state)
        .await
        .unwrap();
    assert_eq!(result["deleted"], 1);
    let remaining = state
        .storage
        .db
        .lock()
        .await
        .load_stream_earliest_epochs()
        .unwrap();
    assert!(
        remaining.is_empty(),
        "all stream earliest epochs must be deleted"
    );
}

#[tokio::test]
async fn admin_reset_earliest_epoch_per_stream() {
    let state = setup();
    {
        let db = state.storage.db.lock().await;
        db.save_stream_earliest_epoch("endpoint-1", "11111111-1111-1111-1111-111111111111", 7)
            .unwrap();
        db.save_stream_earliest_epoch("endpoint-2", "22222222-2222-2222-2222-222222222222", 3)
            .unwrap();
    }
    control_api::admin_reset_earliest_epoch(
        &state,
        StreamRef {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
        },
    )
    .await
    .unwrap();

    let remaining = state
        .storage
        .db
        .lock()
        .await
        .load_stream_earliest_epochs()
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].stream_id,
        LocalStreamKey::new("endpoint-2", "22222222-2222-2222-2222-222222222222").as_str()
    );
    assert_eq!(remaining[0].forwarder_endpoint_id, "endpoint-2");
}

#[tokio::test]
async fn admin_purge_subscriptions_deletes_all() {
    let state = setup();
    {
        let mut db = state.storage.db.lock().await;
        db.replace_stream_subscriptions(&[StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "10.0.0.1".to_owned(),
            local_port_override: None,
            event_type: EventType::Finish,
            forwarder_id: Some("f1".to_owned()),
            reader_ip: Some("10.0.0.1".to_owned()),
        }])
        .unwrap();
    }
    let result = control_api::admin_purge_subscriptions(&state)
        .await
        .unwrap();
    assert_eq!(result["deleted"], 1);
    let remaining = state
        .storage
        .db
        .lock()
        .await
        .load_stream_subscriptions()
        .unwrap();
    assert!(remaining.is_empty(), "all subscriptions must be deleted");
}

#[tokio::test]
async fn admin_purge_subscriptions_requests_reconnect_when_connected() {
    let state = setup();
    {
        let mut db = state.storage.db.lock().await;
        db.replace_stream_subscriptions(&[StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "10.0.0.1".to_owned(),
            local_port_override: None,
            event_type: EventType::Finish,
            forwarder_id: Some("f1".to_owned()),
            reader_ip: Some("10.0.0.1".to_owned()),
        }])
        .unwrap();
    }
    state.set_connection_state(ConnectionState::Connected).await;

    let _result = control_api::admin_purge_subscriptions(&state)
        .await
        .unwrap();

    let status = control_api::get_status(&state).await;
    assert_eq!(status.connection_state, ConnectionState::Connecting);
}

#[tokio::test]
async fn admin_reset_profile_clears_credentials() {
    let state = setup();
    {
        let mut db = state.storage.db.lock().await;
        db.save_profile("https://thin.test", "tok", "check-only", Some("recv-1"))
            .unwrap();
    }
    control_api::admin_reset_profile(&state).await.unwrap();

    // After reset, profile should have empty values
    let profile = control_api::get_profile(&state).await.unwrap();
    assert_eq!(profile.server_url, "");
    assert_eq!(profile.token, "");
}

#[tokio::test]
async fn admin_reset_profile_disconnects_when_connected() {
    let state = setup();
    {
        let mut db = state.storage.db.lock().await;
        db.save_profile("https://thin.test", "tok", "check-only", Some("recv-1"))
            .unwrap();
    }
    state.set_connection_state(ConnectionState::Connected).await;

    control_api::admin_reset_profile(&state).await.unwrap();

    let status = control_api::get_status(&state).await;
    assert_eq!(status.connection_state, ConnectionState::Disconnecting);
}

#[tokio::test]
async fn admin_factory_reset_clears_everything() {
    let state = setup();
    {
        let mut db = state.storage.db.lock().await;
        db.save_profile("https://thin.test", "tok", "check-only", Some("recv-1"))
            .unwrap();
        db.replace_stream_subscriptions(&[StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "10.0.0.1".to_owned(),
            local_port_override: None,
            event_type: EventType::Finish,
            forwarder_id: Some("f1".to_owned()),
            reader_ip: Some("10.0.0.1".to_owned()),
        }])
        .unwrap();
        db.jump_stream_cursor(
            LocalStreamKey::new("endpoint-1", "10.0.0.1:10000").as_str(),
            10,
        )
        .unwrap();
        db.save_stream_earliest_epoch("endpoint-1", "10.0.0.1", 7)
            .unwrap();
    }
    control_api::admin_factory_reset(&state).await.unwrap();

    let profile = control_api::get_profile(&state).await.unwrap();
    assert_eq!(profile.server_url, "");
    assert_eq!(profile.token, "");

    let db = state.storage.db.lock().await;
    assert!(
        db.load_stream_subscriptions().unwrap().is_empty(),
        "factory reset must delete subscriptions"
    );
    assert!(
        db.load_stream_cursors().unwrap().is_empty(),
        "factory reset must delete stream cursors"
    );
    assert!(
        db.load_stream_earliest_epochs().unwrap().is_empty(),
        "factory reset must delete earliest epochs"
    );
}

#[tokio::test]
async fn admin_update_port_sets_override() {
    let state = setup();
    {
        let mut db = state.storage.db.lock().await;
        db.replace_stream_subscriptions(&[StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            local_port_override: None,
            event_type: EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();
    }
    control_api::admin_update_port(
        &state,
        UpdatePortRequest {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            local_port_override: Some(9000),
        },
    )
    .await
    .unwrap();

    let subs = state
        .storage
        .db
        .lock()
        .await
        .load_stream_subscriptions()
        .unwrap();
    assert_eq!(subs[0].local_port_override, Some(9000));
}

#[tokio::test]
async fn admin_update_port_returns_not_found_for_missing_subscription() {
    let state = setup();
    let result = control_api::admin_update_port(
        &state,
        UpdatePortRequest {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            local_port_override: Some(9000),
        },
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn admin_update_port_clears_override() {
    let state = setup();
    {
        let mut db = state.storage.db.lock().await;
        db.replace_stream_subscriptions(&[StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            local_port_override: Some(9000),
            event_type: EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();
    }
    control_api::admin_update_port(
        &state,
        UpdatePortRequest {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            local_port_override: None,
        },
    )
    .await
    .unwrap();

    let subs = state
        .storage
        .db
        .lock()
        .await
        .load_stream_subscriptions()
        .unwrap();
    assert_eq!(subs[0].local_port_override, None);
}

#[tokio::test]
async fn streams_response_includes_stored_port_override_separate_from_resolved_port() {
    let state = setup();
    {
        let mut db = state.storage.db.lock().await;
        db.replace_stream_subscriptions(&[
            StreamSubscription {
                forwarder_endpoint_id: "endpoint-default".to_owned(),
                stream_id: "10.0.0.5:10000".to_owned(),
                local_port_override: None,
                event_type: EventType::Finish,
                forwarder_id: None,
                reader_ip: None,
            },
            StreamSubscription {
                forwarder_endpoint_id: "endpoint-explicit".to_owned(),
                stream_id: "10.0.0.6:10000".to_owned(),
                local_port_override: Some(9900),
                event_type: EventType::Finish,
                forwarder_id: None,
                reader_ip: None,
            },
        ])
        .unwrap();
    }

    let response = control_api::get_streams(&state).await;

    let defaulted = response
        .streams
        .iter()
        .find(|s| s.forwarder_endpoint_id == "endpoint-default")
        .unwrap();
    assert_eq!(defaulted.local_port, Some(10005));
    assert_eq!(defaulted.local_port_override, None);
    let serialized_defaulted = serde_json::to_value(defaulted).unwrap();
    assert!(
        serialized_defaulted
            .as_object()
            .unwrap()
            .contains_key("local_port_override")
    );
    assert_eq!(
        serialized_defaulted["local_port_override"],
        serde_json::Value::Null
    );

    let explicit = response
        .streams
        .iter()
        .find(|s| s.forwarder_endpoint_id == "endpoint-explicit")
        .unwrap();
    assert_eq!(explicit.local_port, Some(9900));
    assert_eq!(explicit.local_port_override, Some(9900));
}

#[tokio::test]
async fn streams_response_includes_cursor_data() {
    let state = setup();
    let stream_1 = "127.0.0.1:10000";
    let stream_2 = "127.0.0.1:10001";
    {
        let mut db = state.storage.db.lock().await;
        db.replace_stream_subscriptions(&[
            StreamSubscription {
                forwarder_endpoint_id: "endpoint-1".to_owned(),
                stream_id: stream_1.to_owned(),
                local_port_override: None,
                event_type: EventType::Finish,
                forwarder_id: Some("f1".to_owned()),
                reader_ip: Some("10.0.0.1".to_owned()),
            },
            StreamSubscription {
                forwarder_endpoint_id: "endpoint-2".to_owned(),
                stream_id: stream_2.to_owned(),
                local_port_override: None,
                event_type: EventType::Finish,
                forwarder_id: Some("f2".to_owned()),
                reader_ip: Some("10.0.0.2".to_owned()),
            },
        ])
        .unwrap();
        db.jump_stream_cursor(LocalStreamKey::new("endpoint-1", stream_1).as_str(), 42)
            .unwrap();
    }
    let response = control_api::get_streams(&state).await;
    assert_eq!(response.streams.len(), 2);

    let f1 = response
        .streams
        .iter()
        .find(|s| s.stream_id == stream_1)
        .unwrap();
    assert_eq!(f1.cursor_epoch, None);
    assert_eq!(f1.cursor_seq, Some(42));

    let f2 = response
        .streams
        .iter()
        .find(|s| s.stream_id == stream_2)
        .unwrap();
    assert_eq!(f2.cursor_epoch, None);
    assert_eq!(f2.cursor_seq, None);
}

#[tokio::test]
async fn put_dbf_config_updates_enabled_flag_only() {
    let state = setup();
    let dir = tempfile::tempdir().unwrap();
    {
        let mut db = state.storage.db.lock().await;
        db.save_profile("https://thin.test", "tok", "check-only", Some("recv-1"))
            .unwrap();
        db.save_rd_import_config(&receiver::db::RdImportConfig {
            enabled: false,
            dir: dir.path().to_string_lossy().into_owned(),
            interval_secs: 15,
        })
        .unwrap();
    }

    control_api::put_dbf_config(
        &state,
        receiver::db::DbfConfig {
            enabled: true,
            flush_interval_ms: receiver::db::DEFAULT_DBF_FLUSH_INTERVAL_MS,
        },
    )
    .await
    .unwrap();

    let config = control_api::get_dbf_config(&state).await.unwrap();
    assert!(config.enabled);
}
