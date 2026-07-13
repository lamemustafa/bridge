# Security policy

## Supported versions

| Version | Security status |
| --- | --- |
| `master` / upcoming `0.2.x` | Receives security fixes before release |
| `v0.1.0` | Historical bootstrap release; unsupported |

There is currently no supported downloadable binary release. CI bundle
artifacts are unsigned smoke evidence and must not be presented as a signed
production release.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability or credential leak.
Use [GitHub private vulnerability reporting](https://github.com/lamemustafa/bridge/security/advisories/new).
If that channel is temporarily unavailable, do not disclose the vulnerability
in a public issue; wait for the private channel to be restored.

Private security reporting supersedes the public Bug/Rectify issue rule in
[AGENTS.md](./AGENTS.md). A sanitized public issue or PR may be opened only
after maintainers determine disclosure is safe. Reporters should receive an
initial acknowledgement through the advisory within seven days; remediation
and disclosure timing depends on impact and coordinated-fix availability.

Do not include exploit details, credentials, customer data, certificate private
key material, certificate dumps, token PINs, API keys, or access tokens in a
public issue, pull request, discussion, screenshot, fixture, or log.

## Privacy and diagnostic data

Repository content and shared diagnostics must use synthetic data. Remove or
replace:

- personal names, email addresses, phone numbers, and account identifiers
- company, tax, ledger, voucher, financial, and document data
- certificate subject, issuer, serial number, fingerprint, and private key data
- PINs, tokens, secrets, session identifiers, and authentication headers
- local usernames, home directories, and absolute checkout paths

Preserve only the smallest redacted excerpt needed to reproduce a problem.
Treat certificate metadata and hardware-token details as sensitive even when
they are not private key material.

## Security review scope

Changes to DSC, credentials, endpoints, Tally data, documents, or persistence
require review of:

- secret lifetime and in-memory handling
- error, tracing, and subprocess output
- file-system and path boundaries
- endpoint scheme, host, and redirect validation
- PKCS#11 library discovery and loading
- migration compatibility and rollback behavior
- Windows and macOS differences

Security-sensitive changes require the security review and impact notes defined
in [AGENTS.md](./AGENTS.md) and [review-checklist.md](./review-checklist.md).
