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

The normal Bridge build cannot materialize or send the canary's import payload.
A disabled dispatch-seam feature provides only an opaque, in-memory, redacted
payload-commitment capsule. Sealing consumes the non-cloneable prepared canary,
so one prepared instance cannot yield a second capsule; the capsule has no raw
XML accessor or callback escape hatch.

The separately disabled runtime-dispatch feature adds one constrained internal
sequence: it derives the fixed canary only from an enrolled local company pin,
performs the exact one-time preflight read, repeats durable admission, and then
consumes the capsule once to POST through Bridge's bounded loopback transport.
Its raw request and response remain sealed, it has no generic payload API,
retry loop, persistence hook, or UI route. Only a build with the explicit
non-default `fixture-canary-runtime-dispatch` feature exposes one Tauri command;
that command accepts no payload, XML, target override, retry choice, evidence,
or digest and returns only a final-verdict identifier and timestamp. It rechecks
an explicit disposable-fixture acknowledgement and backup acknowledgement before
starting the sealed sequence. The canonical loopback origin must match the
enrolled source pin before the one-time reservation, and is rechecked from the
prepared fixture before preflight and is revalidated on the exclusive dispatch
lease immediately before import. The coordinator claims durable exact preflight
evidence before its four-request sequence: preflight read, same-lease pinned-GUID
revalidation, one import, and final readback. It stores only a digest-only final
verdict. A failure before the durable
dispatch claim is reported truthfully as no Tally import sent (though local
one-time state may need review). A failure after that claim is an unknown
outcome: do not retry or send another write; inspect Tally and revoke the
fixture before a new enrollment.

Revocation appends a local `operator_revoked` event. It changes the local
candidate gate only and never alters Tally. A revoked fixture requires a new
fresh review and a new complete attestation before it can be enrolled again.
