# Synthetic write-canary fixture gate

This gate records a local, revocable operator attestation for a future synthetic
write-canary. It does not construct XML, call Tally, write to Tally, change the
Capability Passport, or establish write support.

## Enrollment prerequisites

1. Use a dedicated disposable synthetic company. An existing demo company is
   not automatically eligible: the operator attestation is a gate, not proof
   that the company is disposable.
2. Do not use customer, personal, or production data.
3. Before any later canary, create an offline backup, record how to restore it,
   and verify the restore path against a separate copy. If this is not possible,
   do not acknowledge the backup guidance and do not proceed.
4. Persist the selected GUID-bearing company scope, then obtain a separate fresh
   Probe review for the local enrollment. A review consumed by setup save cannot
   be reused.

## Local effects and revocation

Enrollment stores only commitment hashes, attestation flags, and local event
timestamps. It does not store fixture content, company names, GUIDs, backup
locations, or free text in the enrollment evidence tables. The UI must continue
to report `write capability: Unknown`.

The normal Bridge build also cannot materialize the canary's import payload. A
separate disabled build feature provides only a one-use, in-memory, redacted
payload capsule with no endpoint, HTTP client, retry loop, persistence hook, or
command. Enabling that feature alone cannot contact Tally; a later reviewed
dispatch coordinator must bind it to the durable exact preflight evidence and
one-time dispatch claim before any request can be introduced.

Revocation appends a local `operator_revoked` event. It changes the local
candidate gate only and never alters Tally. A revoked fixture requires a new
fresh review and a new complete attestation before it can be enrolled again.
