# Sanitized Tally JSONEX structure fixtures

These fixtures are synthetic structural derivatives of Tally Solutions'
official TallyPrime 7.0+ JSON integration examples, reviewed on 2026-07-15:

- https://help.tallysolutions.com/tally-prime-integration-using-json-1/
- https://help.tallysolutions.com/wp-content/uploads/2025/11/Ledger-Collection-Response.docx
- https://help.tallysolutions.com/wp-content/uploads/2025/11/voucher-collection-response.docx

The original downloadable examples are not committed. The checked-in files use
synthetic Bridge names, identifiers, and voucher numbers while retaining the
documented envelope, wrapper, omitted-versus-empty, multilingual, accounting-
value, and nested-array shapes needed for parser tests. They contain no live
Tally capture, customer data, phone/email/address, GST registration, bank
detail, local path, or developer identity.

This corpus is structure evidence only. It does not prove Bridge's custom TDL
JSONEX profile, company identity binding, date-range filtering, completeness,
source atomicity, Education-mode availability, performance, or production
support. Redistribution of the official DOCX assets is not required; this
repository stores only independently authored synthetic test JSON.
