use super::*;

#[test]
fn fixed_definition_has_only_the_verified_outstandings_wildcard() {
    let xml = render_outstandings_template("Synthetic & Company", "20260401", "20260402", 400, 800);
    assert!(!xml.contains("$$NumItems"));
    assert!(!xml.contains("<COMPUTE>"));
    assert!(!xml.contains("ALLLEDGERENTRIES.BILLALLOCATIONS"));
    assert!(!xml.contains("BILLALLOCATIONS.BILLTYPE"));
    assert_eq!(xml.matches("ALLLEDGERENTRIES.*").count(), 1);
    assert!(xml.contains("Synthetic &amp; Company"));
    assert!(xml.contains("$AlterID &gt; 400 AND $AlterID &lt;= 800"));
}

#[test]
fn empty_partition_witness_is_date_only_and_uses_no_wildcard_exception_shape() {
    let xml =
        render_empty_partition_witness_template("Synthetic & Company", "20260401", "20260402");
    assert!(xml.contains("<ID>BridgeVoucherEmptyPartitionWitnessV1</ID>"));
    assert!(xml.contains("<FETCH>GUID, ALTERID, DATE</FETCH>"));
    assert!(xml.contains("$Date &gt;= ##SVFromDate AND $Date &lt;= ##SVToDate"));
    assert!(xml.contains("Synthetic &amp; Company"));
    assert!(!xml.contains("ALLLEDGERENTRIES.*"));
    assert!(!xml.contains("$AlterID"));
    assert!(!xml.contains("<COMPUTE>"));
    assert!(!xml.contains("$$NumItems"));
}
