# Install Bridge for Claude Desktop

Use the Bridge install page when it is deployed. It chooses the current GitHub
Release asset for your operating system and gives the same setup steps without
developer configuration. This guide is the fallback when that page is
unavailable.

## Download the right package

Download a `.mcpb` from the project's [GitHub Releases](https://github.com/lamemustafa/bridge/releases).
Preview packages are available for Windows x64 and Apple Silicon Mac (ARM64).
Intel Mac and other platforms are not qualified. Package availability is not a
host-validation claim; read each release's notes for its current runtime gaps.

An **unsigned preview** is labelled as a prerelease and is only for evaluation;
it is not signed or notarized. Each archive has a same-named `.sha256` file and
a small provenance record on its release so an organization can identify the
downloaded bytes and source commit.

## Install and configure

1. Open the `.mcpb` file. If it does not open Claude Desktop, use **Settings →
   Extensions → Advanced settings → Install Extension…** and choose the file.
2. Keep **Tally host** as `localhost`. Bridge accepts only a local loopback
   endpoint. On a Mac, Tally must already be available there through a local
   Windows VM or organization-approved local forwarding. A separate PC or a
   LAN-only Tally cannot be reached by entering its network address.
3. Set **Tally port** to Tally's local HTTP gateway port. It defaults to `9000`.
   This is not a Tally licence port. Changing it changes only where Bridge calls
   Tally, not Tally's own HTTP setting.
4. Save the extension settings and restart Claude Desktop if its tools are not
   visible. In a new chat, use **Connectors** to confirm Bridge is connected.

Journal preparation and posting are available by default. Every new posting
requires your approval in a separate Bridge dialog. Turn off **Allow Journal posting**
in the extension settings for a read-only connector.

Native posting currently accepts one Journal with existing ledgers and no supplied
voucher number. Tally assigns the number. Bridge uses a private request identity
for the native attempt; the selected XML file stays unchanged. Do not manually
import a file and then post it through Bridge: if the original Journal was edited,
Bridge may be unable to recognize that earlier business event.

While posting, pause other imports and ledger changes in the selected company
and leave Tally's product/licence mode unchanged. Bridge's checks do not lock
out changes made directly in Tally or by other software.

Stop Bridge and every client running its connector before upgrading, then restart
them with the updated version. On Windows, dispatch coordination now uses the
operating system's local app-data folder even when launcher environment variables
are absent or overridden; older processes may use a different coordination path.
Keep the recovery data when upgrading. New posting attempts add a native request
commitment to the journal; older connector builds cannot read that new record.
Use this version or a newer compatible build to reconcile it rather than removing
the journal to downgrade. Older receipts may lack evidence that every result
counter was actually reported. After upgrading, Bridge keeps those receipts but
cannot confirm a clean response from them, even when the Journal matches in
Tally. Preserve the original history for investigation; do not repost the Journal
to replace its receipt.

Bridge only accepts loopback Tally endpoints. Do not open a port to the
internet or use a remote host to make this work.

## Data and updates

Bridge runs locally. When Claude uses a Bridge tool, the selected Tally result
is sent to the AI provider used for that conversation, so the conversation is
not wholly local. Choose the package's redaction setting when it suits the
workflow.

Private MCPB downloads do not update automatically. Install a newer release
from Claude Desktop's Extensions settings to upgrade, then confirm its version
and settings. Use the same screen to uninstall. Neither action changes Tally's
HTTP gateway configuration.
