import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("MCPB release workflow publishes only an explicit unsigned preview with both qualified host archives", async () => {
  const workflow = await readFile(new URL("../.github/workflows/release-mcpb-preview.yml", import.meta.url), "utf8");
  assert.match(workflow, /workflow_dispatch:/);
  assert.doesNotMatch(workflow, /\n  push:/);
  assert.doesNotMatch(workflow, /production-signed/);
  assert.match(workflow, /check-mcpb-preview-admission\.mjs/);
  assert.match(workflow, /SOURCE_REF: \$\{\{ github\.ref \}\}/);
  assert.match(workflow, /DEFAULT_BRANCH: \$\{\{ github\.event\.repository\.default_branch \}\}/);
  assert.match(workflow, /windows-x64/);
  assert.match(workflow, /macos-arm64/);
  assert.match(
    workflow,
    /name: Set up Windows native prerequisites\s+id: windows-mcpb-prerequisites\s+if: runner\.os == 'Windows'\s+uses: \.\/\.github\/actions\/setup-windows-native/,
  );
  assert.match(workflow, /refusing to replace existing release assets/);
  assert.match(workflow, /git ls-remote --tags origin "refs\/tags\/\$RELEASE_TAG" "refs\/tags\/\$RELEASE_TAG\^\{\}"/);
  assert.match(workflow, /\$\{peeled_sha:-\$tag_sha\}/);
  assert.match(workflow, /could not verify whether \$RELEASE_TAG already exists; refusing to publish/);
  assert.match(workflow, /existing tag \$RELEASE_TAG does not identify \$SOURCE_SHA/);
  assert.match(workflow, /gh api --method POST "repos\/\$GH_REPO\/git\/refs"/);
  assert.match(workflow, /--verify-tag/);
  assert.doesNotMatch(workflow, /--target "\$SOURCE_SHA"/);
  assert.match(workflow, /scripts\/check-mcpb-bundle\.py/);
  assert.match(workflow, /--prerelease/);
  assert.match(workflow, /GH_REPO: \$\{\{ github\.repository \}\}/);
  assert.match(workflow, /packaging\/mcpb\/UNSIGNED_PREVIEW_RELEASE\.md/);
});

test("the MCPB smoke binds initialize serverInfo.version to the archived manifest", async () => {
  const smoke = await readFile(new URL("./check-mcpb-bundle.py", import.meta.url), "utf8");
  assert.match(smoke, /server_version_mismatch/);
  assert.match(smoke, /validate_server_version\(replies\[0\], manifest\)/);
});

test("unsigned preview notes state the host-validation scope and remaining gaps", async () => {
  const notes = await readFile(new URL("../packaging/mcpb/UNSIGNED_PREVIEW_RELEASE.md", import.meta.url), "utf8");
  assert.match(notes, /hosted Windows x64 and Apple Silicon Mac runners/);
  assert.match(notes, /does not establish\nlive Tally behaviour or Claude Desktop conversational tool calls/);
  assert.match(notes, /Native Windows Tally\/Claude Desktop validation remains outstanding/);
  assert.match(notes, /Intel Mac is not qualified/);
});

test("install page deployment remains a reviewed manual action", async () => {
  const workflow = await readFile(new URL("../.github/workflows/deploy-install-page.yml", import.meta.url), "utf8");
  assert.match(workflow, /workflow_dispatch:/);
  assert.doesNotMatch(workflow, /\n  push:/);
  assert.match(
    workflow,
    /if: github\.ref == format\('refs\/heads\/\{0\}', github\.event\.repository\.default_branch\)/,
  );
  assert.match(workflow, /path: site/);
  assert.match(workflow, /actions\/deploy-pages@/);
});
