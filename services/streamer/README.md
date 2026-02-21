# Streamer

Connects to one or more IPICO readers over TCP and fans out reads to any number of local TCP clients. Optionally backs up all received reads to a file on disk.

## Build

```bash
cargo build --release -p streamer
```

The binary is written to `target/release/streamer`.

## Usage

```
Rusty Timer: Read Streamer

Usage: streamer [OPTIONS] <reader_ip>...

Arguments:
  <reader_ip>...  The socket address of the reader to connect to. Eg. 192.168.0.52:10000

Options:
  -p, --port <port>         The port of the local machine to bind to [default: 10001]
  -t, --type <read_type>    The type of read the reader is sending [default: raw]
                             Possible values: raw, fsls
  -f, --file <file>         The file to output the reads to
  -b, --bibchip <bibchip>   The bib-chip file
  -P, --ppl <participants>  The .ppl participant file (requires --bibchip)
  -B, --buffer              Buffer the output. Use if high CPU use is encountered
  -h, --help                Print help
  -V, --version             Print version
```

## Examples

Connect to a single reader and serve reads on the default port:

```bash
streamer 192.168.0.52:10000
```

Connect to two readers and serve on a custom port:

```bash
streamer -p 9000 192.168.0.52:10000 192.168.0.53:10000
```

Save all reads to a backup file while streaming:

```bash
streamer -f reads.txt 192.168.0.52:10000
```
