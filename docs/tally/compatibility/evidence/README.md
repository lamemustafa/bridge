# Reviewed live evidence

This directory accepts only paired `*.receipt.json` and
`*.attestation.json` files reviewed through a pull request. Never add raw Tally
requests or responses, company names or GUIDs, GSTIN/PAN values, amounts,
narrations, endpoint details, ports, paths, usernames, headers, or raw errors.

Repository-synthetic parser qualification receipts belong elsewhere and are
not valid live compatibility evidence.

Redacted P0 live-evidence findings may use a `p0b-*.md` filename in this
directory. They are exploratory engineering records only: they cannot contain
raw requests or responses, source identifiers, endpoint details, or promoted
compatibility claims, and they do not substitute for a receipt/attestation
pair.
