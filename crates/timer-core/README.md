# timer-core

Core timing models and TCP worker types for the streamer pipeline.

## Purpose

Provides the domain models for chip timing and the TCP worker abstractions used by the streamer and emulator services. The models represent parsed timing data and race results, while the workers manage TCP connections to IPICO readers and downstream clients.

## Key types

### Models (`models` module)

- **`ChipRead`** -- A parsed chip read with tag ID, timestamp, and read type.
- **`ChipBib`** -- Association between a chip tag ID and a bib number.
- **`Participant`** -- Race participant with name, bib, and gender.
- **`RaceResult`** -- Computed race result for a participant.
- **`Message`** -- Internal message type for worker communication.
- **`Timestamp`** -- Date-time representation for timing data.
- **`ReadType`** / **`Gender`** -- Supporting enums.

### Workers (`workers` module)

- **`Client`** -- A single downstream TCP client connection.
- **`ClientConnector`** -- Accepts incoming TCP connections and produces `Client` instances.
- **`TimingReader`** -- Reads IPICO data from a TCP connection to a timing reader.
- **`ClientPool`** -- Manages a pool of connected downstream clients.
- **`ReaderPool`** -- Manages a pool of upstream timing reader connections.

### Utilities (`util` module)

- I/O helpers shared across worker implementations.
