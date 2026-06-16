use thiserror::Error;

/// Stable protocol error codes used by the P2P control/data planes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolErrorCode {
    /// The peers do not share a compatible protocol minor version.
    UnsupportedVersion,
    /// Authentication or authorization failed.
    AuthDenied,
    /// The peer was revoked.
    RevokedPeer,
    /// The requested stream is not known.
    UnknownStream,
    /// The requested stream is disabled.
    StreamDisabled,
    /// The requested cursor is invalid.
    InvalidCursor,
    /// The requested cursor falls outside retained data.
    RetentionGap,
    /// A peer violated the protocol contract.
    ProtocolViolation,
    /// The advertised frame length exceeds the configured cap.
    FrameTooLarge,
    /// A protobuf payload could not be decoded.
    DecodeError,
    /// Backpressure did not clear before the timeout.
    BackpressureTimeout,
    /// An internal error occurred.
    Internal,
}

/// Runtime protocol error with retry and stream metadata.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// The peers do not share a compatible protocol minor version.
    #[error("unsupported protocol version")]
    UnsupportedVersion {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
    /// Authentication or authorization failed.
    #[error("authentication denied")]
    AuthDenied {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
    /// The peer was revoked.
    #[error("peer revoked")]
    RevokedPeer {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
    /// The requested stream is not known.
    #[error("unknown stream")]
    UnknownStream {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
    /// The requested stream is disabled.
    #[error("stream disabled")]
    StreamDisabled {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
    /// The requested cursor is invalid.
    #[error("invalid cursor")]
    InvalidCursor {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
    /// The requested cursor falls outside retained data.
    #[error("retention gap")]
    RetentionGap {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
    /// A peer violated the protocol contract.
    #[error("protocol violation")]
    ProtocolViolation {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
    /// The advertised frame length exceeds the configured cap.
    #[error("frame too large")]
    FrameTooLarge {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
    /// A protobuf payload could not be decoded.
    #[error("decode error: {source}")]
    DecodeError {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
        /// The prost decode error.
        #[source]
        source: prost::DecodeError,
    },
    /// Backpressure did not clear before the timeout.
    #[error("backpressure timeout")]
    BackpressureTimeout {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
    /// An internal error occurred.
    #[error("internal error")]
    Internal {
        /// Whether the operation may be retried.
        retryable: bool,
        /// Stream the error pertains to, if any.
        stream_id: Option<Vec<u8>>,
    },
}

impl ProtocolError {
    /// Returns the stable error code for this error.
    pub const fn code(&self) -> ProtocolErrorCode {
        match self {
            Self::UnsupportedVersion { .. } => ProtocolErrorCode::UnsupportedVersion,
            Self::AuthDenied { .. } => ProtocolErrorCode::AuthDenied,
            Self::RevokedPeer { .. } => ProtocolErrorCode::RevokedPeer,
            Self::UnknownStream { .. } => ProtocolErrorCode::UnknownStream,
            Self::StreamDisabled { .. } => ProtocolErrorCode::StreamDisabled,
            Self::InvalidCursor { .. } => ProtocolErrorCode::InvalidCursor,
            Self::RetentionGap { .. } => ProtocolErrorCode::RetentionGap,
            Self::ProtocolViolation { .. } => ProtocolErrorCode::ProtocolViolation,
            Self::FrameTooLarge { .. } => ProtocolErrorCode::FrameTooLarge,
            Self::DecodeError { .. } => ProtocolErrorCode::DecodeError,
            Self::BackpressureTimeout { .. } => ProtocolErrorCode::BackpressureTimeout,
            Self::Internal { .. } => ProtocolErrorCode::Internal,
        }
    }

    /// Returns whether the operation may be retried.
    pub const fn retryable(&self) -> bool {
        match self {
            Self::UnsupportedVersion { retryable, .. }
            | Self::AuthDenied { retryable, .. }
            | Self::RevokedPeer { retryable, .. }
            | Self::UnknownStream { retryable, .. }
            | Self::StreamDisabled { retryable, .. }
            | Self::InvalidCursor { retryable, .. }
            | Self::RetentionGap { retryable, .. }
            | Self::ProtocolViolation { retryable, .. }
            | Self::FrameTooLarge { retryable, .. }
            | Self::DecodeError { retryable, .. }
            | Self::BackpressureTimeout { retryable, .. }
            | Self::Internal { retryable, .. } => *retryable,
        }
    }

    /// Returns the associated stream id, if this error is stream-scoped.
    pub fn stream_id(&self) -> Option<&[u8]> {
        match self {
            Self::UnsupportedVersion { stream_id, .. }
            | Self::AuthDenied { stream_id, .. }
            | Self::RevokedPeer { stream_id, .. }
            | Self::UnknownStream { stream_id, .. }
            | Self::StreamDisabled { stream_id, .. }
            | Self::InvalidCursor { stream_id, .. }
            | Self::RetentionGap { stream_id, .. }
            | Self::ProtocolViolation { stream_id, .. }
            | Self::FrameTooLarge { stream_id, .. }
            | Self::DecodeError { stream_id, .. }
            | Self::BackpressureTimeout { stream_id, .. }
            | Self::Internal { stream_id, .. } => stream_id.as_deref(),
        }
    }

    pub(crate) fn frame_too_large() -> Self {
        Self::FrameTooLarge {
            retryable: false,
            stream_id: None,
        }
    }

    pub(crate) fn decode_error(source: prost::DecodeError) -> Self {
        Self::DecodeError {
            retryable: false,
            stream_id: None,
            source,
        }
    }

    pub(crate) fn unsupported_version() -> Self {
        Self::UnsupportedVersion {
            retryable: false,
            stream_id: None,
        }
    }
}
