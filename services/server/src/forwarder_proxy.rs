use crate::state::{AppState, ForwarderCommand, ForwarderProxyReply};
use rt_protocol::{ConfigGetResponse, ConfigSetResponse, RestartResponse};
use std::time::Duration;
use tracing::warn;

const PROXY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum ProxyError {
    NotConnected,
    Disconnected,
    Timeout,
    InternalError(String),
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => write!(f, "forwarder not connected"),
            Self::Disconnected => write!(f, "forwarder disconnected"),
            Self::Timeout => write!(f, "forwarder did not respond in time"),
            Self::InternalError(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ProxyError {}

pub async fn proxy_config_get(
    state: &AppState,
    forwarder_id: &str,
) -> Result<ConfigGetResponse, ProxyError> {
    let tx = get_command_sender(state, forwarder_id).await?;
    let request_id = uuid::Uuid::new_v4().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ForwarderCommand::ConfigGet {
        request_id,
        reply: reply_tx,
    };
    send_command(&tx, cmd).await?;
    match await_reply(reply_rx).await? {
        ForwarderProxyReply::Response(resp) => Ok(resp),
        ForwarderProxyReply::Timeout => Err(ProxyError::Timeout),
        ForwarderProxyReply::InternalError(msg) => Err(ProxyError::InternalError(msg)),
    }
}

pub async fn proxy_config_set(
    state: &AppState,
    forwarder_id: &str,
    section: String,
    payload: serde_json::Value,
) -> Result<ConfigSetResponse, ProxyError> {
    let tx = get_command_sender(state, forwarder_id).await?;
    let request_id = uuid::Uuid::new_v4().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ForwarderCommand::ConfigSet {
        request_id,
        section,
        payload,
        reply: reply_tx,
    };
    send_command(&tx, cmd).await?;
    match await_reply(reply_rx).await? {
        ForwarderProxyReply::Response(resp) => Ok(resp),
        ForwarderProxyReply::Timeout => Err(ProxyError::Timeout),
        ForwarderProxyReply::InternalError(msg) => Err(ProxyError::InternalError(msg)),
    }
}

pub async fn proxy_restart(
    state: &AppState,
    forwarder_id: &str,
) -> Result<RestartResponse, ProxyError> {
    let tx = get_command_sender(state, forwarder_id).await?;
    let request_id = uuid::Uuid::new_v4().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ForwarderCommand::Restart {
        request_id,
        reply: reply_tx,
    };
    send_command(&tx, cmd).await?;
    match await_reply(reply_rx).await? {
        ForwarderProxyReply::Response(resp) => Ok(resp),
        ForwarderProxyReply::Timeout => Err(ProxyError::Timeout),
        ForwarderProxyReply::InternalError(msg) => Err(ProxyError::InternalError(msg)),
    }
}

async fn get_command_sender(
    state: &AppState,
    forwarder_id: &str,
) -> Result<tokio::sync::mpsc::Sender<ForwarderCommand>, ProxyError> {
    let senders = state.forwarder_command_senders.read().await;
    match senders.get(forwarder_id) {
        Some(tx) => Ok(tx.clone()),
        None => {
            warn!(forwarder_id = %forwarder_id, "proxy request for disconnected forwarder");
            Err(ProxyError::NotConnected)
        }
    }
}

async fn send_command(
    tx: &tokio::sync::mpsc::Sender<ForwarderCommand>,
    cmd: ForwarderCommand,
) -> Result<(), ProxyError> {
    match tokio::time::timeout(PROXY_TIMEOUT, tx.send(cmd)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => {
            warn!("forwarder command channel closed during proxy send");
            Err(ProxyError::Disconnected)
        }
        Err(_) => {
            warn!("proxy command send timed out (channel backpressure)");
            Err(ProxyError::Timeout)
        }
    }
}

async fn await_reply<T>(
    rx: tokio::sync::oneshot::Receiver<ForwarderProxyReply<T>>,
) -> Result<ForwarderProxyReply<T>, ProxyError> {
    match tokio::time::timeout(PROXY_TIMEOUT, rx).await {
        Ok(Ok(reply)) => Ok(reply),
        Ok(Err(_)) => {
            warn!("forwarder proxy reply channel closed (forwarder disconnected)");
            Err(ProxyError::Disconnected)
        }
        Err(_) => {
            warn!("forwarder proxy reply timed out");
            Err(ProxyError::Timeout)
        }
    }
}
