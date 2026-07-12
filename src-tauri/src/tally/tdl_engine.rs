pub fn company_list_request() -> String {
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
                            Company Name Header, Address Header, State Header, Phone Header, Email Header,
                            INCOMETAXNUMBER Header, MOBILENUMBERS Header, TANREGNO Header, TANUMBER Header,
                            Website Header, Pincode Header
                        </LEFTFIELDS>
                    </LINE>
                    <FIELD NAME="Company Name Header"><SET>"Company Name"</SET></FIELD>
                    <FIELD NAME="Address Header"><SET>"Address"</SET></FIELD>
                    <FIELD NAME="State Header"><SET>"State"</SET></FIELD>
                    <FIELD NAME="Phone Header"><SET>"Phone"</SET></FIELD>
                    <FIELD NAME="Email Header"><SET>"Email"</SET></FIELD>
                    <FIELD NAME="INCOMETAXNUMBER Header"><SET>"INCOMETAXNUMBER"</SET></FIELD>
                    <FIELD NAME="MOBILENUMBERS Header"><SET>"MOBILENUMBERS"</SET></FIELD>
                    <FIELD NAME="TANREGNO Header"><SET>"TANREGNO"</SET></FIELD>
                    <FIELD NAME="TANUMBER Header"><SET>"TANUMBER"</SET></FIELD>
                    <FIELD NAME="Website Header"><SET>"Website"</SET></FIELD>
                    <FIELD NAME="Pincode Header"><SET>"Pincode"</SET></FIELD>
                    <LINE NAME="Company Details">
                        <LEFTFIELDS>
                            Company Name Field, Address Field, State Field, Phone Field, Email Field,
                            INCOMETAXNUMBER Field, MOBILENUMBERS Field, TANREGNO Field, TANUMBER Field,
                            Website Field, Pincode Field
                        </LEFTFIELDS>
                        <XMLTAG>"CompanyInfo"</XMLTAG>
                    </LINE>
                    <FIELD NAME="Company Name Field"><SET>$Name</SET></FIELD>
                    <FIELD NAME="Address Field"><SET>$Address</SET></FIELD>
                    <FIELD NAME="State Field"><SET>$StateName</SET></FIELD>
                    <FIELD NAME="Phone Field"><SET>$PhoneNumber</SET></FIELD>
                    <FIELD NAME="Email Field"><SET>$Email</SET></FIELD>
                    <FIELD NAME="INCOMETAXNUMBER Field"><SET>$INCOMETAXNUMBER</SET></FIELD>
                    <FIELD NAME="MOBILENUMBERS Field"><SET>$MOBILENUMBERS</SET></FIELD>
                    <FIELD NAME="TANREGNO Field"><SET>$TANREGNO</SET></FIELD>
                    <FIELD NAME="TANUMBER Field"><SET>$TANUMBER</SET></FIELD>
                    <FIELD NAME="Website Field"><SET>$Website</SET></FIELD>
                    <FIELD NAME="Pincode Field"><SET>$Pincode</SET></FIELD>
                    <COLLECTION NAME="CompanyCollection">
                        <TYPE>Company</TYPE>
                        <FETCH>
                            Name, Address, StateName, PhoneNumber, Email,
                            INCOMETAXNUMBER, MOBILENUMBERS, TANREGNO, TANUMBER,
                            Website, Pincode
                        </FETCH>
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
            <NATIVEMETHOD>*</NATIVEMETHOD>
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
    format!(
        r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>EXPORT</TALLYREQUEST>
        <TYPE>COLLECTION</TYPE>
        <ID>Ledgers</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
                <SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY>
            </STATICVARIABLES>
            <TDL>
                <TDLMESSAGE>
                    <COLLECTION ISMODIFY="No" ISFIXED="No" ISINITIALIZE="No" ISOPTION="No" ISINTERNAL="No" NAME="Ledgers">
                        <TYPE>Ledger</TYPE>
                        <NATIVEMETHOD>*</NATIVEMETHOD>
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

pub fn vouchers_request(company: &str, from: &str, to: &str) -> String {
    format!(
        r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>EXPORT</TALLYREQUEST>
        <TYPE>COLLECTION</TYPE>
        <ID>Vouchers</ID>
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
                    <COLLECTION NAME="Vouchers" ISMODIFY="No" ISFIXED="No" ISINITIALIZE="No" ISOPTION="No" ISINTERNAL="No">
                        <TYPE>Voucher</TYPE>
                        <NATIVEMETHOD>*</NATIVEMETHOD>
                    </COLLECTION>
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

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
