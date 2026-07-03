# rt-ui-log

Small shared in-memory log buffer for UI-facing service logs.

The forwarder uses this crate to expose recent structured log entries through
its local status API and embedded UI without making callers scrape process logs.
