pub fn company_list_request() -> String {
    bridge_tally_protocol::xml_read_profiles::compatibility::company_list_request()
}

pub fn standard_ledger_identity_request(company: &str) -> String {
    bridge_tally_protocol::xml_read_profiles::compatibility::standard_ledger_identity_request(
        company,
    )
}

pub fn standard_ledger_catalog_request(company: &str) -> String {
    bridge_tally_protocol::xml_read_profiles::compatibility::standard_ledger_catalog_request(
        company,
    )
}

#[cfg(test)]
fn legacy_company_list_request() -> String {
    r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>Export</TALLYREQUEST>
        <TYPE>Data</TYPE>
        <ID>Company Report</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
            </STATICVARIABLES>
            <TDL>
                <TDLMESSAGE>
                    <REPORT NAME="Company Report">
                        <FORMS>Company Form</FORMS>
                        <TITLE>"Company Details"</TITLE>
                    </REPORT>
                    <FORM NAME="Company Form">
                        <TOPPARTS>Company Part</TOPPARTS>
                        <HEIGHT>100% Page</HEIGHT>
                        <WIDTH>100% Page</WIDTH>
                    </FORM>
                    <PART NAME="Company Part">
                        <TOPLINES>Company Header, Company Details</TOPLINES>
                        <REPEAT>Company Details : CompanyCollection</REPEAT>
                        <SCROLLED>Vertical</SCROLLED>
                        <COMMONBORDERS>Yes</COMMONBORDERS>
                    </PART>
                    <LINE NAME="Company Header">
                        <LEFTFIELDS>
                            Company Name Header, Company GUID Header
                        </LEFTFIELDS>
                    </LINE>
                    <FIELD NAME="Company Name Header"><SET>"Company Name"</SET></FIELD>
                    <FIELD NAME="Company GUID Header"><SET>"Company GUID"</SET></FIELD>
                    <LINE NAME="Company Details">
                        <LEFTFIELDS>
                            Company Name Field, Company GUID Field
                        </LEFTFIELDS>
                        <XMLTAG>"CompanyInfo"</XMLTAG>
                    </LINE>
                    <FIELD NAME="Company Name Field"><SET>$Name</SET></FIELD>
                    <FIELD NAME="Company GUID Field"><SET>$GUID</SET></FIELD>
                    <COLLECTION NAME="CompanyCollection">
                        <TYPE>Company</TYPE>
                        <FETCH>Name, GUID</FETCH>
                    </COLLECTION>
                </TDLMESSAGE>
            </TDL>
        </DESC>
    </BODY>
</ENVELOPE>
"#
    .trim()
    .to_string()
}

pub fn sales_vouchers_request(company: &str, from: &str, to: &str) -> String {
    format!(
        r#"
<ENVELOPE>
  <HEADER>
    <VERSION>1</VERSION>
    <TALLYREQUEST>EXPORT</TALLYREQUEST>
    <TYPE>COLLECTION</TYPE>
    <ID>Sales Vouchers</ID>
  </HEADER>
  <BODY>
    <DESC>
      <STATICVARIABLES>
        <SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY>
        <SVFROMDATE>{}</SVFROMDATE>
        <SVTODATE>{}</SVTODATE>
        <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
      </STATICVARIABLES>
      <TDL>
        <TDLMESSAGE>
          <COLLECTION NAME="Sales Vouchers">
            <TYPE>Voucher</TYPE>
            <FILTERS>SalesOnly</FILTERS>
            <FETCH>Date, VoucherTypeName, VoucherNumber, PartyLedgerName</FETCH>
          </COLLECTION>
          <SYSTEM TYPE="Formulae" NAME="SalesOnly">$$IsSales:$VoucherTypeName</SYSTEM>
        </TDLMESSAGE>
      </TDL>
    </DESC>
  </BODY>
</ENVELOPE>
"#,
        xml_escape(company),
        xml_escape(from),
        xml_escape(to)
    )
    .trim()
    .to_string()
}

pub fn ledgers_request(company: &str) -> String {
    bridge_tally_protocol::xml_read_profiles::compatibility::ledgers_request(company)
}

pub fn groups_request(company: &str) -> String {
    format!(
        r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>EXPORT</TALLYREQUEST>
        <TYPE>DATA</TYPE>
        <ID>BRIDGE Group Export V1</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
                <SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY>
            </STATICVARIABLES>
            <TDL>
                <TDLMESSAGE>
                    <REPORT NAME="BRIDGE Group Export V1">
                        <FORMS>BRIDGE Group Export Form V1</FORMS>
                        <PLAINXML>Yes</PLAINXML>
                    </REPORT>
                    <FORM NAME="BRIDGE Group Export Form V1">
                        <TOPPARTS>BRIDGE Group Context Part V1, BRIDGE Group Rows Part V1</TOPPARTS>
                    </FORM>
                    <PART NAME="BRIDGE Group Context Part V1">
                        <TOPLINES>BRIDGE Group Context Line V1</TOPLINES>
                    </PART>
                    <PART NAME="BRIDGE Group Rows Part V1">
                        <TOPLINES>BRIDGE Group Row Line V1</TOPLINES>
                        <REPEAT>BRIDGE Group Row Line V1 : BRIDGE Group Collection V1</REPEAT>
                    </PART>
                    <LINE NAME="BRIDGE Group Context Line V1">
                        <LEFTFIELDS>BRIDGE Group Schema V1, BRIDGE Group Object Type V1, BRIDGE Group Company Name V1, BRIDGE Group Company GUID V1, BRIDGE Group Record Count V1</LEFTFIELDS>
                        <XMLTAG>"COMPANYCONTEXT"</XMLTAG>
                    </LINE>
                    <FIELD NAME="BRIDGE Group Schema V1">
                        <SET>"bridge.tally.groups/1"</SET>
                        <XMLTAG>"SCHEMA"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Group Object Type V1">
                        <SET>"GROUP"</SET>
                        <XMLTAG>"OBJECTTYPE"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Group Company Name V1">
                        <SET>##SVCurrentCompany</SET>
                        <XMLTAG>"NAME"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Group Company GUID V1">
                        <SET>$GUID:Company:##SVCurrentCompany</SET>
                        <XMLTAG>"GUID"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Group Record Count V1">
                        <SET>$$NumItems:BRIDGE Group Collection V1</SET>
                        <XMLTAG>"RECORDCOUNT"</XMLTAG>
                    </FIELD>
                    <LINE NAME="BRIDGE Group Row Line V1">
                        <LEFTFIELDS>BRIDGE Group Parent V1</LEFTFIELDS>
                        <XMLTAG>"GROUP"</XMLTAG>
                        <XMLATTR>"NAME" : $Name</XMLATTR>
                        <XMLATTR>"GUID" : $GUID</XMLATTR>
                        <XMLATTR>"REMOTEID" : $RemoteID</XMLATTR>
                        <XMLATTR>"MASTERID" : $MasterID</XMLATTR>
                        <XMLATTR>"ALTERID" : $AlterID</XMLATTR>
                    </LINE>
                    <FIELD NAME="BRIDGE Group Parent V1">
                        <SET>$Parent</SET>
                        <XMLTAG>"PARENT"</XMLTAG>
                    </FIELD>
                    <COLLECTION NAME="BRIDGE Group Collection V1" ISMODIFY="No" ISFIXED="No" ISINITIALIZE="No" ISOPTION="No" ISINTERNAL="No">
                        <TYPE>Group</TYPE>
                        <FETCH>Name, GUID, RemoteID, MasterID, AlterID, Parent</FETCH>
                    </COLLECTION>
                </TDLMESSAGE>
            </TDL>
        </DESC>
    </BODY>
</ENVELOPE>
"#,
        xml_escape(company)
    )
    .trim()
    .to_string()
}

pub fn selected_vouchers_request(company: &str, from: &str, to: &str) -> String {
    bridge_tally_protocol::xml_read_profiles::compatibility::selected_vouchers_request(
        company, from, to,
    )
}

/// Experimental Bridge-defined ledger-balance cross-view. The request emits no
/// ledger names: rows are joined to the canonical mirror by candidate native
/// identifiers. Exact semantics and applicability remain capability-gated.
pub fn ledger_period_balances_request(company: &str, from: &str, to: &str) -> String {
    format!(
        r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>EXPORT</TALLYREQUEST>
        <TYPE>DATA</TYPE>
        <ID>BRIDGE Ledger Period Balances V1</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
                <SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY>
                <SVFROMDATE TYPE="Date">{}</SVFROMDATE>
                <SVTODATE TYPE="Date">{}</SVTODATE>
            </STATICVARIABLES>
            <TDL>
                <TDLMESSAGE>
                    <REPORT NAME="BRIDGE Ledger Period Balances V1">
                        <FORMS>BRIDGE Ledger Period Balances Form V1</FORMS>
                        <PLAINXML>Yes</PLAINXML>
                    </REPORT>
                    <FORM NAME="BRIDGE Ledger Period Balances Form V1">
                        <TOPPARTS>BRIDGE Ledger Period Context Part V1, BRIDGE Ledger Period Rows Part V1</TOPPARTS>
                    </FORM>
                    <PART NAME="BRIDGE Ledger Period Context Part V1">
                        <TOPLINES>BRIDGE Ledger Period Context Line V1</TOPLINES>
                    </PART>
                    <PART NAME="BRIDGE Ledger Period Rows Part V1">
                        <TOPLINES>BRIDGE Ledger Period Row Line V1</TOPLINES>
                        <REPEAT>BRIDGE Ledger Period Row Line V1 : BRIDGE Ledger Period Collection V1</REPEAT>
                    </PART>
                    <LINE NAME="BRIDGE Ledger Period Context Line V1">
                        <LEFTFIELDS>BRIDGE Ledger Period Schema V1, BRIDGE Ledger Period Object V1, BRIDGE Ledger Period Company GUID V1, BRIDGE Ledger Period From V1, BRIDGE Ledger Period To V1, BRIDGE Ledger Period Ordinary Books Requested V1, BRIDGE Ledger Period Count V1</LEFTFIELDS>
                        <XMLTAG>"COMPANYCONTEXT"</XMLTAG>
                    </LINE>
                    <FIELD NAME="BRIDGE Ledger Period Schema V1"><SET>"bridge.tally.ledger-period-balances/1"</SET><XMLTAG>"SCHEMA"</XMLTAG></FIELD>
                    <FIELD NAME="BRIDGE Ledger Period Object V1"><SET>"LEDGERPERIODBALANCE"</SET><XMLTAG>"OBJECTTYPE"</XMLTAG></FIELD>
                    <FIELD NAME="BRIDGE Ledger Period Company GUID V1"><SET>$GUID:Company:##SVCurrentCompany</SET><XMLTAG>"GUID"</XMLTAG></FIELD>
                    <FIELD NAME="BRIDGE Ledger Period From V1"><SET>"{}"</SET><XMLTAG>"FROMDATE"</XMLTAG></FIELD>
                    <FIELD NAME="BRIDGE Ledger Period To V1"><SET>"{}"</SET><XMLTAG>"TODATE"</XMLTAG></FIELD>
                    <FIELD NAME="BRIDGE Ledger Period Ordinary Books Requested V1"><SET>Yes</SET><TYPE>Logical</TYPE><XMLTAG>"ORDINARYBOOKSREQUESTED"</XMLTAG></FIELD>
                    <FIELD NAME="BRIDGE Ledger Period Count V1"><SET>$$NumItems:BRIDGE Ledger Period Collection V1</SET><XMLTAG>"RECORDCOUNT"</XMLTAG></FIELD>
                    <LINE NAME="BRIDGE Ledger Period Row Line V1">
                        <LEFTFIELDS>BRIDGE Ledger Period Opening V1, BRIDGE Ledger Period Closing V1</LEFTFIELDS>
                        <XMLTAG>"LEDGERPERIODBALANCE"</XMLTAG>
                        <XMLATTR>"GUID" : $GUID</XMLATTR>
                        <XMLATTR>"REMOTEID" : $RemoteID</XMLATTR>
                        <XMLATTR>"MASTERID" : $MasterID</XMLATTR>
                        <XMLATTR>"ALTERID" : $AlterID</XMLATTR>
                    </LINE>
                    <FIELD NAME="BRIDGE Ledger Period Opening V1"><SET>$TBalOpening</SET><TYPE>Amount</TYPE><FORMAT>"No Symbol, No Comma"</FORMAT><XMLTAG>"OPENINGBALANCE"</XMLTAG></FIELD>
                    <FIELD NAME="BRIDGE Ledger Period Closing V1"><SET>$TBalClosing</SET><TYPE>Amount</TYPE><FORMAT>"No Symbol, No Comma"</FORMAT><XMLTAG>"CLOSINGBALANCE"</XMLTAG></FIELD>
                    <COLLECTION NAME="BRIDGE Ledger Period Collection V1" ISMODIFY="No" ISFIXED="No" ISINITIALIZE="Yes" ISOPTION="No" ISINTERNAL="No">
                        <TYPE>Ledger</TYPE>
                        <FETCH>GUID, RemoteID, MasterID, AlterID, TBalOpening, TBalClosing</FETCH>
                    </COLLECTION>
                </TDLMESSAGE>
            </TDL>
        </DESC>
    </BODY>
</ENVELOPE>
"#,
        xml_escape(company),
        xml_escape(from),
        xml_escape(to),
        xml_escape(from),
        xml_escape(to),
    )
    .trim()
    .to_string()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::{
        company_list_request, groups_request, ledger_period_balances_request, ledgers_request,
        legacy_company_list_request,
    };

    #[test]
    fn portable_read_profiles_preserve_the_existing_production_bytes() {
        assert_eq!(company_list_request(), legacy_company_list_request());
    }

    #[test]
    fn requests_only_fields_used_by_the_renderer() {
        let combined = format!(
            "{}{}{}",
            company_list_request(),
            groups_request("Synthetic Company"),
            ledgers_request("Synthetic Company")
        );
        for prohibited in [
            "NATIVEMETHOD>*",
            "$Address",
            "$INCOMETAXNUMBER",
            "$TANREGNO",
            "$TANUMBER",
            "$Website",
            "$Narration",
            "$StateName",
            "$PhoneNumber",
            "$Email",
            "$MOBILENUMBERS",
            "$Pincode",
            "$GSTRegNumber",
        ] {
            assert!(
                !combined.contains(prohibited),
                "unexpected TDL field: {prohibited}"
            );
        }
        assert!(company_list_request().contains("<FETCH>Name, GUID</FETCH>"));
    }

    #[test]
    fn exact_report_collection_is_shared_by_count_and_rows() {
        let ledgers = ledgers_request("BRIDGE SYNTHETIC BOOK");
        assert!(ledgers.contains("<TYPE>DATA</TYPE>"));
        assert!(ledgers.contains("<ID>BRIDGE Ledger Export V1</ID>"));
        assert!(ledgers.contains("$$NumItems:BRIDGE Ledger Collection V1"));
        assert!(ledgers
            .contains("<REPEAT>BRIDGE Ledger Row Line V1 : BRIDGE Ledger Collection V1</REPEAT>"));
        assert!(ledgers.contains("<XMLTAG>\"COMPANYCONTEXT\"</XMLTAG>"));
        assert!(ledgers.contains("<XMLATTR>\"GUID\" : $GUID</XMLATTR>"));
        assert!(ledgers.contains("<XMLATTR>\"REMOTEID\" : $RemoteID</XMLATTR>"));
        assert!(ledgers.contains("<XMLATTR>\"MASTERID\" : $MasterID</XMLATTR>"));
        assert!(ledgers.contains("<XMLATTR>\"ALTERID\" : $AlterID</XMLATTR>"));
        assert!(ledgers.contains("Name, GUID, RemoteID, MasterID, AlterID"));

        let groups = groups_request("BRIDGE SYNTHETIC BOOK");
        assert!(groups.contains("<TYPE>DATA</TYPE>"));
        assert!(groups.contains("<ID>BRIDGE Group Export V1</ID>"));
        assert!(groups.contains("$$NumItems:BRIDGE Group Collection V1"));
        assert!(groups
            .contains("<REPEAT>BRIDGE Group Row Line V1 : BRIDGE Group Collection V1</REPEAT>"));
        assert!(groups.contains("<TYPE>Group</TYPE>"));
    }

    #[test]
    fn report_requests_escape_all_user_controlled_static_variables() {
        let ledgers = ledgers_request("BRIDGE & <SYNTHETIC> \"BOOK\"");
        assert!(ledgers.contains("BRIDGE &amp; &lt;SYNTHETIC&gt; &quot;BOOK&quot;"));
        assert!(!ledgers.contains("BRIDGE & <SYNTHETIC>"));

        let groups = groups_request("BRIDGE & <GROUPS>");
        assert!(groups.contains("BRIDGE &amp; &lt;GROUPS&gt;"));

        let period = ledger_period_balances_request("BRIDGE & PERIOD", "2026<0401", "2026&0430");
        assert!(period.contains("BRIDGE &amp; PERIOD"));
        assert!(period.contains("2026&lt;0401"));
        assert!(period.contains("2026&amp;0430"));
    }

    #[test]
    fn period_balance_request_is_identity_scoped_and_name_free() {
        let request = ledger_period_balances_request("Synthetic Company", "20260401", "20260430");
        assert!(request.contains("bridge.tally.ledger-period-balances/1"));
        assert!(request.contains("<SET>$TBalOpening</SET>"));
        assert!(request.contains("<SET>$TBalClosing</SET>"));
        assert!(request.contains("<XMLTAG>\"ORDINARYBOOKSREQUESTED\"</XMLTAG>"));
        assert!(!request.contains("<XMLTAG>\"ORDINARYBOOKS\"</XMLTAG>"));
        assert!(request.contains("<XMLATTR>\"GUID\" : $GUID</XMLATTR>"));
        assert!(request.contains("<XMLTAG>\"FROMDATE\"</XMLTAG>"));
        assert!(!request.contains("<XMLTAG>\"NAME\"</XMLTAG>"));
    }
}
