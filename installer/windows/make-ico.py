#!/usr/bin/env python3
"""Builds installer/windows/barista.ico from site/assets/app-icon-1024.png.

Each size is stored as a PNG inside the ICO (supported since Windows Vista). Resizing uses
macOS's `sips`; on other systems pass already-resized PNGs: make-ico.py out.ico 16.png 32.png ...

    python3 installer/windows/make-ico.py
"""
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SIZES = [16, 24, 32, 48, 64, 256]


def png_size(data: bytes) -> tuple[int, int]:
    assert data[:8] == b'\x89PNG\r\n\x1a\n', 'not a PNG'
    return struct.unpack('>II', data[16:24])


def write_ico(out: Path, images: list[bytes]) -> None:
    header = struct.pack('<HHH', 0, 1, len(images))
    entries, blobs, offset = b'', b'', 6 + 16 * len(images)
    for data in images:
        width, height = png_size(data)
        # 256 is stored as 0 in the one-byte fields.
        entries += struct.pack('<BBBBHHII', width % 256, height % 256, 0, 0, 1, 32, len(data), offset)
        blobs += data
        offset += len(data)
    out.write_bytes(header + entries + blobs)


def main() -> None:
    if len(sys.argv) > 2:
        write_ico(Path(sys.argv[1]), [Path(p).read_bytes() for p in sys.argv[2:]])
        return
    source = ROOT / 'site' / 'assets' / 'app-icon-1024.png'
    out = Path(__file__).with_name('barista.ico')
    with tempfile.TemporaryDirectory() as temporary:
        images = []
        for size in SIZES:
            target = Path(temporary) / f'{size}.png'
            subprocess.run(['sips', '-z', str(size), str(size), str(source), '--out', str(target)],
                           check=True, capture_output=True)
            images.append(target.read_bytes())
    write_ico(out, images)
    print(f'Wrote {out} ({", ".join(map(str, SIZES))} px)')


if __name__ == '__main__':
    main()
