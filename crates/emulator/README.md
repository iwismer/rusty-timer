# Emulator

Generates reproducible IPICO timing events from YAML scenario definitions.

The primary mode is reader mode: the emulator binds TCP sockets and behaves like
physical IPICO readers. The deterministic P2P E2E stack uses this mode to feed
the real forwarder process.

Scenario files can also carry harness metadata for tests that bypass physical
reader sockets, but production data-plane validation should prefer the real
emulator → forwarder → receiver path.
