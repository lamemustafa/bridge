# Derived voucher-import candidate

The paired `.xml` is **not a capture**. It is a structural derivative of a
real, user-authored Tally voucher-import file supplied by an operator, added in
`a33e1aa8` (#270). The envelope nesting, element set and order, attribute set
(including `ACTION`), per-voucher field presence, two ledger entries per
voucher, the balanced +/- pair and the value formats are transcribed from that
document; every value is generated, with dates moved outside the source's year
range. A structural diff against the original reported no difference, and a
leak check against both private source revisions reported none.

The full derivation and its limits are in `docs/tally/TEST_CORPUS.md` §8. In
short: this is format evidence, never acceptance evidence — the original
file's import outcome is unknown — and because it is a derivative it cannot
support any claim about exact bytes a real instance received. No byte count or
SHA-256 is declared for that reason.

Used by `accepts_the_structure_of_a_real_voucher_import_candidate` in
`src-tauri/src/source_draft_xml_tests.rs`.
