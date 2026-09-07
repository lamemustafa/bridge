export const repository = "lamemustafa/bridge";
const platforms = ["windows-x64", "macos-arm64"];

export function isInstallablePreview(release) {
  return !release.draft
    && release.prerelease
    && /^mcp-preview-[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z]+)*$/.test(release.tag_name)
    && platforms.every((platform) => releaseAssets(release, platform));
}

export function selectRelease(releases) {
  return releases.find(isInstallablePreview);
}

export function assetName(tag, platform) {
  return `bridge-tally-${tag}-${platform}.mcpb`;
}

export function releaseAssets(release, platform) {
  const name = assetName(release.tag_name, platform);
  const bundle = release.assets.find((asset) => asset.name === name);
  const checksum = release.assets.find((asset) => asset.name === `${name}.sha256`);
  if (!bundle || !checksum) return undefined;
  return { bundle, checksum };
}

export function releaseLabel(release) {
  return `${release.tag_name} — unsigned preview`;
}
