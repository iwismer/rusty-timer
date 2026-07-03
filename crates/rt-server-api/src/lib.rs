pub mod allowlist;
pub mod announcer;
pub mod catalog;
pub mod device;
pub mod forwarders;
pub mod register;
pub mod status;

#[cfg(test)]
mod tests {
    use crate::device::{ApprovalState, DeviceKind};

    #[test]
    fn device_kind_uses_lowercase_wire_values() {
        assert_eq!(DeviceKind::Forwarder.as_str(), "forwarder");
        assert_eq!(DeviceKind::parse("receiver"), Some(DeviceKind::Receiver));
        assert_eq!(
            serde_json::to_string(&DeviceKind::Forwarder).unwrap(),
            "\"forwarder\""
        );
        assert_eq!(
            serde_json::from_str::<DeviceKind>("\"receiver\"").unwrap(),
            DeviceKind::Receiver
        );
        assert_eq!(DeviceKind::parse("FORWARDER"), None);
    }

    #[test]
    fn approval_state_uses_lowercase_wire_values() {
        assert_eq!(ApprovalState::Pending.as_str(), "pending");
        assert_eq!(ApprovalState::parse("active"), Some(ApprovalState::Active));
        assert_eq!(
            serde_json::to_string(&ApprovalState::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::from_str::<ApprovalState>("\"active\"").unwrap(),
            ApprovalState::Active
        );
        assert_eq!(ApprovalState::parse("ACTIVE"), None);
    }

    #[test]
    fn register_dtos_match_existing_wire_shape() {
        let request = crate::register::RegisterRequest {
            endpoint_id: "ep-receiver-1".to_owned(),
            device_kind: "receiver".to_owned(),
            display_name: Some("Finish Line".to_owned()),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({
                "endpoint_id": "ep-receiver-1",
                "device_kind": "receiver",
                "display_name": "Finish Line"
            })
        );

        let unnamed_request = crate::register::RegisterRequest {
            endpoint_id: "ep-forwarder-1".to_owned(),
            device_kind: "forwarder".to_owned(),
            display_name: None,
        };
        assert_eq!(
            serde_json::to_value(&unnamed_request).unwrap(),
            serde_json::json!({
                "endpoint_id": "ep-forwarder-1",
                "device_kind": "forwarder"
            })
        );

        let response = crate::register::RegisterResponse {
            endpoint_id: "ep-forwarder-1".to_owned(),
            device_kind: DeviceKind::Forwarder,
            approval_state: ApprovalState::Pending,
            device_token: None,
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            serde_json::json!({
                "endpoint_id": "ep-forwarder-1",
                "device_kind": "forwarder",
                "approval_state": "pending"
            })
        );
    }

    #[test]
    fn catalog_and_allowlist_dtos_match_existing_wire_shape() {
        let catalog = crate::catalog::ForwarderCatalogRequest {
            endpoint_id: "fwd-node-1".to_owned(),
            display_name: Some("Start Line".to_owned()),
            direct_addrs: vec!["127.0.0.1:12345".to_owned()],
            streams: vec![crate::catalog::ForwarderCatalogStream {
                stream_id: "reader-a".to_owned(),
                epoch: 3,
                next_seq: 42,
            }],
        };
        assert_eq!(
            serde_json::to_value(&catalog).unwrap(),
            serde_json::json!({
                "endpoint_id": "fwd-node-1",
                "display_name": "Start Line",
                "direct_addrs": ["127.0.0.1:12345"],
                "streams": [{"stream_id": "reader-a", "epoch": 3, "next_seq": 42}]
            })
        );

        let response = crate::catalog::ForwarderCatalogResponse {
            endpoint_id: "fwd-node-1".to_owned(),
            stream_count: 1,
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            serde_json::json!({"endpoint_id": "fwd-node-1", "stream_count": 1})
        );

        let update =
            crate::allowlist::ReceiverAllowListResponse::replace(vec!["receiver-a".to_owned()]);
        assert_eq!(
            serde_json::to_value(&update).unwrap(),
            serde_json::json!({"receiver_endpoint_ids": ["receiver-a"], "version": 0})
        );
        assert_eq!(
            serde_json::from_value::<crate::allowlist::ReceiverAllowListResponse>(
                serde_json::json!({"receiver_endpoint_ids": ["receiver-a"]})
            )
            .unwrap()
            .version,
            0
        );
    }

    #[test]
    fn announcer_dtos_match_existing_wire_shape_and_defaults() {
        let request = crate::announcer::PushRowsRequest {
            announcer_source_generation: 3,
            forwarder_endpoint_id: "fwd-endpoint".to_owned(),
            stream_id: "finish-line".to_owned(),
            rows: vec![crate::announcer::PushRow {
                seq: 7,
                chip_id: "000000012345".to_owned(),
                bib: Some(42),
                display_name: "Ada Lovelace".to_owned(),
                reader_timestamp: None,
                received_unix_ms: 1_700_000_000_100,
                division: None,
            }],
            max_list_size: Some(25),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({
                "announcer_source_generation": 3,
                "forwarder_endpoint_id": "fwd-endpoint",
                "stream_id": "finish-line",
                "rows": [{
                    "seq": 7,
                    "chip_id": "000000012345",
                    "bib": 42,
                    "display_name": "Ada Lovelace",
                    "reader_timestamp": null,
                    "received_unix_ms": 1_700_000_000_100_i64,
                    "division": null
                }],
                "max_list_size": 25
            })
        );
        let missing_defaults =
            serde_json::from_value::<crate::announcer::PushRowsRequest>(serde_json::json!({
                "announcer_source_generation": 3,
                "forwarder_endpoint_id": "fwd-endpoint",
                "stream_id": "finish-line",
                "rows": [{
                    "seq": 7,
                    "chip_id": "000000012345",
                    "bib": null,
                    "display_name": "Ada Lovelace",
                    "reader_timestamp": null,
                    "received_unix_ms": 1_700_000_000_100_i64
                }]
            }))
            .unwrap();
        assert_eq!(missing_defaults.rows[0].division, None);
        assert_eq!(missing_defaults.max_list_size, None);

        assert_eq!(
            serde_json::to_value(crate::announcer::TakeoverResponse {
                announcer_source_generation: 9,
            })
            .unwrap(),
            serde_json::json!({"announcer_source_generation": 9})
        );
    }

    #[test]
    fn forwarders_and_status_dtos_match_existing_wire_shape() {
        let forwarders = crate::forwarders::ForwardersResponse {
            forwarders: vec![crate::forwarders::ApprovedForwarder {
                endpoint_id: "fwd-approved".to_owned(),
                display_name: Some("Start Line".to_owned()),
                direct_addrs: vec!["127.0.0.1:5000".to_owned()],
                streams: vec![crate::forwarders::ApprovedForwarderStream {
                    stream_id: "reader-a".to_owned(),
                    epoch: 3,
                    next_seq: 42,
                }],
            }],
        };
        assert_eq!(
            serde_json::to_value(&forwarders).unwrap(),
            serde_json::json!({
                "forwarders": [{
                    "endpoint_id": "fwd-approved",
                    "display_name": "Start Line",
                    "direct_addrs": ["127.0.0.1:5000"],
                    "streams": [{"stream_id": "reader-a", "epoch": 3, "next_seq": 42}]
                }]
            })
        );

        let status = crate::status::StatusResponse {
            announcer_source_generation: 1,
            finisher_count: 2,
            announcer_rows: vec![crate::status::AnnouncerRow {
                forwarder_endpoint_id: "fwd-1".to_owned(),
                stream_id: "finish-line".to_owned(),
                seq: 7,
                chip_id: "000000012345".to_owned(),
                bib: Some(42),
                display_name: "Ada Lovelace".to_owned(),
                reader_timestamp: None,
                received_at: chrono::DateTime::parse_from_rfc3339("2026-07-02T12:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                division: None,
            }],
            devices: vec![crate::status::DeviceRecord {
                endpoint_id: "rx-1".to_owned(),
                device_kind: DeviceKind::Receiver,
                approval_state: ApprovalState::Active,
                display_name: None,
            }],
            forwarders: vec![crate::status::ForwarderRecord {
                endpoint_id: "fwd-1".to_owned(),
                display_name: None,
                direct_addrs: vec![],
                last_seen_unix_ms: 123,
                approval_state: ApprovalState::Pending,
            }],
            forwarder_streams: vec![crate::status::ForwarderStreamRecord {
                stream_id: "reader-a".to_owned(),
                endpoint_id: "fwd-1".to_owned(),
                epoch: 3,
                next_seq: 42,
            }],
        };
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(value["devices"][0]["device_kind"], "receiver");
        assert_eq!(value["devices"][0]["approval_state"], "active");
        assert_eq!(value["forwarders"][0]["approval_state"], "pending");
        assert_eq!(
            value["announcer_rows"][0]["received_at"],
            "2026-07-02T12:00:00Z"
        );
        assert_eq!(value["announcer_rows"][0]["forwarder_endpoint_id"], "fwd-1");
    }
}
