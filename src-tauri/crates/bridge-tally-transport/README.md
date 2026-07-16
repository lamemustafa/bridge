# Bridge Tally transport

This crate owns Bridge's production XML-over-HTTP boundary for Tally. It is
portable and deliberately has no Tauri, database, credential, or native-library
dependency.

The transport:

- accepts only localhost or literal loopback endpoints;
- disables proxy inheritance and redirects;
- applies one whole-request deadline;
- bounds outbound XML and encoded response bytes;
- handles UTF-8 and BOM-marked UTF-16 through `bridge-tally-protocol`;
- returns closed, privacy-safe error codes rather than URLs or response bodies.

It proves transport behavior only. HTTP success does not establish Tally
application success; callers must still parse the response envelope and require
the appropriate Tally application status and company evidence.
