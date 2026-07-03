# Emulator

Generates synthetic IPICO chip reads for testing. Can emit reads at a fixed interval or replay reads from a pre-recorded file.

## Build

```bash
cargo build --release -p emulator
```

The binary is written to `target/release/emulator`.

## Usage

```
Read Emulator

Usage: emulator [OPTIONS]

Options:
  -p, --port <port>       The port of the local machine to listen for connections [default: 10001]
  -f, --file <file>       The file to get the reads from
  -d, --delay <delay>     Delay between reads in milliseconds [default: 1000]
  -t, --type <read_type>  The type of read the reader is sending [default: raw]
                          Possible values: raw, fsls
      --verbatim          Emit file reads byte-for-byte without restamping
      --once              Emit each read once and then stop instead of looping forever
      --pause-when-unsubscribed
                          Pause emission while no client is connected and resume on reconnect
  -h, --help              Print help
  -V, --version           Print version
```

## Examples

Emit synthetic reads every second on the default port:

```bash
emulator
```

Replay reads from a previously recorded file:

```bash
emulator -f reads.txt
```

Emit reads at a faster rate (every 100 ms):

```bash
emulator -d 100
```

Replay a deterministic fixture once, preserving timestamps and bytes:

```bash
emulator --file reads.txt --verbatim --once
```
