# Install Bridge for Claude Desktop

Use the Bridge install page when it is deployed. It chooses the current GitHub
Release asset for your operating system and gives the same setup steps without
developer configuration. This guide is the fallback when that page is
unavailable.

## Download the right package

Download a `.mcpb` from the project's [GitHub Releases](https://github.com/lamemustafa/bridge/releases).
Choose Windows x64 or Apple Silicon Mac (ARM64) for the computer where both
Tally and Claude Desktop run. Intel Mac and other platforms are not qualified.

Use a production download only when its release notes identify the signed,
host-validated delivery path. An **unsigned preview** is labelled as a
prerelease and is only for evaluation; it is not signed or notarized. Each
archive has a same-named `.sha256` file and a small provenance record on its
release so an organization can identify the downloaded bytes and source commit.

## Install and configure

1. Open the `.mcpb` file. If it does not open Claude Desktop, use **Settings →
   Extensions → Advanced settings → Install Extension…** and choose the file.
2. Keep **Tally host** as `localhost`.
3. Set **Tally port** to Tally's local HTTP gateway port. It defaults to `9000`.
   This is not a Tally licence port. Changing it changes only where Bridge calls
   Tally, not Tally's own HTTP setting.
4. Save the extension settings and restart Claude Desktop if its tools are not
   visible. In a new chat, use **Connectors** to confirm Bridge is connected.

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
