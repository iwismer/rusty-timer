use receiver::Db;
use receiver::control_api::{
    self, AppState, ConnectionState, CursorResetRequest, EarliestEpochRequest, ProfileRequest,
    UpdatePortRequest,
};
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
            server_url: "wss://s.com".to_owned(),
            token: "tok".to_owned(),
            receiver_id: None,
        },
    )
    .await
    .unwrap();

    let profile = control_api::get_profile(&state).await.unwrap();
    assert_eq!(profile.server_url, "wss://s.com");
    assert_eq!(profile.token, "tok");
    assert_eq!(profile.receiver_id, "test-receiver");
}

#[tokio::test]
async fn put_profile_with_receiver_id_updates_state() {
    let state = setup();
    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "wss://s.com".to_owned(),
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
            server_url: "wss://s.com".to_owned(),
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
            server_url: "wss://s.com".to_owned(),
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
        rt_protocol::ReceiverMode::Live {
            streams: vec![rt_protocol::StreamRef {
                forwarder_id: "f1".to_owned(),
                reader_ip: "10.0.0.1:10000".to_owned(),
            }],
            earliest_epochs: vec![],
        },
    )
    .await
    .unwrap();

    let mode = control_api::get_mode(&state).await.unwrap();
    if let rt_protocol::ReceiverMode::Live { streams, .. } = &mode {
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
        rt_protocol::ReceiverMode::Live {
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
            server_url: "wss://s.com".to_owned(),
            token: "tok".to_owned(),
            receiver_id: None,
        },
    )
    .await
    .unwrap();

    let result = control_api::put_mode(
        &state,
        rt_protocol::ReceiverMode::Race {
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
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            earliest_epoch: 7,
        },
    )
    .await
    .unwrap();

    let rows = state.db.lock().await.load_earliest_epochs().unwrap();
    assert_eq!(
        rows,
        vec![("f1".to_owned(), "10.0.0.1:10000".to_owned(), 7)]
    );
}

#[tokio::test]
async fn put_earliest_epoch_rejects_negative_values() {
    let state = setup();
    let result = control_api::put_earliest_epoch(
        &state,
        EarliestEpochRequest {
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            earliest_epoch: -1,
        },
    )
    .await;
    assert!(result.is_err());

    let rows = state.db.lock().await.load_earliest_epochs().unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn put_mode_emits_mode_changed_event() {
    let state = setup();
    let mut ui_rx = state.ui_tx.subscribe();

    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "wss://s.com".to_owned(),
            token: "tok".to_owned(),
            receiver_id: None,
        },
    )
    .await
    .unwrap();

    control_api::put_mode(
        &state,
        rt_protocol::ReceiverMode::Race {
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
                rt_protocol::ReceiverMode::Race {
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
            server_url: "wss://s.com".to_owned(),
            token: "tok".to_owned(),
            receiver_id: Some("recv-original".to_owned()),
        },
    )
    .await
    .unwrap();

    control_api::put_profile(
        &state,
        ProfileRequest {
            server_url: "wss://s2.com".to_owned(),
            token: "tok2".to_owned(),
            receiver_id: None,
        },
    )
    .await
    .unwrap();

    let db = state.db.lock().await;
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
            server_url: "wss://s.com".to_owned(),
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
            server_url: "wss://s.com".to_owned(),
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
            server_url: "wss://s.com".to_owned(),
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
        let db = state.db.lock().await;
        db.save_cursor("f1", "10.0.0.1:10000", 1, 10).unwrap();
        db.save_cursor("f2", "10.0.0.2:10000", 2, 20).unwrap();
    }
    let result = control_api::admin_reset_all_cursors(&state).await.unwrap();
    assert_eq!(result["deleted"], 2);
}

#[tokio::test]
async fn admin_reset_all_earliest_epochs_deletes_all() {
    let state = setup();
    {
        let db = state.db.lock().await;
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
    }
    let result = control_api::admin_reset_all_earliest_epochs(&state)
        .await
        .unwrap();
    assert_eq!(result["deleted"], 1);
}

#[tokio::test]
async fn admin_reset_earliest_epoch_per_stream() {
    let state = setup();
    {
        let db = state.db.lock().await;
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
        db.save_earliest_epoch("f2", "10.0.0.2", 3).unwrap();
    }
    control_api::admin_reset_earliest_epoch(
        &state,
        CursorResetRequest {
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1".to_owned(),
        },
    )
    .await
    .unwrap();

    let remaining = state.db.lock().await.load_earliest_epochs().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].0, "f2");
}

#[tokio::test]
async fn admin_purge_subscriptions_deletes_all() {
    let state = setup();
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
    }
    let result = control_api::admin_purge_subscriptions(&state)
        .await
        .unwrap();
    assert_eq!(result["deleted"], 1);
}

#[tokio::test]
async fn admin_purge_subscriptions_requests_reconnect_when_connected() {
    let state = setup();
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
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
        let mut db = state.db.lock().await;
        db.save_profile("wss://s.com", "tok", "check-only", Some("recv-1"))
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
        let mut db = state.db.lock().await;
        db.save_profile("wss://s.com", "tok", "check-only", Some("recv-1"))
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
        let mut db = state.db.lock().await;
        db.save_profile("wss://s.com", "tok", "check-only", Some("recv-1"))
            .unwrap();
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
        db.save_cursor("f1", "10.0.0.1:10000", 1, 10).unwrap();
        db.save_earliest_epoch("f1", "10.0.0.1", 7).unwrap();
    }
    control_api::admin_factory_reset(&state).await.unwrap();

    let profile = control_api::get_profile(&state).await.unwrap();
    assert_eq!(profile.server_url, "");
    assert_eq!(profile.token, "");
}

#[tokio::test]
async fn admin_update_port_sets_override() {
    let state = setup();
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
    }
    control_api::admin_update_port(
        &state,
        UpdatePortRequest {
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1".to_owned(),
            local_port_override: Some(9000),
        },
    )
    .await
    .unwrap();

    let subs = state.db.lock().await.load_subscriptions().unwrap();
    assert_eq!(subs[0].local_port_override, Some(9000));
}

#[tokio::test]
async fn admin_update_port_returns_not_found_for_missing_subscription() {
    let state = setup();
    let result = control_api::admin_update_port(
        &state,
        UpdatePortRequest {
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1".to_owned(),
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
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", Some(9000)).unwrap();
    }
    control_api::admin_update_port(
        &state,
        UpdatePortRequest {
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1".to_owned(),
            local_port_override: None,
        },
    )
    .await
    .unwrap();

    let subs = state.db.lock().await.load_subscriptions().unwrap();
    assert_eq!(subs[0].local_port_override, None);
}

#[tokio::test]
async fn streams_response_includes_cursor_data() {
    let state = setup();
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
        db.save_subscription("f2", "10.0.0.2", None).unwrap();
        db.save_cursor("f1", "10.0.0.1", 5, 42).unwrap();
    }
    let response = control_api::get_streams(&state).await;
    assert_eq!(response.streams.len(), 2);

    let f1 = response
        .streams
        .iter()
        .find(|s| s.forwarder_id == "f1")
        .unwrap();
    assert_eq!(f1.cursor_epoch, Some(5));
    assert_eq!(f1.cursor_seq, Some(42));

    let f2 = response
        .streams
        .iter()
        .find(|s| s.forwarder_id == "f2")
        .unwrap();
    assert_eq!(f2.cursor_epoch, None);
    assert_eq!(f2.cursor_seq, None);
}
