#!/usr/bin/env python3
"""Analyze clock sync exchanges from pcapng captures with wall-clock timestamps.

Extracts SET_DATE_TIME / GET_DATE_TIME exchanges and shows both wall-clock
timestamps (from pcapng) and reader-reported timestamps, making it possible
to verify sleep timing and drift calculations.
"""

import os
import struct
import sys


def iter_blocks(data):
    offset = 0
    while offset + 8 <= len(data):
        block_type = struct.unpack_from("<I", data, offset)[0]
        block_total_length = struct.unpack_from("<I", data, offset + 4)[0]
        if block_total_length < 12 or offset + block_total_length > len(data):
            break
        body = data[offset + 8 : offset + block_total_length - 4]
        yield block_type, body
        offset += block_total_length


def parse_tcp_packet(frame):
    if len(frame) < 14:
        return None
    ethertype = struct.unpack_from(">H", frame, 12)[0]
    if ethertype != 0x0800:
        return None
    ip = frame[14:]
    if len(ip) < 20 or ip[9] != 6:
        return None
    ihl = (ip[0] & 0x0F) * 4
    tcp = ip[ihl:]
    if len(tcp) < 20:
        return None
    src_port, dst_port, seq, _ack = struct.unpack_from(">HHII", tcp, 0)
    data_offset = ((tcp[12] >> 4) & 0x0F) * 4
    flags = tcp[13]
    payload = tcp[data_offset:]
    return src_port, dst_port, seq, flags, payload


def try_parse_ab(text, pos):
    if pos + 10 > len(text) or text[pos:pos+2] != "ab":
        return None
    try:
        ll = int(text[pos+4:pos+6], 16)
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
            return text[pos:pos+cs_end]
    except ValueError:
        pass
    return None


def decode_datetime_data(data_hex):
    """Decode BCD datetime from GET/SET data field. Returns (sec, cs_hex, human_str)."""
    if len(data_hex) < 14:
        return None, None, data_hex
    try:
        hr = int(data_hex[8:10])
        mn = int(data_hex[10:12])
        sc = int(data_hex[12:14])
        cs = int(data_hex[14:16], 16) if len(data_hex) >= 16 else 0
        ms = cs * 10
        return sc, cs, f"{hr:02d}:{mn:02d}:{sc:02d}.{ms:03d}"
    except (ValueError, IndexError):
        return None, None, data_hex


def extract_packets_with_timestamps(raw_pcap, port=10000):
    """Extract TCP packets with pcapng timestamps."""
    # Find timestamp resolution from IDB
    ts_resolution = 1e-6  # default: microseconds

    packets = []
    for block_type, body in iter_blocks(raw_pcap):
        if block_type == 1:  # Interface Description Block
            # Could parse ts_resol option, but default is microseconds
            pass
        elif block_type == 6:  # Enhanced Packet Block
            if len(body) < 20:
                continue
            interface_id = struct.unpack_from("<I", body, 0)[0]
            ts_high = struct.unpack_from("<I", body, 4)[0]
            ts_low = struct.unpack_from("<I", body, 8)[0]
            ts = (ts_high << 32) | ts_low
            wall_time = ts * ts_resolution  # seconds since epoch

            captured_len = struct.unpack_from("<I", body, 12)[0]
            frame = body[20 : 20 + captured_len]

            parsed = parse_tcp_packet(frame)
            if parsed is None:
                continue
            src_port, dst_port, seq, flags, payload = parsed
            if src_port != port and dst_port != port:
                continue
            if not payload or (flags & 0x02) or (flags & 0x04):
                continue

            direction = "R->C" if src_port == port else "C->R"
            text = payload.decode("ascii", errors="replace").replace("\r\n", "")
            ab = try_parse_ab(text, 0)
            if ab:
                packets.append((wall_time, direction, ab))

    return packets


def analyze_sync(filepath):
    with open(filepath, "rb") as f:
        raw = f.read()

    packets = extract_packets_with_timestamps(raw)
    if not packets:
        print("No IPICO control frames found")
        return

    # Normalize wall times relative to first packet
    t0 = packets[0][0]

    print(f"{'Wall':>10s}  {'Dir':5s}  {'Instr':20s}  {'Reader Time':>15s}  {'Notes'}")
    print("-" * 80)

    last_set_wall = None
    last_set_target_sec = None
    sync_num = 0

    for wall_time, direction, frame in packets:
        t_rel = wall_time - t0

        # Decode frame
        try:
            ll = int(frame[4:6], 16)
            instr = int(frame[6:8], 16)
        except ValueError:
            continue

        instr_names = {
            0x01: "SET_DATE_TIME",
            0x02: "GET_DATE_TIME",
            0x09: "CONFIG3",
            0x0A: "GET_STATISTICS",
            0x4B: "EXT_STATUS",
            0x4C: "UNSOLICITED_4C",
        }
        instr_name = instr_names.get(instr, f"0x{instr:02x}")

        data = ""
        notes = ""
        reader_time_str = ""

        if ll not in (0xFF, 0x00):
            data = frame[8:8+ll*2]

        if instr == 0x01:  # SET_DATE_TIME
            if direction == "C->R" and data:
                # Client sending SET
                sc, cs, human = decode_datetime_data(data)
                reader_time_str = f"SET -> :{sc:02d}" if sc is not None else data
                last_set_wall = t_rel
                last_set_target_sec = sc
                sync_num += 1
                notes = f"=== SYNC #{sync_num} ==="
            elif direction == "R->C":
                reader_time_str = "ACK"
                if last_set_wall is not None:
                    notes = f"ACK {(t_rel - last_set_wall)*1000:.0f}ms after SET"

        elif instr == 0x02:  # GET_DATE_TIME
            if direction == "C->R":
                reader_time_str = "GET"
                if last_set_wall is not None:
                    notes = f"{(t_rel - last_set_wall)*1000:.0f}ms after SET"
            elif direction == "R->C" and data:
                sc, cs, human = decode_datetime_data(data)
                reader_time_str = human if human else data
                if last_set_wall is not None and sc is not None:
                    dt = (t_rel - last_set_wall) * 1000
                    notes = f"cs=0x{cs:02x}={cs:d}"
                    if last_set_target_sec is not None:
                        if sc == last_set_target_sec:
                            notes += f"  ** ON TARGET :{last_set_target_sec:02d} **"
                        elif sc == (last_set_target_sec - 1) % 60:
                            notes += f"  (still on :{sc:02d}, not yet :{last_set_target_sec:02d})"
                    notes += f"  [{dt:.0f}ms post-SET]"

        elif instr == 0x4C:  # Unsolicited status
            if data and len(data) >= 4:
                offset_hex = data[-4:]
                try:
                    offset_val = int(offset_hex, 16)
                    notes = f"sync_offset={offset_val}ms"
                except ValueError:
                    pass

        else:
            if direction == "C->R":
                reader_time_str = "query"
            else:
                reader_time_str = "response"

        # Only show SET/GET and 0x4C frames (skip EXT_STATUS etc. noise)
        if instr not in (0x01, 0x02, 0x4C):
            continue

        print(f"{t_rel:10.3f}s  {direction:5s}  {instr_name:20s}  {reader_time_str:>15s}  {notes}")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <file.pcapng>", file=sys.stderr)
        sys.exit(1)
    for f in sys.argv[1:]:
        print(f"\n=== {os.path.basename(f)} ===\n")
        analyze_sync(f)
    print()
