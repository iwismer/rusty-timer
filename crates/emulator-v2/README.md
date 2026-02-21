# emulator-v2

Deterministic multi-reader emulator for integration testing.

## Purpose

Generates reproducible sequences of IPICO timing events from YAML scenario definitions. Supports two modes: reader mode (binds TCP sockets to simulate physical readers) and forwarder mode (connects to the server WebSocket as a fake forwarder). Includes a fault injection framework for testing error handling. Used in integration tests.

## Key types

### Scenario (`scenario` module)

- **`ScenarioConfig`** -- Top-level YAML scenario configuration (mode, seed, readers, optional forwarder settings).
- **`ReaderScenarioConfig`** -- Per-reader configuration (IP, port, read type, chip IDs, event rate, fault schedule).
- **`EmulatorMode`** -- Enum: `Reader` or `Forwarder`.
- **`FaultConfig`** -- A fault injection entry (type, trigger point, duration).
- **`EmulatedEvent`** -- A single generated read event with reader IP, sequence number, timestamp, and raw read line.
- **`ScenarioError`** -- Error type for scenario parsing and validation.

### Functions

- **`load_scenario_from_str(yaml)`** -- Parse a YAML string into a `ScenarioConfig`.
- **`load_scenario_from_file(path)`** -- Parse a YAML scenario file.
- **`generate_reader_events(reader, seed)`** -- Generate a deterministic event sequence for a reader configuration.

### Faults (`faults` module)

- **`FaultSchedule`** -- Parsed fault trigger schedule built from `FaultConfig` entries.
- **`FaultOutcome`** -- Enum of possible outcomes at a given event point: `Normal`, `Jitter`, `Disconnect`, `ReconnectDelay`, `MalformedMessage`, `SlowAck`.
- **`apply_fault_to_event_emission(schedule, event_num)`** -- Determine the fault outcome for a given event emission.
