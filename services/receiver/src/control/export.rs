//! Handlers for TAGDATA export commands.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::control_api::AppState;
use crate::error::ReceiverError;
use crate::export::{
    render_merged_tagdata, render_separate_tagdata_zip, render_tagdata_line,
    write_separate_tagdata_files,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportEpochsResponse {
    pub epochs: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportTagdataRequest {
    #[serde(default, alias = "streamId", skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<i64>,
    #[serde(default = "default_merged_mode")]
    pub mode: String,
    #[serde(
        default,
        alias = "destinationDir",
        alias = "destination_dir",
        alias = "destinationPath",
        skip_serializing_if = "Option::is_none"
    )]
    pub destination_path: Option<String>,
}

fn default_merged_mode() -> String {
    "merged".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportTagdataResponse {
    pub read_count: usize,
    pub filename: String,
    pub written_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zip_base64: Option<String>,
}

/// Query distinct epochs for export.
pub async fn get_export_epochs(
    state: &AppState,
    stream_id: Option<String>,
) -> Result<ExportEpochsResponse, ReceiverError> {
    let db = state.storage.db.lock().await;
    let epochs = db
        .load_distinct_received_epochs(stream_id.as_deref())
        .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    Ok(ExportEpochsResponse { epochs })
}

/// Export raw reads to TAGDATA format (merged text or separate zip / files).
pub async fn export_tagdata(
    state: &AppState,
    request: ExportTagdataRequest,
) -> Result<ExportTagdataResponse, ReceiverError> {
    let db = state.storage.db.lock().await;
    let events = db
        .load_raw_export_events(request.stream_id.as_deref(), request.epoch)
        .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    drop(db);

    let read_count = events
        .iter()
        .filter(|e| render_tagdata_line(&e.raw_frame).is_some())
        .count();

    if request.mode == "separate" {
        let filename = "TAGDATA.ZIP".to_string();
        let zip_bytes = render_separate_tagdata_zip(&events)
            .map_err(|e| ReceiverError::Internal(format!("failed to create zip: {e}")))?;

        let mut written_paths = Vec::new();
        if let Some(ref dest) = request.destination_path {
            let p = Path::new(dest);
            if p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
            {
                if let Some(parent) = p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(p, &zip_bytes).map_err(|e| {
                    ReceiverError::Internal(format!("failed to write zip file: {e}"))
                })?;
                written_paths.push(p.to_string_lossy().into_owned());
            } else {
                let written = write_separate_tagdata_files(p, &events)
                    .map_err(|e| ReceiverError::Internal(format!("failed to write files: {e}")))?;
                written_paths = written
                    .into_iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
            }
        }

        let zip_base64 = base64::engine::general_purpose::STANDARD.encode(&zip_bytes);

        Ok(ExportTagdataResponse {
            read_count,
            filename,
            written_paths,
            content: None,
            zip_base64: Some(zip_base64),
        })
    } else {
        let filename = "TAGDATA.TXT".to_string();
        let content = render_merged_tagdata(&events);

        let mut written_paths = Vec::new();
        if let Some(ref dest) = request.destination_path {
            let p = Path::new(dest);
            let target_file = if p.extension().is_some() {
                p.to_path_buf()
            } else {
                p.join("TAGDATA.TXT")
            };
            if let Some(parent) = target_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&target_file, &content).map_err(|e| {
                ReceiverError::Internal(format!("failed to write TAGDATA file: {e}"))
            })?;
            written_paths.push(target_file.to_string_lossy().into_owned());
        }

        Ok(ExportTagdataResponse {
            read_count,
            filename,
            written_paths,
            content: Some(content),
            zip_base64: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, ReceivedEventInsert};

    #[tokio::test]
    async fn test_get_export_epochs_and_export_tagdata() {
        let db = Db::open_in_memory().unwrap();
        db.insert_received_event(&ReceivedEventInsert {
            stream_id: "s1",
            seq: 1,
            epoch: 1,
            raw_frame: b"aa00058000123b3200012606251750411376\r\n",
            read_kind: "tag",
            reader_timestamp: None,
            received_unix_ms: 100,
            dbf_delivered_unix_ms: None,
            chip_id: None,
        })
        .unwrap();

        db.insert_received_event(&ReceivedEventInsert {
            stream_id: "s2",
            seq: 1,
            epoch: 2,
            raw_frame: b"aa00058000999b9900012606251750419999\r\n",
            read_kind: "tag",
            reader_timestamp: None,
            received_unix_ms: 200,
            dbf_delivered_unix_ms: None,
            chip_id: None,
        })
        .unwrap();

        // ab control frame
        db.insert_received_event(&ReceivedEventInsert {
            stream_id: "s1",
            seq: 2,
            epoch: 1,
            raw_frame: b"ab0102030405060708\r\n",
            read_kind: "control",
            reader_timestamp: None,
            received_unix_ms: 300,
            dbf_delivered_unix_ms: None,
            chip_id: None,
        })
        .unwrap();

        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        // Epochs
        let eps = get_export_epochs(&state, None).await.unwrap();
        assert_eq!(eps.epochs, vec![2, 1]);

        let eps_s1 = get_export_epochs(&state, Some("s1".to_string()))
            .await
            .unwrap();
        assert_eq!(eps_s1.epochs, vec![1]);

        // Export all merged
        let res = export_tagdata(
            &state,
            ExportTagdataRequest {
                stream_id: None,
                epoch: None,
                mode: "merged".to_string(),
                destination_path: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(res.read_count, 2);
        assert_eq!(res.filename, "TAGDATA.TXT");
        assert!(res.content.as_ref().unwrap().contains("aa00058000123b32"));
        assert!(res.content.as_ref().unwrap().contains("aa00058000999b99"));
        assert!(!res.content.as_ref().unwrap().contains("ab010203"));

        // Export epoch 1
        let res_ep1 = export_tagdata(
            &state,
            ExportTagdataRequest {
                stream_id: None,
                epoch: Some(1),
                mode: "merged".to_string(),
                destination_path: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(res_ep1.read_count, 1);
        assert!(
            res_ep1
                .content
                .as_ref()
                .unwrap()
                .contains("aa00058000123b32")
        );
        assert!(
            !res_ep1
                .content
                .as_ref()
                .unwrap()
                .contains("aa00058000999b99")
        );

        // Export separate with destination dir
        let temp_dir = tempfile::tempdir().unwrap();
        let dest = temp_dir.path().to_string_lossy().to_string();
        let res_sep = export_tagdata(
            &state,
            ExportTagdataRequest {
                stream_id: None,
                epoch: None,
                mode: "separate".to_string(),
                destination_path: Some(dest),
            },
        )
        .await
        .unwrap();
        assert_eq!(res_sep.filename, "TAGDATA.ZIP");
        assert_eq!(res_sep.read_count, 2);
        assert!(res_sep.zip_base64.is_some());
        assert_eq!(res_sep.written_paths.len(), 2);
        assert!(temp_dir.path().join("s1/TAGDATA.TXT").exists());
        assert!(temp_dir.path().join("s2/TAGDATA.TXT").exists());

        // Export directly to a custom file path
        let custom_file = temp_dir.path().join("CUSTOM_TAGDATA.TXT");
        let res_custom = export_tagdata(
            &state,
            ExportTagdataRequest {
                stream_id: None,
                epoch: None,
                mode: "merged".to_string(),
                destination_path: Some(custom_file.to_string_lossy().to_string()),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            res_custom.written_paths,
            vec![custom_file.to_string_lossy().to_string()]
        );
        assert!(custom_file.exists());
    }
}
