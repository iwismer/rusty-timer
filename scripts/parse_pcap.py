#!/usr/bin/env python3
"""Parse pcapng files and decode IPICO protocol frames from reassembled TCP streams.

Reassembles TCP streams from pcapng captures of IPICO reader traffic on port
10000, then extracts and decodes aa (tag read) and ab (control) frames.

The IPICO reader's embedded TCP stack has a known quirk where multiple segments
share the same sequence number. This parser handles that by preferring PSH-flagged
segments and limiting each segment group's contribution to the number of bytes the
sequence number actually advances.

Usage:
    python scripts/parse_pcap.py [file1.pcapng file2.pcapng ...]

If no files are given, all .pcapng files in docs/ are parsed.

Only uses Python stdlib (struct, sys, os).
"""

import os
import struct
import sys

# ---------------------------------------------------------------------------
# IPICO instruction codes
# ---------------------------------------------------------------------------

INSTRUCTION_NAMES = {
    0x01: "SET_DATE_TIME",
    0x02: "GET_DATE_TIME",
    0x09: "CONFIG3",
    0x0A: "GET_STATISTICS",
    0x2C: "CONNECTED",
    0x37: "PRINT_BANNER",
    0x4B: "EXT_STATUS",
    0xE0: "UNKNOWN_INIT",
    0xE2: "UNKNOWN_E2",
    0xF2: "ERROR_UNSUPPORTED",
}

# ---------------------------------------------------------------------------
# pcapng block parsing
# ---------------------------------------------------------------------------


def iter_blocks(data):
    """Yield (block_type, block_body) tuples from pcapng data."""
    offset = 0
    while offset + 8 <= len(data):
        block_type = struct.unpack_from("<I", data, offset)[0]
        block_total_length = struct.unpack_from("<I", data, offset + 4)[0]
        if block_total_length < 12 or offset + block_total_length > len(data):
            break
        body = data[offset + 8 : offset + block_total_length - 4]
        yield block_type, body
        offset += block_total_length


# ---------------------------------------------------------------------------
# Network header parsing
# ---------------------------------------------------------------------------


def parse_tcp_packet(frame):
    """Parse an Ethernet frame through to TCP.

    Returns (src_ip, dst_ip, src_port, dst_port, seq, flags, payload)
    or None if parsing fails at any layer.
    """
    # Ethernet
    if len(frame) < 14:
        return None
    ethertype = struct.unpack_from(">H", frame, 12)[0]
    if ethertype != 0x0800:
        return None

    # IPv4
    ip = frame[14:]
    if len(ip) < 20:
        return None
    ihl = (ip[0] & 0x0F) * 4
    if len(ip) < ihl or ip[9] != 6:
        return None
    src_ip = ip[12:16]
    dst_ip = ip[16:20]

    # TCP
    tcp = ip[ihl:]
    if len(tcp) < 20:
        return None
    src_port, dst_port, seq, _ack = struct.unpack_from(">HHII", tcp, 0)
    data_offset = ((tcp[12] >> 4) & 0x0F) * 4
    flags = tcp[13]
    if len(tcp) < data_offset:
        return None
    payload = tcp[data_offset:]
    return src_ip, dst_ip, src_port, dst_port, seq, flags, payload


SYN = 0x02
RST = 0x04
PSH = 0x08


# ---------------------------------------------------------------------------
# TCP stream reassembly
# ---------------------------------------------------------------------------


def format_ipv4(ip_bytes):
    """Format a 4-byte IPv4 address as dotted decimal."""
    return ".".join(str(octet) for octet in ip_bytes)


def extract_tcp_flows(raw_pcap, port=10000):
    """Extract TCP payload segments grouped by reader/client flow.

    Returns a dict keyed by (reader_ip, reader_port, client_ip, client_port),
    where each value is {"c2r": [...], "r2c": [...]} and each segment list
    contains (seq, flags, payload) tuples in capture order.
    """
    flows = {}
    for block_type, body in iter_blocks(raw_pcap):
        if block_type == 6:  # Enhanced Packet Block
            if len(body) < 20:
                continue
            captured_len = struct.unpack_from("<I", body, 12)[0]
            frame = body[20 : 20 + captured_len]
        elif block_type == 3:  # Simple Packet Block
            if len(body) < 4:
                continue
            pkt_len = struct.unpack_from("<I", body, 0)[0]
            frame = body[4 : 4 + pkt_len]
        else:
            continue

        parsed = parse_tcp_packet(frame)
        if parsed is None:
            continue
        src_ip, dst_ip, src_port, dst_port, seq, flags, payload = parsed

        if src_port != port and dst_port != port:
            continue
        if not payload:
            continue
        if flags & SYN or flags & RST:
            continue

        if src_port == port:
            reader_ip = src_ip
            reader_port = src_port
            client_ip = dst_ip
            client_port = dst_port
            direction = "r2c"
        else:
            reader_ip = dst_ip
            reader_port = dst_port
            client_ip = src_ip
            client_port = src_port
            direction = "c2r"

        key = (reader_ip, reader_port, client_ip, client_port)
        flow = flows.setdefault(key, {"c2r": [], "r2c": []})
        flow[direction].append((seq, flags, payload))

    return flows


def extract_tcp_segments(raw_pcap, port_filter):
    """Extract TCP segments from pcapng data for one direction.

    port_filter: callable(src_port, dst_port) -> bool

    Returns a list of (seq, flags, payload) tuples in capture order,
    excluding SYN, RST, and empty-payload packets.
    """
    segments = []
    for block_type, body in iter_blocks(raw_pcap):
        if block_type == 6:  # Enhanced Packet Block
            if len(body) < 20:
                continue
            captured_len = struct.unpack_from("<I", body, 12)[0]
            frame = body[20 : 20 + captured_len]
        elif block_type == 3:  # Simple Packet Block
            if len(body) < 4:
                continue
            pkt_len = struct.unpack_from("<I", body, 0)[0]
            frame = body[4 : 4 + pkt_len]
        else:
            continue

        parsed = parse_tcp_packet(frame)
        if parsed is None:
            continue
        _src_ip, _dst_ip, src_port, dst_port, seq, flags, payload = parsed

        if not port_filter(src_port, dst_port):
            continue
        if not payload:
            continue
        if flags & SYN or flags & RST:
            continue

        segments.append((seq, flags, payload))

    return segments


def reassemble_stream(segments):
    """Reassemble a TCP byte stream from segments.

    Handles the IPICO reader's broken TCP stack where multiple segments share
    the same sequence number. Strategy:
      - Group consecutive same-seq segments.
      - Prefer the PSH-flagged segment (the authoritative send).
      - Limit each group's contribution to (next_group_seq - this_seq) bytes,
        which is how many bytes the reader's seq counter actually advanced.
    """
    if not segments:
        return b""

    buf = bytearray()
    i = 0
    while i < len(segments):
        seq = segments[i][0]

        # Find the extent of same-seq group
        j = i + 1
        while j < len(segments) and segments[j][0] == seq:
            j += 1

        # Determine contribution: bytes until next distinct seq
        if j < len(segments):
            contribution = (segments[j][0] - seq) & 0xFFFFFFFF
            # Sanity check for sequence wrap or anomalies
            if contribution > 65536:
                contribution = len(segments[i][2])
        else:
            contribution = None  # Last group: take everything

        if j - i == 1:
            # Single segment
            payload = segments[i][2]
            buf.extend(payload[:contribution] if contribution is not None else payload)
        else:
            # Multiple same-seq segments: prefer PSH
            psh_payload = None
            for idx in range(i, j):
                if segments[idx][1] & PSH:
                    psh_payload = segments[idx][2]
            chosen = psh_payload if psh_payload else segments[j - 1][2]
            buf.extend(chosen[:contribution] if contribution is not None else chosen)

        i = j

    return bytes(buf)


# ---------------------------------------------------------------------------
# Frame scanning and decoding
# ---------------------------------------------------------------------------


def try_parse_ab_at(text, pos):
    """Try to parse an ab frame starting at text[pos].

    Returns (frame_str, end_pos) or None.
    """
    if pos + 10 > len(text) or text[pos : pos + 2] != "ab":
        return None
    try:
        ll = int(text[pos + 4 : pos + 6], 16)
    except ValueError:
        return None

    if ll == 0xFF or ll == 0x00:
        cs_end = 10
    else:
        cs_end = 8 + ll * 2 + 2

    if pos + cs_end > len(text):
        return None

    try:
        expected = int(text[pos + cs_end - 2 : pos + cs_end], 16)
        actual = sum(text[pos + 2 : pos + cs_end - 2].encode("ascii")) & 0xFF
        if expected == actual:
            return text[pos : pos + cs_end], pos + cs_end
    except ValueError:
        pass
    return None


def try_parse_aa_at(text, pos):
    """Try to parse an aa frame starting at text[pos].

    Returns (frame_str, end_pos) or None.
    """
    if pos + 36 > len(text) or text[pos : pos + 2] != "aa":
        return None
    try:
        expected = int(text[pos + 34 : pos + 36], 16)
        actual = sum(text[pos + 2 : pos + 34].encode("ascii")) & 0xFF
        if expected != actual:
            return None
    except ValueError:
        return None

    frame_len = 36
    if pos + 38 <= len(text) and text[pos + 36 : pos + 38] in ("FS", "LS"):
        frame_len = 38
    return text[pos : pos + frame_len], pos + frame_len


def scan_frames(text):
    """Scan a continuous hex stream for valid aa and ab frames.

    Returns list of (position, frame_string, frame_type) where frame_type
    is 'aa' or 'ab'.
    """
    frames = []
    i = 0
    while i < len(text) - 1:
        if text[i : i + 2] == "ab":
            result = try_parse_ab_at(text, i)
            if result:
                frame_str, end = result
                frames.append((i, frame_str, "ab"))
                i = end
                continue
        if text[i : i + 2] == "aa":
            result = try_parse_aa_at(text, i)
            if result:
                frame_str, end = result
                frames.append((i, frame_str, "aa"))
                i = end
                continue
        i += 1
    return frames


# ---------------------------------------------------------------------------
# Frame decoding
# ---------------------------------------------------------------------------


def decode_aa(frame):
    """Decode an aa (tag read) frame. Returns a dict."""
    d = {"raw": frame, "type": "aa"}
    d["reader_id"] = frame[2:4]
    d["tag_id"] = frame[4:16]
    d["unknown"] = frame[16:20]

    try:
        d["year"] = int(frame[20:22])
        d["month"] = int(frame[22:24])
        d["day"] = int(frame[24:26])
        d["hour"] = int(frame[26:28])
        d["minute"] = int(frame[28:30])
        d["second"] = int(frame[30:32])
        cs = int(frame[32:34], 16)
        d["millis"] = cs * 10
        d["timestamp"] = (
            f"20{d['year']:02d}-{d['month']:02d}-{d['day']:02d}"
            f"T{d['hour']:02d}:{d['minute']:02d}:{d['second']:02d}"
            f".{d['millis']:03d}"
        )
    except (ValueError, IndexError):
        d["timestamp"] = "PARSE_ERROR"

    expected = int(frame[34:36], 16)
    actual = sum(frame[2:34].encode("ascii")) & 0xFF
    d["checksum_ok"] = expected == actual

    if len(frame) == 38:
        d["suffix"] = frame[36:38]
        d["read_type"] = "FSLS"
    else:
        d["suffix"] = None
        d["read_type"] = "RAW"

    return d


def decode_ab(frame):
    """Decode an ab (control) frame. Returns a dict."""
    d = {"raw": frame, "type": "ab"}
    d["reader_id"] = frame[2:4]

    try:
        d["length"] = int(frame[4:6], 16)
        d["instruction"] = int(frame[6:8], 16)
    except ValueError:
        d["error"] = "bad header fields"
        return d

    d["instruction_name"] = INSTRUCTION_NAMES.get(
        d["instruction"], f"UNKNOWN(0x{d['instruction']:02x})"
    )

    ll = d["length"]
    if ll == 0xFF:
        d["mode"] = "GET"
        d["data"] = ""
    elif ll == 0x00:
        d["mode"] = "CMD"
        d["data"] = ""
    else:
        d["mode"] = "DATA"
        d["data"] = frame[8 : 8 + ll * 2]

    d["checksum_ok"] = True  # Already validated by scanner
    return d


def decode_ab_datetime(data_hex):
    """Decode GET_DATE_TIME response data (9 bytes = 18 hex chars).

    Format: YY MM DD WW HH MM SS CC XX
    """
    if len(data_hex) < 16:
        return data_hex
    try:
        yr = int(data_hex[0:2])
        mo = int(data_hex[2:4])
        dy = int(data_hex[4:6])
        _wd = int(data_hex[6:8])
        hr = int(data_hex[8:10])
        mn = int(data_hex[10:12])
        sc = int(data_hex[12:14])
        cs = int(data_hex[14:16], 16)
        ms = cs * 10
        return f"20{yr:02d}-{mo:02d}-{dy:02d}T{hr:02d}:{mn:02d}:{sc:02d}.{ms:03d}"
    except (ValueError, IndexError):
        return data_hex


# ---------------------------------------------------------------------------
# Frame display
# ---------------------------------------------------------------------------


def format_aa(d):
    """Format decoded aa frame for display."""
    tag = d["tag_id"]
    ts = d.get("timestamp", "?")
    rdr = d["reader_id"]
    rt = d["read_type"]
    sfx = f" [{d['suffix']}]" if d.get("suffix") else ""
    return f"TAG  reader={rdr} tag={tag} time={ts} type={rt}{sfx}"


def format_ab(d):
    """Format decoded ab frame for display."""
    if "error" in d:
        return f"CTRL ERROR: {d['error']}"
    rdr = d["reader_id"]
    inst = d["instruction_name"]
    mode = d["mode"]
    data = d["data"]

    # Add human-readable data interpretation for known instructions
    extra = ""
    if d["instruction"] == 0x02 and data:
        extra = f" -> {decode_ab_datetime(data)}"
    elif d["instruction"] == 0x01 and data:
        extra = f" -> {decode_ab_datetime(data)}"

    data_str = f" data={data}" if data else ""
    return f"CTRL reader={rdr} instr={inst} mode={mode}{data_str}{extra}"


# ---------------------------------------------------------------------------
# Banner / non-frame text extraction
# ---------------------------------------------------------------------------


def extract_text_lines(stream_bytes):
    """Extract printable text lines (banners, etc.) from the stream."""
    text = stream_bytes.decode("ascii", errors="replace")
    lines = []
    for line in text.split("\r\n"):
        line = line.strip("\x00").strip()
        # Skip lines that are purely hex frame data (aa/ab frames)
        # Keep lines that contain readable ASCII text
        if not line:
            continue
        # Check if this looks like readable text (contains spaces or letters)
        has_text = any(c.isalpha() and c not in "abcdef" for c in line.lower())
        if has_text and len(line) > 10:
            lines.append(line)
    return lines


# ---------------------------------------------------------------------------
# Main processing per file
# ---------------------------------------------------------------------------


def render_flow(reader_ip, reader_port, client_ip, client_port, c2r_segs, r2c_segs):
    """Render one decoded reader/client TCP flow."""
    c2r_bytes = reassemble_stream(c2r_segs)
    r2c_bytes = reassemble_stream(r2c_segs)

    # C->R: split on \r\n (client commands are cleanly framed)
    c2r_text = c2r_bytes.decode("ascii", errors="replace")
    c2r_lines = [l.strip("\x00").strip() for l in c2r_text.split("\r\n")]
    c2r_lines = [l for l in c2r_lines if l]

    # R->C: strip \r\n, scan for valid frames by checksum
    r2c_text = r2c_bytes.decode("ascii", errors="replace")
    r2c_clean = r2c_text.replace("\r\n", "")
    r2c_scanned = scan_frames(r2c_clean)

    # Also extract banner/text lines from R->C
    r2c_banners = extract_text_lines(r2c_bytes)

    # Build interleaved output using packet ordering
    # We output C->R frames first (they're requests), then R->C (responses)
    # In practice, the traffic is request-response, so showing all C->R first
    # then all R->C per "exchange" is helpful. But since we can't perfectly
    # correlate them without timestamps, we show them grouped.

    reader = f"{format_ipv4(reader_ip)}:{reader_port}"
    client = f"{format_ipv4(client_ip)}:{client_port}"
    print(f"  Flow: {reader} -> {client}")

    frame_num = 0

    # Show banner text if any
    if r2c_banners:
        for banner in r2c_banners:
            print(f"  BANNER: {banner}")
        print()

    # Parse and display C->R frames
    c2r_parsed = []
    for line in c2r_lines:
        result = try_parse_ab_at(line, 0)
        if result:
            frame_str, _ = result
            c2r_parsed.append(("C->R", "ab", frame_str))
        else:
            c2r_parsed.append(("C->R", "unknown", line))

    # Display R->C frames
    r2c_parsed = []
    for _pos, frame_str, kind in r2c_scanned:
        r2c_parsed.append(("R->C", kind, frame_str))

    # Interleave: for each C->R command, find the matching R->C response(s)
    # Simple approach: C->R commands and R->C responses alternate in the
    # typical polling pattern. Show them paired where possible.

    r2c_idx = 0

    # Check if there are unsolicited R->C frames before first C->R command
    # (e.g., the CONNECTED notification)
    if r2c_parsed and c2r_parsed:
        # Show any R->C frames that appear to be unsolicited (before commands)
        # Heuristic: if the first R->C instruction doesn't match any C->R instruction
        while r2c_idx < len(r2c_parsed):
            _, kind, frame = r2c_parsed[r2c_idx]
            if kind == "ab":
                d = decode_ab(frame)
                # Check if this instruction matches any C->R command
                matches_command = False
                for _, ck, cf in c2r_parsed:
                    if ck == "ab":
                        cd = decode_ab(cf)
                        if cd.get("instruction") == d.get("instruction"):
                            matches_command = True
                            break
                if matches_command:
                    break
            r2c_idx += 1
            frame_num += 1
            d = decode_ab(frame) if kind == "ab" else decode_aa(frame)
            detail = format_ab(d) if kind == "ab" else format_aa(d)
            print(f"  [{frame_num:3d}] R->C  {detail}")
            print(f"           raw: {frame}")

    # Now show paired exchanges
    for _, ck, cf in c2r_parsed:
        frame_num += 1
        if ck == "ab":
            d = decode_ab(cf)
            detail = format_ab(d)
        else:
            detail = f"UNKNOWN: {cf!r}"
        print(f"  [{frame_num:3d}] C->R  {detail}")
        print(f"           raw: {cf}")

        # Show matching R->C response(s)
        # Heuristic: after a C->R command, show R->C frames until we see
        # the next C->R-matching instruction or run out
        if ck == "ab" and r2c_idx < len(r2c_parsed):
            c2r_instr = decode_ab(cf).get("instruction")
            # Collect responses until we've consumed the expected response
            found_response = False
            while r2c_idx < len(r2c_parsed):
                _, rk, rf = r2c_parsed[r2c_idx]
                rd = decode_ab(rf) if rk == "ab" else decode_aa(rf)
                r_instr = rd.get("instruction") if rk == "ab" else None

                # If this is a response matching our command, show it
                # Also show any aa frames and non-matching ab frames
                # (they might be async notifications)
                r2c_idx += 1
                frame_num += 1
                detail = format_ab(rd) if rk == "ab" else format_aa(rd)
                print(f"  [{frame_num:3d}] R->C  {detail}")
                print(f"           raw: {rf}")

                if rk == "ab" and r_instr == c2r_instr:
                    found_response = True
                    break

    # Show remaining R->C frames (e.g., tag reads that arrived after commands)
    while r2c_idx < len(r2c_parsed):
        _, rk, rf = r2c_parsed[r2c_idx]
        r2c_idx += 1
        frame_num += 1
        rd = decode_ab(rf) if rk == "ab" else decode_aa(rf)
        detail = format_ab(rd) if rk == "ab" else format_aa(rd)
        print(f"  [{frame_num:3d}] R->C  {detail}")
        print(f"           raw: {rf}")

    if frame_num == 0:
        print("  (no IPICO frames found)")


def process_file(filepath):
    """Parse one pcapng file and print decoded frames."""
    with open(filepath, "rb") as f:
        raw = f.read()

    basename = os.path.basename(filepath)
    print(f"\n{'=' * 76}")
    print(f"  {basename}")
    print(f"{'=' * 76}")

    flows = extract_tcp_flows(raw)
    if not flows:
        print("  (no IPICO frames found)")
        return

    sorted_flows = sorted(
        flows.items(),
        key=lambda item: (
            item[0][0],
            item[0][2],
            item[0][3],
            item[0][1],
        ),
    )

    first = True
    for (reader_ip, reader_port, client_ip, client_port), directions in sorted_flows:
        if not first:
            print()
        first = False
        render_flow(
            reader_ip,
            reader_port,
            client_ip,
            client_port,
            directions["c2r"],
            directions["r2c"],
        )


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def main():
    if len(sys.argv) > 1:
        files = sys.argv[1:]
    else:
        script_dir = os.path.dirname(os.path.abspath(__file__))
        docs_dir = os.path.join(os.path.dirname(script_dir), "docs")
        if not os.path.isdir(docs_dir):
            print(f"docs/ directory not found at {docs_dir}", file=sys.stderr)
            sys.exit(1)
        files = sorted(
            os.path.join(docs_dir, f)
            for f in os.listdir(docs_dir)
            if f.endswith(".pcapng")
        )
        if not files:
            print("No .pcapng files found in docs/", file=sys.stderr)
            sys.exit(1)

    for filepath in files:
        if not os.path.isfile(filepath):
            print(f"File not found: {filepath}", file=sys.stderr)
            continue
        process_file(filepath)

    print()


if __name__ == "__main__":
    main()
