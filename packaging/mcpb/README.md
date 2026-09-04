# MCPB assembly

Build the host `bridge_mcp` binary before assembling an MCPB archive:

```sh
node scripts/package-mcpb.mjs
```

The command copies the host binary into `packaging/mcpb/bin/`, matching the entry points in `manifest.json`. That directory is a build output and must not be committed. Run the script independently on each supported host before creating its archive.
