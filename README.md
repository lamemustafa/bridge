# ComplyEaze Bridge

ComplyEaze Bridge lets an AI assistant read from, and write to, the TallyPrime
running on your own computer. Nothing in that path copies your books to a server
of ours. What the assistant reads does reach the AI provider you chose, exactly as
the rest of that conversation does — see *One thing to understand before you use it*
below before you point this at client data.

It connects to Tally over Tally's own local XML gateway, on `localhost` only. A
remote Tally host is refused outright rather than supported, so there is no
configuration in which Bridge reaches a book across the internet.

## Is this for you

It is aimed at a practising accountant or a CA firm that already keeps client
books in TallyPrime and wants to ask questions of them, or post entries into
them, through an AI assistant such as Claude Desktop.

**What it does today**

- **Reads** the loaded companies, ledger masters, trial balance, vouchers in a
  date window, outstanding receivables and payables, and ledger movement.
- **Checks ledger names before you post.** Give it the names from a bank
  statement or an invoice and it reports which exist in the book and which are
  near-misses needing your decision. Reading the ledger list first is the single
  biggest cause of an import being rejected wholesale when it is skipped.
- **Records what it did.** Every tool call Bridge runs — read or write, and
  whether it succeeds or is refused — appends a receipt to a log on your own
  machine, naming the company it touched and fingerprinting what was asked and
  what came back. Reads keep those fingerprints as evidence alongside. A
  prepared batch records the local endpoint it was built for, and a native posting
  is refused if that endpoint has changed since; that is a safety check kept in
  Bridge's internal import ledger, not a line in the proof report a reviewer
  opens. A reviewer can read the log rather than take a summary on trust.

**Whether writing is on depends on how you installed it.** Everything above is
reading. When writing is off, the write tools do not merely refuse — they are
**absent from the tool list entirely**, so an assistant cannot see that they
exist.

- **The Claude Desktop extension turns voucher posting off by default.** Two
  known limits in posting remain. The post names its company only by name, and Tally cannot bind an import to a company's GUID. Bridge confirms the company as its last request before the post, and afterwards reports which companies changed, but another loaded company renamed to, or loaded under, the exact same name in that moment would still receive the voucher
  ([#574](https://github.com/lamemustafa/bridge/issues/574)). And Bridge cannot
  delete or roll back a voucher it has posted, so a wrong post must be
  corrected by hand in Tally
  ([#579](https://github.com/lamemustafa/bridge/issues/579)). Turning on
  **Allow voucher posting (Journal, Payment, Receipt, Contra)** in the extension
  settings adds `post_import`, which posts one saved voucher of those types; every
  posting still waits for your approval in a separate Bridge dialog. Leave it
  off unless you accept those risks. Voucher file preparation and bank-statement
  parsing, which write nothing to Tally, stay available with the setting off.
  If you installed an earlier version, check the setting: an earlier default
  may still be saved as on.
- **A source build turns writing off by default.** Preparing a file needs
  `BRIDGE_AGENT_ENABLE_IMPORT`; posting additionally needs
  `BRIDGE_AGENT_ENABLE_WRITES`, which grants both.

With writing on:

- **Prepares vouchers as a local file** — Journal, Payment, Receipt and
  Contra. Bridge writes the file; it does not send it.
- **Posts a single Journal**, and only after you approve that exact voucher in
  a dialog on your own machine. The assistant cannot approve it. Payment,
  Receipt and Contra are prepared but not posted: you import those through
  Tally yourself, and Bridge then reads them back so you can see what actually
  landed.

**What it does not do**

- **Your Tally data is never uploaded.** Bridge reads it over a local
  connection and hands it to the assistant you are talking to; nothing in the
  Tally path sends it to a server of ours.
- It will not post anything without a separate, explicit step after the file is
  prepared.
- It is not a Tally replacement, a reporting suite, or a filing tool.

**One part of the app does upload, and it is not this one.** Bridge also
contains a document feature that uploads files *you* choose to ComplyEaze cloud
storage, and an AXAL sign-in. Those are separate and user-initiated, and share
no code with the Tally path described here. They are compiled into the same
binary the Claude Desktop extension runs, and no Bridge tool can reach them, but
you should know they are present before deciding what to run on a machine
holding client books. Both are documented under *Integration trust
boundaries* below.

**One thing to understand before you use it.** When you ask an AI assistant for
financial data through Bridge, the assistant's provider sees what it reads —
company names, party names and amounts. That is a property of using a hosted
assistant, not of Bridge. Bridge can mask party names or drop narration first
(`BRIDGE_AGENT_REDACTION`), but **neither setting removes amounts** — figures
always go with the answer. Decide this deliberately for client data.

## Installing it

**Before you install, turn on Tally's HTTP gateway.** TallyPrime does not
listen for ComplyEaze Bridge by default. In Tally's own connectivity / client-server
configuration settings, set Tally to act as a server (**"acts as Both"** in
Tally's own words) and note its HTTP gateway port — `9000` by default, but
configurable. To check it is actually on, open `http://localhost:9000/status`
(substitute your port) in a browser: a running gateway answers with a short
Tally XML response, and a browser that cannot connect means the gateway is
still off — **unless Tally is running in a Windows virtual machine on a Mac**,
in which case run this check inside that VM, or only once your local
forwarding is working. A Mac browser that cannot connect may mean the
forwarding described below is missing rather than that the gateway is off.
If instead it hangs without answering, Tally may simply be busy behind
another request — wait and retry rather than changing the setting.

An **unsigned evaluation preview** of the Claude Desktop extension is published
as [`mcp-preview-0.2.0`](https://github.com/lamemustafa/bridge/releases/tag/mcp-preview-0.2.0).
Follow the [installation guide](./docs/agent/INSTALL.md) to install and configure
it. Before you do, know what it is and is not:

- **It is a preview for evaluation, not a production release.** It is not
  code-signed or notarized, so your operating system may warn before opening it.
  Each package has a `.sha256` file and a provenance record so you can confirm
  exactly which bytes and which source commit you downloaded.
- **Checked only as far as launching.** The release build confirms the package
  starts and lists its tools. It does **not** establish that it works against
  your Tally, or in conversation inside Claude Desktop. Validation against Tally
  on Windows is still outstanding.
- **Windows x64 and Apple Silicon Macs only.** Intel Macs are not supported.
- **On a Mac, Tally must run on that same Mac**, in a local Windows virtual
  machine or through approved local forwarding. Bridge only talks to Tally on
  your own computer, so a separate PC or a Tally elsewhere on your network
  cannot be reached by typing its address.
- **It does not update itself.** To upgrade, install a newer release from
  Claude Desktop's Extensions settings.

The Bridge **desktop application** is a separate program and has no published
installer; building it from source is described under *Contributor quick start*
below.

---

The rest of this file is for people working on Bridge. The repository is
self-contained: build and development commands resolve files relative to the
clone, not to a developer-specific directory. It holds a Tauri desktop
application and the MCPB packaging path for Claude Desktop, with
React/TypeScript and Rust components for Tally, document, sync, and local
database operations.

## First useful result

An unsigned evaluation preview of the Claude Desktop extension is published as
[`mcp-preview-0.2.0`](https://github.com/lamemustafa/bridge/releases/tag/mcp-preview-0.2.0);
install it with the [installation guide](./docs/agent/INSTALL.md). For source
use, the contributor quick start below builds the desktop app; to run the MCP
server from source, follow the [source MCP setup](./docs/agent/README.md).

Before requesting financial data through an MCP client, the client may send the selected
Tally result to its AI provider, including company
identity, party or open-bill details, and amounts. Source installations default
to `BRIDGE_AGENT_REDACTION=none`; set it to `mask_parties` or `drop_narration`
before launch when that better fits the workflow. These settings mask party
names or drop narration; they do not remove amounts. The package installation
settings expose the same choices.

For a first result, run `tally_status` to check that TallyPrime and its Licensed
or Education mode are observed, then list the loaded companies. Select a
company with exactly one observed INR currency master and request receivables
or payables. In Education mode, explicitly supply an `as_of` date on day 1, 2,
or 31; an omitted date defaults to today and may be refused. Bridge rechecks
product, mode, dates and currency for the financial read; other or unobserved
products/modes, no currency master, non-INR, or multiple currency masters are
refused.

The desktop also offers [local XML draft preparation](./docs/source-drafts.md) through **Prepare file**. It preserves source observations beside editable proposals and saves a local draft for later review.

For contributors, use the setup and development path below.

## Supported development hosts

Bridge is intended to build and run on Windows and macOS 12.4 or later. Run
platform checks on a native host for each operating system; a successful build
on one operating system does not verify the other.

Shared prerequisites:

- Node.js 24 (>=24.15.0) and Corepack (`.node-version` pins the CI baseline)
- the Rust toolchain pinned by `rust-toolchain.toml`
- Perl 5 with `Locale::Maketext::Simple` for the bundled SQLCipher/OpenSSL build
- LLVM/libclang for SQLCipher binding generation (`LIBCLANG_PATH` may be required)
- the operating-system dependencies listed in the
  [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)

On Windows, install the Microsoft C++ build tools, WebView2 components, and a
complete Perl distribution such as Strawberry Perl. If another incomplete
`perl.exe` appears first on `PATH`, set `OPENSSL_SRC_PERL` to the complete Perl
executable. Install LLVM as well; if `libclang.dll` is not discoverable, set
`LIBCLANG_PATH` to its directory (commonly `C:\Program Files\LLVM\bin`). On
macOS, install Xcode Command Line Tools. Bridge's macOS bundles require macOS
12.4 or later.

## Contributor quick start

Run these commands from the repository root in PowerShell, Command Prompt, or a
POSIX-compatible shell:

```text
corepack pnpm install --frozen-lockfile
corepack pnpm exec playwright install chromium webkit
corepack pnpm test
corepack pnpm run build
corepack pnpm run cargo:check
corepack pnpm run tauri:dev
```

`pnpm test` includes Chromium and WebKit evidence-drawer focus suites; installing
the lock-pinned browsers after dependencies is therefore required once for each
developer environment. `tauri:dev` starts the Vite development server and
desktop application. It does not require a fixed checkout location. The first
Rust build can take several minutes.

For a release build, run `corepack pnpm run tauri:build` on each target host.
CI-produced bundles are unsigned smoke artifacts only. Do not redistribute a
desktop installer until the signing, notarization, provenance, and rollback
gates in [the release runbook](./docs/release-process.md) are complete.

## Platform verification

Before claiming support for a platform, run the following on that platform:

```text
corepack pnpm install --frozen-lockfile
corepack pnpm run build
corepack pnpm run cargo:check
corepack pnpm run tauri:build
```

Also manually exercise the affected Tally, document, and sync workflows.
Vendor integrations may require host-specific software even though repository
paths and project commands are portable.

## Integration trust boundaries

Bridge restricts native network and file access even if the renderer is
compromised:

- Tally connections are loopback-only (`localhost`, `127.0.0.0/8`, or `::1`).
  Remote plaintext Tally hosts are intentionally rejected.
- AXAL credentials are sent only to the exact `https://complyeaze.com` origin
  by default. Self-hosted deployments must set
  `BRIDGE_AXAL_ALLOWED_ORIGINS` before Bridge starts to a comma-separated list
  of exact HTTPS origins such as `https://bridge.example`. Entries cannot
  contain paths, credentials, queries, or fragments.
- Documents must be selected with Bridge's native file or folder picker. Scan
  IDs are short-lived and native-only paths are never returned to the webview.
  Presigned uploads are limited to `https://complyeaze.com` by default; set
  `BRIDGE_DOCUMENT_UPLOAD_ALLOWED_ORIGINS` to the exact comma-separated HTTPS
  storage origins used by your AXAL deployment.

These environment variables are process configuration, not checkout paths;
the same policy applies on Windows and macOS. Restart Bridge after changing
them.

## Privacy and safe diagnostics

Do not commit or attach real customer, company, tax, certificate, credential,
financial, or document data. Before sharing logs, screenshots, fixtures, or
reproduction steps, replace personal and customer data with synthetic values
and remove local usernames and absolute paths. See [SECURITY.md](./SECURITY.md)
for private reporting and handling requirements.

## Repository map

- `src/` - React UI and API bindings
- `src-tauri/` - Rust core and Tauri configuration
- `docs/` - architecture, roadmap, and operational guidance
- `.github/` - issue and pull-request templates plus CI configuration

## Governance

- [Agent responsibilities](./AGENTS.md)
- [Contributor guide](./CONTRIBUTING.md)
- [Review checklist](./review-checklist.md)
- [Security policy](./SECURITY.md)
- [Rectification guidelines](./docs/rectify-guidelines.md)
- [Roadmap](./docs/step-by-step-roadmap.md)
- [Managed Git guidance](./docs/bootstrap/managed-git.md)
- [Source and asset provenance](./docs/provenance.md)
- [Release process](./docs/release-process.md)

## License

Bridge is licensed under the [Apache License, Version 2.0](./LICENSE).
Attribution notices are provided in [NOTICE](./NOTICE).
The historical `v0.1.0` release remains under the MIT license shipped with
that tag; current development source is version `0.2.0` under Apache-2.0.
