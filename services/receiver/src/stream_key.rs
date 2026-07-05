use std::borrow::Borrow;

const SEPARATOR: char = '\u{1f}';

#[must_use]
pub(crate) fn is_valid_identity_part(part: &str) -> bool {
    !part.trim().is_empty()
}

#[must_use]
pub(crate) fn is_valid_endpoint_id(endpoint_id: &str) -> bool {
    is_valid_identity_part(endpoint_id) && !endpoint_id.contains(SEPARATOR)
}

/// Receiver-local canonical stream identity: forwarder endpoint + wire stream id.
///
/// Encoded as a single string `{endpoint_id}\u{1F}{stream_id}` for SQLite TEXT
/// columns and map keys. The unit separator cannot appear in an iroh endpoint
/// id (hex/z-base32); the wire stream_id is arbitrary UTF-8 and lives after the
/// first separator, so `split_once` is unambiguous.
///
/// Keep `scripts/e2e/run_stack.py` in sync: the E2E harness mirrors this
/// encoding in Python when asserting receiver database state.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalStreamKey(String);

impl LocalStreamKey {
    #[must_use]
    pub fn new(endpoint_id: &str, wire_stream_id: &str) -> Self {
        assert!(
            is_valid_identity_part(endpoint_id),
            "endpoint_id must not be empty"
        );
        assert!(
            !endpoint_id.contains(SEPARATOR),
            "endpoint_id must not contain separator"
        );
        assert!(
            is_valid_identity_part(wire_stream_id),
            "wire_stream_id must not be empty"
        );
        Self(format!("{endpoint_id}{SEPARATOR}{wire_stream_id}"))
    }

    #[must_use]
    pub fn endpoint_id(&self) -> &str {
        self.parts().0
    }

    #[must_use]
    pub fn wire_stream_id(&self) -> &str {
        self.parts().1
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub(crate) fn display_encoded(encoded: &str) -> String {
        encoded.replace(SEPARATOR, "␟")
    }

    fn parts(&self) -> (&str, &str) {
        self.0
            .split_once(SEPARATOR)
            .expect("local stream key is missing endpoint/stream separator")
    }
}

impl AsRef<str> for LocalStreamKey {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for LocalStreamKey {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for LocalStreamKey {
    /// Renders the encoded key with U+241F (SYMBOL FOR UNIT SEPARATOR) so logs
    /// remain readable and never carry raw U+001F control characters. The
    /// persisted/key encoding returned by [`LocalStreamKey::as_str`] is
    /// unchanged.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&Self::display_encoded(self.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "endpoint_id must not be empty")]
    fn constructor_rejects_whitespace_only_endpoint_id() {
        let _ = LocalStreamKey::new("   ", "stream-1");
    }

    #[test]
    #[should_panic(expected = "wire_stream_id must not be empty")]
    fn constructor_rejects_whitespace_only_wire_stream_id() {
        let _ = LocalStreamKey::new("endpoint-1", "   ");
    }
}
