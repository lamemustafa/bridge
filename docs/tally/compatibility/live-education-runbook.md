# Read-only Tally Education qualification

This runbook observes one legitimate local Tally Education profile. It does not
bypass licensing, change system time, send an import, or prove that the local
responder is authentic Tally. Tally must be running with its HTTP server
configured on the selected loopback port. Official prerequisites describe a
loaded company, HTTP POST, port configuration, and supported encodings:
<https://help.tallysolutions.com/pre-requisites-for-integrations/>.

## Prepare a disposable fixture

Use no customer or personal data. In Tally Education, create or load a
disposable company whose name exactly matches the reviewed `company_marker` in
[`fixtures/education-small-v1.json`](./fixtures/education-small-v1.json). Add at
least one synthetic ledger and at least one synthetic voucher dated 1 or 2
April 2026. Exactly one ledger must use the reviewed `ledger_sentinel`, and
exactly one voucher must use the reviewed `voucher_number_sentinel`. Keep all
counts within the reviewed minimum and maximum bounds and keep 3 April 2026
empty. The 1st and 2nd are documented Education
voucher dates; the controller performs reads only and adds no workaround.

From Tally's About page, read the exact Application and Release. Confirm the
visible operating mode, ODBC state, and locale. Do not infer the release from
XML `<VERSION>` or `/status` text.

## Create the ignored local profile

From the repository root:

```powershell
New-Item -ItemType Directory -Force .bridge-live
Copy-Item docs/tally/compatibility/live-profile.example.json .bridge-live/profile.json
```

Edit `.bridge-live/profile.json` and replace every `unknown` value only after
the exact setting was directly observed. Product, release, mode, ODBC state,
and locale must all be exact: an unknown, wildcard, or placeholder value stops
the controller before consent and before any network request. Keep the config
and JSON output as direct children of the repository's canonical
`.bridge-live/` directory; the whole directory is ignored.

Set `no_customer_data_attested` to `true` only after personally confirming the
loaded books contain no customer, personal, or production data. The receipt
records this as your attestation; Bridge does not infer it from the fixture
filename or company marker. A false attestation stops before consent and before
any network request.

## Run

From `src-tauri`:

```powershell
cargo run --locked -p bridge-tally-live-read -- run `
  ../.bridge-live/profile.json ../.bridge-live/receipt.json `
  --consent read-only-synthetic
```

The controller first prints a single-use, run-bound `QUALIFY ...` challenge.
The challenge uses fresh operating-system randomness, expires after five
minutes, and is bound to the full config and endpoint, reviewed fixture,
current source surface, commit/dirty state, executable, and Cargo.lock. Network
reads start only after the exact text is entered, and the source surface is
revalidated immediately before dispatch. It reads loaded-company metadata,
requires one unique GUID-bearing match for the reviewed fixture marker, and
then reads ledgers, an expected-empty voucher range, and an expected-populated
voucher range. The fixture contract is verified only when the unique ledger and
voucher sentinels, count bounds, dates, GUID, and company context all pass. A
marker, sentinel, GUID, company-context, parser, application-status, range,
timeout, or transport failure stops every later read.

The controller next prints the exact privacy-reduced receipt and a
receipt-and-output-bound `SAVE ...` challenge. The controller consumes a
repository-issued output target, revalidates its canonical parent at save time,
and saves atomically only after that exact text is entered. An existing file is
never replaced. Raw
responses, names, GUIDs, GSTIN/PAN values, amounts, narrations, endpoint/port,
paths, headers, and raw errors are never written into the receipt.

## Review authority

A local receipt is observation evidence only. It records that no writes were
attempted and does not establish responder authenticity, accounting
correctness, source completeness/atomicity, Tauri runtime behavior, performance
support, or a support claim. Promotion requires a pull-request review, a
trusted non-revoked Ed25519 attestation, an exact matching matrix cell, and the
release gate.
