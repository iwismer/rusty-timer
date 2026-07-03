use bytes::{Buf, BufMut, Bytes, BytesMut};
use prost::Message;

use crate::ProtocolError;

/// Maximum accepted protobuf payload length for a single frame.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// A decoded length-prefixed protobuf payload.
pub type Frame = Bytes;

/// Encodes a protobuf message as a length-prefixed frame.
pub fn encode_frame(message: &impl Message) -> Bytes {
    let len = message.encoded_len();
    let len_u32 = u32::try_from(len).expect("protobuf frame length exceeds u32::MAX");
    let mut buf = BytesMut::with_capacity(4 + len);

    buf.put_u32_le(len_u32);
    message
        .encode(&mut buf)
        .expect("encoding into BytesMut should not fail");

    buf.freeze()
}

/// Decodes and validates a little-endian frame payload length prefix.
pub fn decode_frame_len(len_buf: [u8; 4]) -> Result<usize, ProtocolError> {
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(ProtocolError::frame_too_large(len));
    }

    Ok(len)
}

/// Decodes one complete length-prefixed frame, if enough bytes are buffered.
pub fn decode_frame(buf: &mut BytesMut) -> Result<Option<Frame>, ProtocolError> {
    if buf.len() < 4 {
        return Ok(None);
    }

    let len = decode_frame_len(buf[..4].try_into().expect("slice has 4 bytes"))?;

    if buf.len() < 4 + len {
        return Ok(None);
    }

    buf.advance(4);
    Ok(Some(buf.split_to(len).freeze()))
}

/// Decodes a protobuf message from a previously read length prefix and payload.
pub fn decode_frame_payload<M>(len_buf: [u8; 4], payload: &[u8]) -> Result<M, ProtocolError>
where
    M: Message + Default,
{
    let len = decode_frame_len(len_buf)?;
    if payload.len() != len {
        return Err(ProtocolError::protocol_violation());
    }

    M::decode(payload).map_err(ProtocolError::decode_error)
}

/// Decodes one complete length-prefixed protobuf message, if enough bytes are buffered.
pub fn decode_message_frame<M>(buf: &mut BytesMut) -> Result<Option<M>, ProtocolError>
where
    M: Message + Default,
{
    let Some(frame) = decode_frame(buf)? else {
        return Ok(None);
    };

    M::decode(frame)
        .map(Some)
        .map_err(ProtocolError::decode_error)
}
