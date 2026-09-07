import { releaseAssets, releaseLabel, repository, selectRelease } from "./release-catalog.mjs";

const status = document.querySelector("#release-status");
const channelNote = document.querySelector("#channel-note");
const port = document.querySelector("#tally-port");
const portGuidance = document.querySelector("#port-guidance");
let releases = [];

function setDownload(option, asset) {
  if (!asset) {
    option.removeAttribute("href");
    option.setAttribute("aria-disabled", "true");
    option.querySelector("small").textContent = "No matching release asset is published yet.";
    return;
  }
  option.href = asset.bundle.browser_download_url;
  option.removeAttribute("aria-disabled");
  option.querySelector("small").textContent = `Download ${asset.bundle.name}; verify ${asset.checksum.name}.`;
}

function renderRelease() {
  const release = selectRelease(releases);
  if (!release) {
    status.textContent = "No installable unsigned preview is published.";
    channelNote.textContent = "See all releases for release notes and availability.";
    document.querySelectorAll(".download-option").forEach((option) => setDownload(option));
    return;
  }
  status.textContent = releaseLabel(release);
  channelNote.textContent = "Unsigned previews are for evaluation. Review the checksum and release notes before opening one.";
  document.querySelectorAll(".download-option").forEach((option) => {
    setDownload(option, releaseAssets(release, option.dataset.platform));
  });
}

async function loadReleases() {
  try {
    const response = await fetch(`https://api.github.com/repos/${repository}/releases?per_page=100`, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!response.ok) throw new Error(`release_lookup_failed:${response.status}`);
    releases = await response.json();
    renderRelease();
  } catch {
    status.textContent = "Release details could not be loaded. Use All releases to choose a download.";
    channelNote.textContent = "The download list is unavailable until the GitHub release service responds.";
  }
}

port.addEventListener("input", () => {
  const value = Number(port.value);
  const valid = Number.isInteger(value) && value >= 1 && value <= 65535;
  port.setAttribute("aria-invalid", String(!valid));
  portGuidance.innerHTML = valid
    ? `Use <strong>${value}</strong> in Bridge's Tally port setting. This does not change Tally's own gateway configuration.`
    : "Enter the local Tally HTTP gateway port, from 1 to 65535.";
});

loadReleases();
