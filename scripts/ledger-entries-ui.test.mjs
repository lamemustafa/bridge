import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("ledger investigation remains an explicit, bounded Overview action", async () => {
  const [app, screen, styles] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/LedgerEntriesScreen.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ]);
  assert.match(app, /import \{ LedgerEntriesScreen \} from "\.\/LedgerEntriesScreen";/);
  assert.match(app, /type View = .*"ledger_entries"/);
  assert.match(app, /onClick=\{\(\) => setView\("ledger_entries"\)\}/);
  assert.match(app, /<LedgerEntriesScreen[\s\S]*?onReadActivity=\{changeChildTallyReadActivity\}/);
  assert.doesNotMatch(app.slice(app.indexOf('<nav aria-label="Bridge navigation">'), app.indexOf("</nav>")), /Investigate ledger/);
  assert.match(screen, /config\.host, config\.port, company\.name, company\.guid, company\.company_number, company\.books_from_yyyymmdd, company\.canonical_origin, ledger, from, to/);
  assert.match(screen, /return \(\) => \{ requestVersion\.current \+= 1; \};/);
  assert.match(screen, /submittedScope === latestScope\.current/);
  assert.match(screen, /fetch_selected_ledger_entries/);
  assert.match(screen, /onSubmit=\{\(event\) => \{[\s\S]*?investigate\(\)/);
  assert.match(screen, /complete requested window before filtering; page controls only limit what is displayed/);
  assert.match(screen, /Voucher entries and counterpart lines/);
  assert.match(screen, /voucher\.cancelled \|\| voucher\.optional/);
  assert.match(screen, /result\.offset \+ result\.items\.length < result\.total/);
  assert.match(styles, /\.ledger-investigation-form\s*\{[\s\S]*?grid-template-columns:/);
  assert.match(styles, /@media \(max-width: 760px\)[\s\S]*?\.ledger-investigation-form/);
});

test("desktop command invokes the one shared selected-voucher operation", async () => {
  const [agent, vouchers, commands] = await Promise.all([
    readFile(new URL("../src-tauri/src/agent.rs", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/agent_vouchers.rs", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/commands.rs", import.meta.url), "utf8"),
  ]);
  assert.match(vouchers, /pub\(crate\) async fn selected_voucher_operation/);
  assert.match(vouchers, /pub\(super\) async fn vouchers[\s\S]*?selected_voucher_operation\(self, args\)/);
  assert.match(agent, /pub\(crate\) async fn desktop_selected_vouchers[\s\S]*?vouchers::selected_voucher_operation_for_verified\([\s\S]*?&server/);
  assert.match(commands, /fetch_selected_ledger_entries[\s\S]*?desktop_selected_vouchers/);
  assert.match(vouchers, /read_ledger_catalogue[\s\S]*?render_agent_vouchers[\s\S]*?read_ledger_catalogue/);
  assert.match(vouchers, /filter_voucher_rows_for_ledger[\s\S]*?skip\(offset\)[\s\S]*?take\(limit\)/);
});
