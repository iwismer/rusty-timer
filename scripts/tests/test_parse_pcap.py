import contextlib
import io
import unittest
from pathlib import Path

import scripts.parse_pcap as parse_pcap


CAPTURES_DIR = Path(__file__).resolve().parents[2] / "docs/ipico-protocol/captures"


class ProcessFileFlowTests(unittest.TestCase):
    def test_process_file_separates_multi_flow_fsls_capture(self) -> None:
        capture = CAPTURES_DIR / "direct-fslsreads-con-dis.pcapng"

        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            parse_pcap.process_file(str(capture))

        output = stdout.getvalue()

        self.assertIn("192.168.0.155:10000 -> 192.168.0.170:54489", output)
        self.assertIn("192.168.0.155:10000 -> 192.168.0.170:54502", output)
        self.assertEqual(output.count("TAG  reader=00"), 16)
        self.assertIn("aa00058000120e38000e26030713560136b0", output)

    def test_process_file_decodes_tto_enabled_reads(self) -> None:
        capture = CAPTURES_DIR / "fsls-event-tto.pcapng"

        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            parse_pcap.process_file(str(capture))

        output = stdout.getvalue()

        self.assertIn(
            "TAG  reader=00 tag=058000123b32 time=2026-03-08T12:22:02.470 "
            "type=RAW tto=index=06 page=00 status=80",
            output,
        )
        self.assertIn("aa00058000123b3200012603081222022f060080cd", output)


if __name__ == "__main__":
    unittest.main()
