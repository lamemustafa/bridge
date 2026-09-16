"""Fetch the pinned PDFium shared library, refusing any byte it did not expect.

Three digests recorded in packaging/pdfium/pdfium.lock.json are checked, and a
mismatch in any of them is a hard failure, never a warning, because the library
is loaded into Bridge's process and shipped in the MCP bundle:

  * the release archive (byte count and SHA-256), BEFORE anything is extracted;
  * the extracted library (byte count and SHA-256);
  * THIRD_PARTY_LICENSES_PDFIUM.txt, the notice this script writes beside it.

The notice reproduces every licence file the archive ships, byte for byte, under
a fixed header. It is generated rather than committed because the archives do
not carry identical licence bytes: the Windows build's files use CRLF, and its
pdfium.txt drops the `//` comment prefix the macOS build keeps.

Usage:
  python3 scripts/fetch-pdfium.py --platform macos-arm64 --dest DIR [--github-env FILE]

Prints the absolute library path. With --github-env, also appends
BRIDGE_PDFIUM_LIBRARY=<path> to that file (a GitHub Actions environment file).
"""

import argparse
import hashlib
import io
import json
import pathlib
import sys
import tarfile
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
LOCK = ROOT / "packaging" / "pdfium" / "pdfium.lock.json"
NOTICE = "THIRD_PARTY_LICENSES_PDFIUM.txt"


def notice_bytes(lock, pin, files):
    """The licence notice for one archive: a fixed header, then each licence
    file (LICENSE first, then licenses/ in name order) exactly as shipped."""
    header = (
        "PDFium, as redistributed in the Bridge MCP bundle\n"
        f"Source: {lock['source']} release {lock['release']}, asset {pin['asset']}\n"
        f"Archive SHA-256: {pin['sha256']}\n"
        "The licence files that asset ships follow, byte for byte.\n"
    ).encode("utf-8")
    body = b""
    for name in ["LICENSE"] + sorted(n for n in files if n.startswith("licenses/")):
        body += f"\n===== {name} =====\n".encode("utf-8") + files[name]
    return header + body


def verify(label, data, expected_bytes, expected_sha256):
    digest = hashlib.sha256(data).hexdigest()
    if len(data) != expected_bytes or digest != expected_sha256:
        raise SystemExit(
            f"{label}: expected {expected_bytes} bytes sha256 {expected_sha256}, "
            f"got {len(data)} bytes sha256 {digest}; refusing it")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--platform", required=True)
    parser.add_argument("--dest", required=True)
    parser.add_argument("--github-env")
    parser.add_argument("--archive", help=argparse.SUPPRESS)  # a local copy, for tests
    args = parser.parse_args(argv)

    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    pin = lock["platforms"].get(args.platform)
    if pin is None:
        raise SystemExit(f"no pinned PDFium for platform {args.platform!r}")
    if args.archive:
        archive = pathlib.Path(args.archive).read_bytes()
    else:
        url = f"{lock['source']}/releases/download/{lock['release']}/{pin['asset']}"
        with urllib.request.urlopen(url, timeout=120) as response:  # noqa: S310 - pinned https URL
            archive = response.read(pin["bytes"] + 1)
    verify(pin["asset"], archive, pin["bytes"], pin["sha256"])

    files = {}
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as bundle:
        for member in bundle.getmembers():
            name = member.name.lstrip("./")
            if member.isfile() and (name in (pin["library"], "LICENSE")
                                    or name.startswith("licenses/")):
                files[name] = bundle.extractfile(member).read()
    if pin["library"] not in files or "LICENSE" not in files:
        raise SystemExit(f"{pin['asset']} does not contain {pin['library']} and LICENSE")
    verify(pin["library"], files[pin["library"]], pin["library_bytes"], pin["library_sha256"])
    notice = notice_bytes(lock, pin, files)
    verify(NOTICE, notice, pin["notice_bytes"], pin["notice_sha256"])

    dest = pathlib.Path(args.dest).resolve()
    library = dest / pathlib.PurePosixPath(pin["library"]).name
    dest.mkdir(parents=True, exist_ok=True)
    library.write_bytes(files[pin["library"]])
    (dest / NOTICE).write_bytes(notice)
    print(library)
    if args.github_env:
        with open(args.github_env, "a", encoding="utf-8") as handle:
            handle.write(f"BRIDGE_PDFIUM_LIBRARY={library}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
