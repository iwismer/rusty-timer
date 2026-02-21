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
  -p, --port <port>        The port of the local machine to listen for connections [default: 10001]
  -f, --file <file>        The file to get the reads from
  -d, --delay <delay>      Delay between reads in milliseconds [default: 1000]
  -t, --type <read_type>   The type of read the reader is sending [default: raw]
                            Possible values: raw, fsls
  -h, --help               Print help
  -V, --version            Print version
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
