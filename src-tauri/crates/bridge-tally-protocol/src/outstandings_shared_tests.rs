use super::*;

#[test]
fn company_book_extent_template_has_only_the_verified_fetch_list() {
    let xml = render_company_book_extent("Synthetic & Company");
    assert!(xml.contains("<ID>BridgeCompanyBookExtentV1</ID>"));
    assert!(xml.contains("ALTVCHID"));
    assert!(xml.contains("ALTMSTID"));
    assert!(xml.contains("Synthetic &amp; Company"));
    assert!(!xml.contains("CompanyNumber"));
    assert!(!xml.contains("<COMPUTE>"));
    assert!(!xml.contains("$$NumItems"));

    let v2 = render_company_book_extent_v2("Synthetic & Company");
    assert!(v2.contains("<ID>BridgeCompanyBookExtentV2</ID>"));
    assert!(v2.contains(
        "<FETCH>Name, GUID, CompanyNumber, BooksFrom, LastVoucherDate, ALTVCHID, ALTMSTID</FETCH>"
    ));
    assert!(v2.contains("ISMODIFY=\"No\""));
}

fn synthetic_extent(
    master_alter_id_high_water: Option<MasterAlterIdHighWater>,
) -> CompanyBookExtent {
    let company = PinnedCompany::verified(
        ValidatedCompanyName::new("Synthetic Company".to_string())
            .expect("synthetic name validates"),
        "synthetic-guid".to_string(),
    )
    .expect("synthetic identity verifies");
    CompanyBookExtent::new(
        company,
        TallyDate::parse("20240101".to_string()).expect("synthetic BooksFrom"),
        TallyDate::parse("20260101".to_string()).expect("synthetic LastVoucherDate"),
        Some(VoucherAlterIdHighWater::parse("1").expect("synthetic ALTVCHID")),
        master_alter_id_high_water,
    )
}

/// The production bracket's strict check, isolated from any particular
/// caller: an extent whose `ALTMSTID` witness is absent must be refused
/// with the typed `MasterWitnessAbsent` error, while a witness-bearing
/// extent passes through unchanged. `connector.rs` and `connection.rs`
/// each call this at the point where their own bracket forms/compares a
/// production extent -- see their `..._fails_closed_when_altmstid_is_absent`
/// tests for that end-to-end wiring.
#[test]
fn require_master_witness_fails_closed_only_when_the_witness_is_absent() {
    assert_eq!(
        require_master_witness(&synthetic_extent(None)),
        Err(OutstandingsError::MasterWitnessAbsent)
    );
    assert_eq!(
        require_master_witness(&synthetic_extent(Some(
            MasterAlterIdHighWater::parse("1").expect("synthetic ALTMSTID")
        ))),
        Ok(())
    );
}
