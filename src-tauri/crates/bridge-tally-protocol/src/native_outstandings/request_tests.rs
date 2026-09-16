use super::*;

#[test]
fn renders_the_verified_working_bills_request_shape() {
    let from = TallyDate::parse("20240401").unwrap();
    let to = TallyDate::parse("20260731").unwrap();
    let xml = render_native_bills_request(
        NativeBillsReportKind::Receivable,
        "Bridge Billwise Lab",
        &from,
        &to,
    );
    assert_eq!(
        xml,
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Data</TYPE><ID>Bills Receivable</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>Bridge Billwise Lab</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">20240401</SVFROMDATE><SVTODATE TYPE="Date">20260731</SVTODATE></STATICVARIABLES></DESC></BODY></ENVELOPE>"#
    );
    let payable = render_native_bills_request(
        NativeBillsReportKind::Payable,
        "Bridge Billwise Lab",
        &from,
        &to,
    );
    assert!(payable.contains("<ID>Bills Payable</ID>"));
}

#[test]
fn master_export_period_validates_only_the_opening_balance_boundary() {
    let legal_books_from = TallyDate::parse("20240101").unwrap();
    let ordinary_last_voucher_date = TallyDate::parse("20240115").unwrap();
    assert_eq!(
        NativeLedgerExportPeriod::new(
            DateBoundaryProfile::EducationRestricted,
            legal_books_from,
            ordinary_last_voucher_date,
        ),
        Ok(NativeLedgerExportPeriod {
            from: TallyDate::parse("20240101").unwrap(),
            to: TallyDate::parse("20240115").unwrap(),
        }),
        "Education mode accepts an ordinary LASTVOUCHERDATE because this export fetches OPENINGBALANCE, not CLOSINGBALANCE"
    );

    let illegal_books_from = TallyDate::parse("20240115").unwrap();
    let legal_book_end = TallyDate::parse("20240131").unwrap();
    assert_eq!(
        NativeLedgerExportPeriod::new(
            DateBoundaryProfile::EducationRestricted,
            illegal_books_from.clone(),
            legal_book_end,
        ),
        Err(NativeLedgerExportPeriodError::UnsupportedBoundary),
        "the observed Education profile must reject an unsupported BOOKSFROM before a silently ignored opening-balance read"
    );
    assert_eq!(
        NativeLedgerExportPeriod::new(
            DateBoundaryProfile::EducationRestricted,
            TallyDate::parse("20240201").unwrap(),
            TallyDate::parse("20240115").unwrap(),
        ),
        Err(NativeLedgerExportPeriodError::InvalidRange),
        "the range remains invalid even when its opening boundary is profile-supported"
    );
    assert!(
        NativeLedgerExportPeriod::new(
            DateBoundaryProfile::ModeAgnostic,
            illegal_books_from,
            TallyDate::parse("20260701").unwrap(),
        )
        .is_ok(),
        "licensed and unknown modes retain arbitrary calendar boundaries"
    );
}

#[test]
fn ledger_snapshot_period_validates_both_closing_balance_boundaries() {
    let legal_from = TallyDate::parse("20240101").unwrap();
    let ordinary_to = TallyDate::parse("20240115").unwrap();
    assert_eq!(
        NativeLedgerSnapshotPeriod::new(
            DateBoundaryProfile::EducationRestricted,
            legal_from.clone(),
            ordinary_to,
        ),
        Err(NativeLedgerSnapshotPeriodError::UnsupportedBoundary),
        "a snapshot CLOSINGBALANCE must not be requested with an Education-refused as-of boundary"
    );
    assert_eq!(
        NativeLedgerSnapshotPeriod::new(
            DateBoundaryProfile::EducationRestricted,
            TallyDate::parse("20240115").unwrap(),
            TallyDate::parse("20240131").unwrap(),
        ),
        Err(NativeLedgerSnapshotPeriodError::UnsupportedBoundary),
        "the opening boundary remains independently required"
    );
    assert_eq!(
        NativeLedgerSnapshotPeriod::new(
            DateBoundaryProfile::EducationRestricted,
            TallyDate::parse("20240201").unwrap(),
            TallyDate::parse("20240131").unwrap(),
        ),
        Err(NativeLedgerSnapshotPeriodError::InvalidRange),
        "inverted snapshot ranges remain invalid"
    );
    assert!(
        NativeLedgerSnapshotPeriod::new(
            DateBoundaryProfile::ModeAgnostic,
            legal_from,
            TallyDate::parse("20240115").unwrap(),
        )
        .is_ok(),
        "licensed and unknown modes retain arbitrary calendar boundaries"
    );
}

#[test]
fn escapes_company_names_in_both_requests() {
    let from = TallyDate::parse("20240401").unwrap();
    let to = TallyDate::parse("20260731").unwrap();
    let xml =
        render_native_bills_request(NativeBillsReportKind::Receivable, "A & B <Co>", &from, &to);
    assert!(xml.contains("A &amp; B &lt;Co&gt;"));
    assert!(!xml.contains("A & B <Co>"));

    let snapshot_period = NativeLedgerSnapshotPeriod::new(
        DateBoundaryProfile::ModeAgnostic,
        from.clone(),
        to.clone(),
    )
    .expect("mode-agnostic profile accepts valid calendar dates");
    let ledger_xml = render_native_ledger_snapshot_request("A & B <Co>", &snapshot_period);
    assert!(ledger_xml.contains("A &amp; B &lt;Co&gt;"));
    assert!(ledger_xml
        .contains("<FETCH>NAME, PARENT, CLOSINGBALANCE, OPENINGBALANCE, ISBILLWISEON</FETCH>"));
    assert!(ledger_xml
        .contains("<COMPUTE>BRIDGECOMPANYGUID:$GUID:Company:##SVCurrentCompany</COMPUTE>"));
    assert!(ledger_xml.contains(r#"<SVFROMDATE TYPE="Date">20240401</SVFROMDATE>"#));
    assert!(ledger_xml.contains(r#"<SVTODATE TYPE="Date">20260731</SVTODATE>"#));

    let group_xml = render_native_group_snapshot_request("A & B <Co>");
    assert!(group_xml.contains("A &amp; B &lt;Co&gt;"));
    assert!(group_xml.contains(r#"<ID>List of Groups</ID>"#));
    assert!(
        group_xml.contains(r#"<FETCH>NAME, PARENT, GUID, MASTERID, ALTERID, RESERVEDNAME</FETCH>"#)
    );
    assert!(
        group_xml.contains("<COMPUTE>BRIDGECOMPANYGUID:$GUID:Company:##SVCurrentCompany</COMPUTE>")
    );
    assert!(!group_xml.contains("<REPORT>"));
    assert!(!group_xml.contains("<FORM>"));
    assert!(!group_xml.contains("<PART>"));
    assert!(!group_xml.contains("<LINE>"));
    assert!(!group_xml.contains("<FIELD>"));
    assert!(!group_xml.contains("$$NumItems"));

    let export_period =
        NativeLedgerExportPeriod::new(DateBoundaryProfile::ModeAgnostic, from.clone(), to.clone())
            .expect("mode-agnostic profile accepts valid calendar dates");
    let export_xml = render_native_ledger_export_request("A & B <Co>", &export_period);
    assert!(export_xml.contains("A &amp; B &lt;Co&gt;"));
    assert!(!export_xml.contains("INCOMETAXNUMBER"));
    assert!(
        !export_xml.contains("BRIDGECOMPANYGUID"),
        "the ordinary ledger profile retains its existing response shape"
    );
    let party_master_xml = render_party_ledger_master_request("A & B <Co>", &export_period);
    assert!(party_master_xml.contains("INCOMETAXNUMBER, NAMEONPAN, LEDPINCODE, LEDGSTPINCODE, MSMEREGNUMBER, LEDUDYAMREGNUMBER, BANKACCHOLDERNAME, BANKDETAILS, IFSCODE, EMAIL, LEDGERPHONE, STATENAME, LEDADDRESS.LIST, TAXTYPE, GSTDUTYHEAD"));
    assert!(party_master_xml
        .contains("<COMPUTE>BRIDGECOMPANYGUID:$GUID:Company:##SVCurrentCompany</COMPUTE>"));
    assert!(!export_xml.contains("<REPORT>"));
    assert!(!export_xml.contains("<FORM>"));
    assert!(!export_xml.contains("<PART>"));
    assert!(!export_xml.contains("<LINE>"));
    assert!(!export_xml.contains("<FIELD>"));
    assert!(!export_xml.contains("$$NumItems"));
    assert!(export_xml.contains(r#"<SVFROMDATE TYPE="Date">20240401</SVFROMDATE>"#));
    assert!(export_xml.contains(r#"<SVTODATE TYPE="Date">20260731</SVTODATE>"#));

    let voucher_type_xml = render_native_voucher_type_export_request("A & B <Co>");
    assert!(voucher_type_xml.contains("A &amp; B &lt;Co&gt;"));
    assert!(voucher_type_xml.contains("<ID>List of VoucherTypes</ID>"));
    assert!(voucher_type_xml.contains("<FETCH>NAME, PARENT, GUID, MASTERID, ALTERID</FETCH>"));
    assert!(!voucher_type_xml.contains("<REPORT>"));
    assert!(!voucher_type_xml.contains("$$NumItems"));

    let voucher_from = TallyDate::parse("20260401").unwrap();
    let voucher_to = TallyDate::parse("20260930").unwrap();
    let voucher_xml =
        render_native_voucher_export_request("A & B <Co>", &voucher_from, &voucher_to);
    assert!(voucher_xml.contains("A &amp; B &lt;Co&gt;"));
    assert!(voucher_xml.contains("<TYPE>Collection</TYPE>"));
    assert!(voucher_xml.contains(
        "ALLLEDGERENTRIES.LEDGERNAME, ALLLEDGERENTRIES.AMOUNT, ALLLEDGERENTRIES.ISDEEMEDPOSITIVE"
    ));
    assert!(!voucher_xml.contains("<REPORT>"));
    assert!(!voucher_xml.contains("<FORM>"));
    assert!(!voucher_xml.contains("<FIELD>"));
    assert!(!voucher_xml.contains("$$NumItems"));
}

/// SECURITY: `from`/`to` are interpolated into the voucher window
/// predicate as *quoted* `$$Date:"..."` TDL arguments. `xml_escape` alone
/// is not sufficient there -- it turns `"` into `&quot;`, but Tally's XML
/// parser decodes `&quot;` back into a literal `"` before the formula
/// text is evaluated, so an escaped quote could still close the quoted
/// argument and inject arbitrary TDL. The fix is a closed input alphabet:
/// `render_native_voucher_export_request` only accepts already-validated
/// `TallyDate`s (exactly 8 ASCII digits), so this module asserts that
/// `TallyDate::parse` -- the only way to construct one, and therefore the
/// only way to reach the predicate -- rejects every value shaped like an
/// injection attempt, whitespace, wrong-length digit runs, and non-ASCII
/// digits. Each of these failed before the fix (the old signature
/// accepted `&str` and rendered whatever was given through `xml_escape`).
#[test]
fn voucher_window_bounds_reject_a_quote_breakout_injection_attempt() {
    // A value shaped to close the quoted $$Date:"..." argument and
    // splice in an alternative predicate clause.
    let injection = r#"20260930" OR $Date>=$$Date:"19000101"#;
    let result = TallyDate::parse(injection);
    assert!(
        result.is_err(),
        "an injection-shaped date must be REJECTED, not rendered: {result:?}"
    );
}

#[test]
fn voucher_window_bounds_reject_whitespace() {
    assert!(TallyDate::parse("2026 0401").is_err());
    assert!(TallyDate::parse("20260401 ").is_err());
    assert!(TallyDate::parse(" 20260401").is_err());
}

#[test]
fn voucher_window_bounds_reject_wrong_length_digit_runs() {
    assert!(TallyDate::parse("2026040").is_err()); // 7 digits
    assert!(TallyDate::parse("202604011").is_err()); // 9 digits
}

#[test]
fn voucher_window_bounds_reject_non_ascii_digits() {
    // U+FF10..U+FF19 are fullwidth digit code points -- not ASCII, and
    // must not be accepted as a stand-in for '0'..'9'.
    assert!(TallyDate::parse("２０２６０４０１").is_err());
}

/// A well-formed 8-digit date still renders exactly today's predicate
/// shape -- the fix rejects malformed input, it does not change accepted
/// behavior.
#[test]
fn voucher_window_bounds_accept_a_valid_date_and_render_the_existing_predicate() {
    let from = TallyDate::parse("20260401").unwrap();
    let to = TallyDate::parse("20260930").unwrap();
    let voucher_xml = render_native_voucher_export_request("Bridge Billwise Lab", &from, &to);
    assert!(voucher_xml.contains(
        r#"<SYSTEM TYPE="Formulae" NAME="BridgeVoucherWindowFilter">$Date &gt;= $$Date:"20260401" AND $Date &lt;= $$Date:"20260930"</SYSTEM>"#
    ));
}

/// TALLY_PROTOCOL_REFERENCE section 5.1/5.2/5.3: `SVFROMDATE`/`SVTODATE`
/// alone do not bound collection membership; a `<SYSTEM TYPE="Formulae">`
/// predicate referenced from `<FILTERS>` is the proven fix; and section
/// 5.3 established that the predicate must use literal `$$Date:"..."`
/// bounds rather than `##SVFromDate`/`##SVToDate`, because a refused
/// period boundary silently widens the period and a predicate depending
/// on `##SVToDate` then resolves to a response indistinguishable from a
/// genuinely empty window. This test pins the request-side filter onto
/// the native voucher export.
#[test]
fn native_voucher_export_request_is_bounded_by_a_date_filter() {
    let from = TallyDate::parse("20260401").unwrap();
    let to = TallyDate::parse("20260930").unwrap();
    let voucher_xml = render_native_voucher_export_request("Bridge Billwise Lab", &from, &to);

    // The SYSTEM Formulae predicate is present and compares $Date against
    // literal date bounds built from the function's own from/to
    // arguments, not against ##SVFromDate/##SVToDate.
    assert!(voucher_xml.contains(
        r#"<SYSTEM TYPE="Formulae" NAME="BridgeVoucherWindowFilter">$Date &gt;= $$Date:"20260401" AND $Date &lt;= $$Date:"20260930"</SYSTEM>"#
    ));
    assert!(voucher_xml.contains(r#"$$Date:"20260401""#));
    assert!(voucher_xml.contains(r#"$$Date:"20260930""#));
    assert!(!voucher_xml.contains("##SVToDate"));
    assert!(!voucher_xml.contains("##SVFromDate"));

    // The COLLECTION references the filter via <FILTERS>.
    assert!(voucher_xml.contains("<FILTERS>BridgeVoucherWindowFilter</FILTERS>"));

    // The filter name itself carries no whitespace anywhere it appears.
    assert!(!"BridgeVoucherWindowFilter".contains(' '));
    for occurrence in voucher_xml.match_indices("BridgeVoucherWindowFilter") {
        let (start, _) = occurrence;
        assert!(
            !voucher_xml[start..start + "BridgeVoucherWindowFilter".len()]
                .chars()
                .any(char::is_whitespace)
        );
    }

    // The FETCH list is exactly the pre-existing, load-bearing dotted
    // ALLLEDGERENTRIES field list -- unchanged by adding the filter.
    assert!(voucher_xml.contains(
        "<FETCH>DATE, GUID, MASTERID, ALTERID, VOUCHERTYPENAME, VOUCHERNUMBER, ISCANCELLED, ISOPTIONAL, ALLLEDGERENTRIES.LEDGERNAME, ALLLEDGERENTRIES.AMOUNT, ALLLEDGERENTRIES.ISDEEMEDPOSITIVE</FETCH>"
    ));

    // No <REPORT> element is introduced.
    assert!(!voucher_xml.contains("<REPORT>"));
    assert!(!voucher_xml.contains("<REPORT "));
}
