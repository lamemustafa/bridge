# Reviewed live evidence

This directory accepts only paired `*.receipt.json` and
`*.attestation.json` files reviewed through a pull request. Never add raw Tally
requests or responses, company names or GUIDs, GSTIN/PAN values, amounts,
narrations, endpoint details, ports, paths, usernames, headers, or raw errors.

Repository-synthetic parser qualification receipts belong elsewhere and are
not valid live compatibility evidence.

Redacted P0 live-evidence findings do **not** belong in this directory. They are
unsigned exploratory engineering records, and the `gate` command rejects any
filename here that is not a `*.receipt.json`, a `*.attestation.json`,
`README.md`, or `.gitkeep` — deliberately, because a directory that mixes signed
receipts with prose stops meaning "verified". They live one level up, in
`docs/tally/compatibility/`, and never substitute for a receipt/attestation
pair.
