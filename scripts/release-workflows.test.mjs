import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { parseDocument } from "yaml";

const defaultBranchGuard = "github.ref == format('refs/heads/{0}', github.event.repository.default_branch)";

async function workflow(path) {
  const source = await readFile(new URL(path, import.meta.url), "utf8");
  const document = parseDocument(source);
  assert.equal(document.errors.length, 0, `${path} must be valid YAML`);
  return document.toJS();
}

function assertWorkflowDispatch(workflow, label) {
  assert.ok(Object.hasOwn(workflow, "on"), `${label} must declare a trigger`);
  assert.ok(Object.hasOwn(workflow.on, "workflow_dispatch"), `${label} must be manually dispatched`);
  assert.equal(Object.hasOwn(workflow.on, "push"), false, `${label} must not publish on push`);
}

function assertEnabled(node, label) {
  assert.notEqual(node.if, false, `${label} must not be disabled`);
  assert.notEqual(node.if, "false", `${label} must not be disabled`);
  assert.notEqual(node.if, "${{ false }}", `${label} must not be disabled`);
}

function step(job, name) {
  const found = job.steps.find((candidate) => candidate.name === name);
  assert.ok(found, `missing ${name} step`);
  assertEnabled(found, name);
  return found;
}

function assertReleaseWorkflow(release) {
  assertWorkflowDispatch(release, "preview release workflow");
  const admission = release.jobs["release-admission"];
  const packageJob = release.jobs.package;
  const publish = release.jobs["publish-preview"];
  assert.ok(admission && packageJob && publish, "release jobs must be present");
  assertEnabled(admission, "release admission job");
  assertEnabled(packageJob, "package job");
  assertEnabled(publish, "publish job");
  assert.equal(packageJob.needs, "release-admission", "package must wait for release admission");
  assert.equal(publish.needs, "package", "publication must wait for both package archives");
  assert.equal(publish.permissions.contents, "write", "only publication receives contents write permission");

  const admissionStep = step(admission, "Require reviewed source and a matching immutable preview version");
  assert.equal(admissionStep.env.SOURCE_REF, "${{ github.ref }}");
  assert.equal(admissionStep.env.DEFAULT_BRANCH, "${{ github.event.repository.default_branch }}");
  assert.match(admissionStep.run, /node scripts\/check-mcpb-preview-admission\.mjs/);

  const windowsSetup = step(packageJob, "Set up Windows native prerequisites");
  assert.equal(windowsSetup.if, "runner.os == 'Windows'");
  assert.equal(windowsSetup.uses, "./.github/actions/setup-windows-native");
  const cargoBuild = step(packageJob, "Build the host Bridge MCP binary");
  assert.match(cargoBuild.run, /cargo build --locked --release --manifest-path src-tauri\/Cargo\.toml --bin bridge_mcp/);
  assert.ok(
    packageJob.steps.indexOf(windowsSetup) < packageJob.steps.indexOf(cargoBuild),
    "Windows prerequisites must run before Cargo",
  );

  const releaseStep = step(publish, "Create the immutable GitHub preview release");
  assert.equal(releaseStep.env.GH_REPO, "${{ github.repository }}");
  assert.match(releaseStep.run, /gh release create/);
  assert.match(releaseStep.run, /--verify-tag/);
  assert.doesNotMatch(releaseStep.run, /--target/);
  assert.match(releaseStep.run, /--prerelease/);
}

function assertInstallPageWorkflow(page) {
  assertWorkflowDispatch(page, "install page workflow");
  const deploy = page.jobs.deploy;
  assert.ok(deploy, "install page deployment job must be present");
  assert.equal(deploy.if, defaultBranchGuard, "Pages deployment must be gated to the default branch");
  const upload = deploy.steps.find((candidate) => candidate.uses?.startsWith("actions/upload-pages-artifact@"));
  const publish = deploy.steps.find((candidate) => candidate.uses?.startsWith("actions/deploy-pages@"));
  assert.ok(upload && publish, "Pages deployment must upload and deploy the site artifact");
  assertEnabled(upload, "Pages upload step");
  assertEnabled(publish, "Pages deployment step");
  assert.equal(upload.with.path, "site");
}

test("publication workflows enforce their parsed trigger, dependency, branch, and platform controls", async () => {
  assertReleaseWorkflow(await workflow("../.github/workflows/release-mcpb-preview.yml"));
  assertInstallPageWorkflow(await workflow("../.github/workflows/deploy-install-page.yml"));
});

test("publication workflow checks reject disabled or misplaced controls", async () => {
  const release = await workflow("../.github/workflows/release-mcpb-preview.yml");
  const disabledAdmission = structuredClone(release);
  disabledAdmission.jobs["release-admission"].steps.find(
    (candidate) => candidate.name === "Require reviewed source and a matching immutable preview version",
  ).if = false;
  assert.throws(() => assertReleaseWorkflow(disabledAdmission), /must not be disabled/);

  const misplacedAdmission = structuredClone(release);
  const admissionSteps = misplacedAdmission.jobs["release-admission"].steps;
  const [admissionStep] = admissionSteps.splice(admissionSteps.findIndex(
    (candidate) => candidate.name === "Require reviewed source and a matching immutable preview version",
  ), 1);
  misplacedAdmission.jobs.package.steps.push(admissionStep);
  assert.throws(() => assertReleaseWorkflow(misplacedAdmission), /missing Require reviewed source/);

  const page = await workflow("../.github/workflows/deploy-install-page.yml");
  const disabledPage = structuredClone(page);
  disabledPage.jobs.deploy.if = false;
  assert.throws(() => assertInstallPageWorkflow(disabledPage), /default branch/);
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
