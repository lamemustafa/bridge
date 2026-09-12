# Parser fixtures

`*-bbox-capture.xml` are `pdftotext -bbox-layout` output from **real** bank statements, sanitised.
Every coordinate, word break, line break and entity encoding is the PDF producer's; every customer
value is fabricated. They are the only fixtures in this repository that can catch a change in a
bank's statement template or in poppler's serialisation, because they are the only ones this
repository did not write.

Read the banner comment at the top of each file for exactly what is real and what is not.

## Re-deriving them

`../sanitise-bbox-capture.py` is the whole procedure and the fixtures reproduce from it byte for byte:

```bash
pdftotext -bbox-layout -opw "$PASSWORD" statement.pdf raw.xml
python3 scripts/sanitise-bbox-capture.py raw.xml \
  scripts/fixtures/hdfc-bbox-capture.xml hdfc \
  0:0-800 2:200-330,700-800 3:200-300
```

```bash
python3 scripts/sanitise-bbox-capture.py raw.xml \
  scripts/fixtures/sbi-bbox-capture.xml sbi \
  0:90-741 1:0-165
```

The page ranges are chosen to keep each parser rule load-bearing: a full first page with its account
header, a continuation page that repeats the column header while a row is in progress, the page that
carries the end-of-statement marker, and — for HDFC — one page *after* that marker, so the marker
cannot be removed without a test noticing.

The bank argument is a closed parser selection (`hdfc` or `sbi`). The sanitiser parses every selected
source page and the complete generated page set before writing the destination. It refuses empty,
misaligned or party-class-changing evidence; captured geometry remains fixture evidence and does not
qualify raw customer data.

## Adding a capture for a new bank

1. Sanitise, then **diff the result against the source** and scan for surviving tokens (the script's
   `--help` prints the one-liner). Everything that survives should be bank vocabulary.
2. If a customer value survives, do not add it to `TEMPLATE`. Work out why the rule matched it.
3. Byte integrity is enforced: `scripts/fixtures/**` is `-text` in `.gitattributes` and the
   directory is registered in `scripts/check-fixture-byte-integrity.mjs`. Line-ending normalisation
   would rewrite the geometry these fixtures exist to preserve. That is also why the sanitiser
   itself lives in `scripts/`, not here — this directory holds evidence, and a tool whose bytes are
   pinned as evidence is a category error.
