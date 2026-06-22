import unittest
from pathlib import Path

from scripts import dev_stack


class DevStackChipTests(unittest.TestCase):
    def test_generated_reads_match_large_bibchip_chip_series(self) -> None:
        bibchip_path = (
            Path(__file__).resolve().parents[2]
            / "test_assets"
            / "bibchip"
            / "large.txt"
        )
        expected_chips = [
            line.split(",", 1)[1].strip()
            for line in bibchip_path.read_text().splitlines()
            if line and line[0].isdigit()
        ][: dev_stack.NUM_FRAMES]

        generated_chips = [
            dev_stack.build_frame(chip_index)[4:16]
            for chip_index in range(1, dev_stack.NUM_FRAMES + 1)
        ]

        self.assertEqual(expected_chips, generated_chips)


if __name__ == "__main__":
    unittest.main()
