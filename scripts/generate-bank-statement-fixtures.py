"""Write the synthetic, password-protected bank-statement PDFs the Rust parser is
tested against.

No client data. Every name, account digit, reference and amount below is
invented; what is carried over from real statements is only the *template*
geometry the SBI and HDFC profiles in `scripts/bank_statement_import.py` were
calibrated on (column bounds, anchors, the stacked SBI date), which is already
public in that module.

Standard library only, and deterministic: the same script writes the same
bytes, so `--check` can prove the committed fixtures are this script's output
rather than a hand-edited file.

Why the text is Courier: a monospaced base-14 font makes every word's right edge
a function of its length (600/1000 em per glyph), so a fragment can be placed to
end *exactly* at a narration cell's wrap edge. That is the case `_dewrap`
exists for, and a proportional font would make "reaches the edge" depend on
glyph metrics this script would have to reproduce.

Encryption is the PDF standard security handler, revision 3 (RC4, 128-bit).
Real statements commonly use AES; RC4 is used here only because it needs no
dependency, and the parser's password handling does not depend on the cipher.
That is a limit of these fixtures, recorded in their PROVENANCE.md.

Usage:
  python3 scripts/generate-bank-statement-fixtures.py DIR          # write
  python3 scripts/generate-bank-statement-fixtures.py DIR --check  # compare
"""

import hashlib
import pathlib
import sys

FONT_SIZE = 7.0
GLYPH = 0.6 * FONT_SIZE  # Courier advance per character, in points

PAD = bytes.fromhex(
    "28BF4E5E4E758A4164004E56FFFA01082E2E00B6D0683E802F0CA9FE6453697A")
PERMISSIONS = -3904


# --------------------------------------------------------------------------- #
# RC4 and the standard security handler (ISO 32000-1, 7.6.3)                  #
# --------------------------------------------------------------------------- #

def rc4(key, data):
    state = list(range(256))
    j = 0
    for i in range(256):
        j = (j + state[i] + key[i % len(key)]) % 256
        state[i], state[j] = state[j], state[i]
    out = bytearray()
    i = j = 0
    for byte in data:
        i = (i + 1) % 256
        j = (j + state[i]) % 256
        state[i], state[j] = state[j], state[i]
        out.append(byte ^ state[(state[i] + state[j]) % 256])
    return bytes(out)


def padded(password):
    return (password.encode("latin-1") + PAD)[:32]


def owner_entry(owner, user):
    """Algorithm 3, revision 3."""
    digest = hashlib.md5(padded(owner)).digest()
    for _ in range(50):
        digest = hashlib.md5(digest).digest()
    key = digest[:16]
    value = rc4(key, padded(user))
    for round_ in range(1, 20):
        value = rc4(bytes(b ^ round_ for b in key), value)
    return value


def file_key(user, owner_value, file_id):
    """Algorithm 2, revision 3."""
    digest = hashlib.md5(
        padded(user) + owner_value
        + PERMISSIONS.to_bytes(4, "little", signed=True) + file_id).digest()
    for _ in range(50):
        digest = hashlib.md5(digest[:16]).digest()
    return digest[:16]


def user_entry(key, file_id):
    """Algorithm 5, revision 3."""
    value = rc4(key, hashlib.md5(PAD + file_id).digest())
    for round_ in range(1, 20):
        value = rc4(bytes(b ^ round_ for b in key), value)
    return value + bytes(16)


def object_key(key, number):
    material = key + number.to_bytes(3, "little") + (0).to_bytes(2, "little")
    return hashlib.md5(material).digest()[:16]


# --------------------------------------------------------------------------- #
# Page content                                                                 #
# --------------------------------------------------------------------------- #

def pdf_string(text):
    return "(" + text.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)") + ")"


def content(height, lines):
    """`lines` are (top, [(x, text), ...]) in the top-left coordinates
    `pdftotext -bbox-layout` reports. The baseline sits below the top by
    Courier's ascent (629/1000 em)."""
    ops = [f"BT /F1 {FONT_SIZE:g} Tf"]
    for top, cells in lines:
        baseline = height - top - 0.629 * FONT_SIZE
        for x, text in cells:
            ops.append(f"1 0 0 1 {x:g} {baseline:.3f} Tm {pdf_string(text)} Tj")
    ops.append("ET")
    return "\n".join(ops).encode("latin-1")


def ending_at(edge, x, text):
    """Assert a fragment's right edge, so a fixture cannot drift off the
    geometry it claims to exercise."""
    right = x + GLYPH * len(text)
    assert abs(right - edge) < 0.01, (text, right, edge)
    return (x, text)


def write_pdf(pages, width, height, user, owner, seed, rotate=0):
    file_id = hashlib.md5(seed.encode("ascii")).digest()
    owner_value = owner_entry(owner, user)
    key = file_key(user, owner_value, file_id)
    user_value = user_entry(key, file_id)

    objects = {}
    page_numbers = []
    next_number = 5
    for page_lines in pages:
        page_number, stream_number = next_number, next_number + 1
        next_number += 2
        page_numbers.append(page_number)
        stream = rc4(object_key(key, stream_number), content(height, page_lines))
        objects[stream_number] = (
            f"<< /Length {len(stream)} >>\nstream\n".encode("ascii")
            + stream + b"\nendstream")
        objects[page_number] = (
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width:g} {height:g}] /Rotate {rotate} "
            f"/Resources << /Font << /F1 3 0 R >> >> /Contents {stream_number} 0 R >>"
        ).encode("ascii")
    objects[1] = b"<< /Type /Catalog /Pages 2 0 R >>"
    kids = " ".join(f"{n} 0 R" for n in page_numbers)
    objects[2] = f"<< /Type /Pages /Kids [{kids}] /Count {len(page_numbers)} >>".encode("ascii")
    objects[3] = (b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier "
                  b"/Encoding /WinAnsiEncoding >>")
    objects[4] = (
        f"<< /Filter /Standard /V 2 /R 3 /Length 128 /P {PERMISSIONS} "
        f"/O <{owner_value.hex()}> /U <{user_value.hex()}> >>").encode("ascii")

    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = {}
    for number in sorted(objects):
        offsets[number] = len(out)
        out += f"{number} 0 obj\n".encode("ascii") + objects[number] + b"\nendobj\n"
    xref = len(out)
    count = max(objects) + 1
    out += f"xref\n0 {count}\n0000000000 65535 f \n".encode("ascii")
    for number in range(1, count):
        out += f"{offsets[number]:010d} 00000 n \n".encode("ascii")
    out += (
        f"trailer\n<< /Size {count} /Root 1 0 R /Encrypt 4 0 R "
        f"/ID [<{file_id.hex()}> <{file_id.hex()}>] >>\n"
        f"startxref\n{xref}\n%%EOF\n").encode("ascii")
    return bytes(out)


# --------------------------------------------------------------------------- #
# HDFC-like statement                                                          #
# --------------------------------------------------------------------------- #
# Column bounds (HDFC profile): date 0-70, narration 70-280 with a wrap edge at
# 240, ref 280-358, value date 358-400, withdrawal 400-480, deposit 480-560,
# balance 560+. Narration starts at x=72, so a 40-character fragment ends at
# exactly 240.

HDFC_HEADER = (
    (60, [(28, "SYNTHETIC STATEMENT - NOT A REAL ACCOUNT")]),
    (150, [(340, "Account No :00000000004321  ZZ01")]),
    (162, [(340, "Cust ID :00000000009876")]),
    (174, [(340, "Phone no. :00000004321")]),
    (200, [(70, "Statement of account")]),
    (220, [(5, "Date"), (72, "Narration"), (282, "Chq./Ref.No."),
           (360, "Value Dt"), (402, "Withdrawal Amt."), (482, "Deposit Amt."),
           (562, "Closing Balance")]),
)

HDFC_PAGE_1 = HDFC_HEADER + (
    # the 12-digit UPI reference is broken mid-token at the 240 edge
    (240, [(2, "01/08/26"),
           ending_at(240, 72, "UPI-NORTHWIND TRADERS-nw@okzz-ZZZZ0-6123"),
           (282, "0000612345678901"), (360, "01/08/26"),
           (482, "10,000.00"), (562, "11,000.00")]),
    (252, [(72, "45678901-PAYMENT")]),
    # a wrap that ends short of the edge ended at a real space
    (272, [(2, "02/08/26"), (72, "NEFT DR-ZZZZ0000001-ACME"),
           (282, "ZZZZN12345678901"), (360, "02/08/26"),
           (402, "2,500.50"), (562, "8,499.50")]),
    (284, [(72, "EXPORTS-MUM-ZZZZN12345678901-BB")]),
    (304, [(2, "03/08/26"),
           (72, "IMPS-712345678901-BLUE RIVER CO-ZZZZ-"),
           (282, "0000712345678901"), (360, "03/08/26"),
           (402, "1,200.00"), (562, "7,299.50")]),
    (316, [(72, "XXXXXXXX0042-RENT")]),
    # a transaction naming the bank must not be read as the page footer
    (336, [(2, "04/08/26"), (72, "TPT-"), (97, "HDFC"), (122, "BANK"), (147, "LIMITED"),
           (282, "0000000000000009"), (360, "04/08/26"),
           (402, "99.50"), (562, "7,200.00")]),
    (400, [(28, "HDFC BANK LIMITED")]),
    # below the footer: must not be read
    (420, [(2, "05/08/26"), (72, "UPI-GHOST-g@zz-ZZZZ0-999999999999-X"),
           (402, "1.00"), (562, "7,199.00")]),
)

HDFC_PAGE_2 = (
    (200, [(70, "Statement of account")]),
    # the ACH reference wraps at the edge, inside the reference
    (240, [(2, "06/08/26"),
           ending_at(240, 72, "ACH D- TP ACH SILVER OAK MUTUAL-12345678"),
           (282, "0000000000000123"), (360, "06/08/26"),
           (402, "5,000.00"), (562, "2,200.00")]),
    (252, [(72, "90")]),
    (272, [(2, "07/08/26"),
           ending_at(240, 72, "UPI-GREEN-FIELD-gf@okzz-ZZZZ0-8123456789"),
           (282, "0000812345678901"), (360, "07/08/26"),
           (482, "1,00,000.00"), (562, "1,02,200.00")]),
    (284, [(72, "01-FEES")]),
    (340, [(200, "STATEMENT SUMMARY")]),
    (352, [(70, "Opening Balance"), (200, "Dr Count"), (300, "Cr Count"),
           (400, "Debits"), (480, "Credits"), (560, "Closing Bal")]),
    (364, [(70, "1,000.00"), (200, "4"), (300, "2"),
           (400, "8,800.00"), (480, "1,10,000.00"), (560, "1,02,200.00")]),
    (400, [(28, "HDFC BANK LIMITED")]),
)

# A later page carries its own top anchor and a row-shaped line. The statement
# already ended at STATEMENT SUMMARY, so nothing here may be read.
HDFC_PAGE_3 = (
    (200, [(70, "Statement of account")]),
    (240, [(2, "08/08/26"), (72, "UPI-PHANTOM-ph@zz-ZZZZ0-111111111111-X"),
           (402, "7.00"), (562, "1,02,193.00")]),
)


# --------------------------------------------------------------------------- #
# SBI-like statement                                                           #
# --------------------------------------------------------------------------- #
# Column bounds (SBI profile): date 0-85, value date 85-140, narration 140-220
# (wrap edge 220), ref 220-299 (wrap edge 299), branch 299-356, debit 356-441,
# credit 441-506, balance 506+. Narration starts at x=142, so an 18-character
# fragment ends at 217.6 (inside the 4pt tolerance); ref starts at x=222, so an
# 18-character fragment ends at 297.6.

SBI_HEADER = (
    (40, [(28, "SYNTHETIC STATEMENT - NOT A REAL ACCOUNT")]),
    (70, [(2, "Account Number"), (100, ":00000000007788")]),
    (82, [(2, "IFSC Code:"), (100, "ZZZZ0000001")]),
    (110, [(2, "Txn Date")]),
    (120, [(88, "Value"), (142, "Description"), (222, "Ref No./Cheque"),
           (300, "Branch"), (400, "Debit"), (460, "Credit"), (510, "Balance")]),
    (130, [(88, "Date"), (222, "No."), (300, "Code")]),
)

SBI_PAGE_1 = SBI_HEADER + (
    # the 12-digit UPI reference is broken mid-token at the narration edge
    # (217.6), and so is the account number in the reference column (297.6)
    (150, [(2, "1 Aug"), (88, "1 Aug 2026"), (142, "BY TRANSFER-"),
           ending_at(297.6, 222, "TRANSFER FROM 4897"),
           (300, "00001"), (460, "2500.00"), (510, "52500.00")]),
    (162, [(2, "2026"), ending_at(217.6, 142, "UPI/CR/71234567890"),
           (222, "2 / BLUE RIVER CO")]),
    (174, [ending_at(217.6, 142, "1/BLUE RIVER CO/ZZ")]),
    (186, [(142, "ZZ/blue@okzz/UPI-")]),
    (206, [(2, "2 Aug"), (88, "2 Aug 2026"), (142, "TO TRANSFER-"),
           (222, "TRANSFER TO 4897"), (300, "00001"),
           (360, "1250.75"), (510, "51249.25")]),
    (218, [(2, "2026"), (142, "INB IMPS/"), (222, "3 / NORTH STAR")]),
    (230, [(142, "812345678901/")]),
)

SBI_PAGE_2 = SBI_HEADER[3:] + (
    (150, [(2, "3 Aug"), (88, "3 Aug 2026"), (142, "BY TRANSFER-"),
           (222, "TRANSFER FROM 4897"), (300, "00001"),
           (460, "749.25"), (510, "51998.50")]),
    (162, [(2, "2026"), (142, "NEFT*ZZZZ0000002*"), (222, "4 / GREEN FIELD")]),
    (174, [(142, "N123456789*GREEN")]),
    (186, [(142, "FIELD LTD--")]),
)


FIXTURES = {
    # opened with the user password
    "hdfc-synthetic.pdf": dict(
        pages=[HDFC_PAGE_1, HDFC_PAGE_2, HDFC_PAGE_3], width=638, height=842,
        user="synthetic-user-4321", owner="synthetic-owner-unused-a", seed="hdfc"),
    # every column bound assumes an upright page, so a rotated one is refused
    "hdfc-rotated.pdf": dict(
        pages=[HDFC_PAGE_1], width=638, height=842,
        user="synthetic-user-4321", owner="synthetic-owner-unused-b", seed="rotated",
        rotate=90),
    # opened only by the owner password: the user password is unknown to the
    # operator, which is the case `pdftotext -upw` rejects
    "sbi-owner-password-only.pdf": dict(
        pages=[SBI_PAGE_1, SBI_PAGE_2], width=595, height=842,
        user="unknown-user-password-7788", owner="synthetic-owner-7788", seed="sbi"),
}


def main(argv):
    if len(argv) not in (2, 3) or (len(argv) == 3 and argv[2] != "--check"):
        raise SystemExit(__doc__)
    directory = pathlib.Path(argv[1])
    check = len(argv) == 3
    stale = []
    for name, spec in FIXTURES.items():
        data = write_pdf(**spec)
        path = directory / name
        if check:
            if not path.exists() or path.read_bytes() != data:
                stale.append(name)
        else:
            directory.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            print(f"wrote {path} ({len(data)} bytes, sha256 {hashlib.sha256(data).hexdigest()})")
    if stale:
        raise SystemExit("stale fixtures: " + ", ".join(stale))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
