use std::io::Cursor;
use std::sync::Arc;
use tokio::sync::Mutex;

use forwarder::status_http::{StatusConfig, StatusServer};
use forwarder::status_store::SubsystemStatus;
use forwarder::storage::journal::Journal;
use tempfile::tempdir;

#[tokio::test]
async fn test_export_tagdata_endpoints() {
    let dir = tempdir().expect("tempdir failed");
    let db_path = dir.path().join("test_export.sqlite3");
    let mut journal = Journal::open(&db_path).expect("journal open failed");

    // Set up stream 1 with 2 epochs
    journal
        .ensure_stream_state("192.168.1.10:10000", 1)
        .expect("ensure stream 1");
    journal
        .insert_event(
            "192.168.1.10:10000",
            1,
            1,
            Some("2026-09-28T10:00:00.000Z"),
            b"aa00058000123b3200012606251750411376\r\n",
            "raw",
        )
        .expect("insert 1");
    // Insert a control response (should be skipped by export)
    journal
        .insert_event(
            "192.168.1.10:10000",
            1,
            2,
            None,
            b"ab010203040506070809\r\n",
            "control",
        )
        .expect("insert control");

    // Advance to epoch 2 on stream 1
    journal
        .advance_epoch("192.168.1.10:10000", Some("5K Race"))
        .expect("advance stream 1");
    journal
        .insert_event(
            "192.168.1.10:10000",
            2,
            3,
            Some("2026-09-28T10:15:00.000Z"),
            b"aa00058000123b3200012606251750480079\r\n",
            "raw",
        )
        .expect("insert 3");

    // Set up stream 2 with epoch 1
    journal
        .ensure_stream_state("192.168.1.11:10000", 1)
        .expect("ensure stream 2");
    journal
        .insert_event(
            "192.168.1.11:10000",
            1,
            1,
            Some("2026-09-28T10:00:01.000Z"),
            b"aa0105800012860800012606251750434f8a\r\n",
            "raw",
        )
        .expect("insert stream 2");

    let shared_journal = Arc::new(Mutex::new(journal));
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start_with_journal(cfg, subsystem, shared_journal.clone())
        .await
        .expect("server start failed");
    let addr = server.local_addr();

    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    // 1. Test /api/v1/export/epochs
    let resp = client
        .get(format!("http://{addr}/api/v1/export/epochs"))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap();
    assert_eq!(status, 200, "epochs endpoint failed: {body}");
    let epochs_json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(epochs_json["epochs"], serde_json::json!([1, 2]));

    // 2. Test /api/v1/export/epochs for single reader
    let resp = client
        .get(format!(
            "http://{addr}/api/v1/export/epochs?reader=192.168.1.11:10000"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let epochs_json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(epochs_json["epochs"], serde_json::json!([1]));

    // 3. Test /api/v1/export/tagdata (merged all)
    let resp = client
        .get(format!("http://{addr}/api/v1/export/tagdata"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/plain; charset=utf-8"
    );
    assert_eq!(
        resp.headers().get("content-disposition").unwrap(),
        "attachment; filename=\"TAGDATA.TXT\""
    );
    let body = resp.text().await.unwrap();
    // Must contain the 3 valid aa frames, but NOT the ab control frame
    assert!(!body.contains("ab010203040506070809"));
    assert!(body.contains("aa00058000123b3200012606251750411376\r\n"));
    assert!(body.contains("aa00058000123b3200012606251750480079\r\n"));
    assert!(body.contains("aa0105800012860800012606251750434f8a\r\n"));

    // 4. Test /api/v1/export/tagdata filtered by epoch 2
    let resp = client
        .get(format!("http://{addr}/api/v1/export/tagdata?epoch=2"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body, "aa00058000123b3200012606251750480079\r\n");

    // 5. Test /api/v1/export/tagdata filtered by reader
    let resp = client
        .get(format!(
            "http://{addr}/api/v1/export/tagdata?reader=192.168.1.11:10000"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body, "aa0105800012860800012606251750434f8a\r\n");

    // 6. Test /api/v1/export/tagdata?mode=separate (zip)
    let resp = client
        .get(format!("http://{addr}/api/v1/export/tagdata?mode=separate"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/zip"
    );
    assert_eq!(
        resp.headers().get("content-disposition").unwrap(),
        "attachment; filename=\"TAGDATA.ZIP\""
    );
    let zip_bytes = resp.bytes().await.unwrap();
    let reader = Cursor::new(zip_bytes);
    let mut zip_archive = zip::ZipArchive::new(reader).expect("valid zip archive");
    assert_eq!(zip_archive.len(), 2);

    let mut stream1_file = zip_archive
        .by_name("192.168.1.10_10000/TAGDATA.TXT")
        .expect("stream 1 file in zip");
    let mut stream1_content = String::new();
    std::io::Read::read_to_string(&mut stream1_file, &mut stream1_content).unwrap();
    assert_eq!(
        stream1_content,
        "aa00058000123b3200012606251750411376\r\naa00058000123b3200012606251750480079\r\n"
    );
}
