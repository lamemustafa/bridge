use quick_xml::{events::Event, name::QName, Reader};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct TallyEnvelope<T> {
    #[serde(rename = "BODY")]
    pub body: Option<T>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TallyCompany {
    pub name: String,
    pub state: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub mobile: Option<String>,
    pub pincode: Option<String>,
    pub gst_number: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TallyLedger {
    pub name: String,
    pub parent: Option<String>,
    pub party_gstin: Option<String>,
    pub opening_balance: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TallyVoucher {
    pub id: Option<String>,
    pub date: Option<String>,
    pub voucher_type: Option<String>,
    pub voucher_number: Option<String>,
    pub party_ledger_name: Option<String>,
}

pub fn parse_xml<T>(xml: &str) -> anyhow::Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    Ok(quick_xml::de::from_str(xml)?)
}

pub fn parse_companies(xml: &str) -> anyhow::Result<Vec<TallyCompany>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut companies = Vec::new();

    loop {
        match reader.read_event()? {
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"COMPANYINFO") =>
            {
                companies.push(parse_company_info(&mut reader)?);
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(companies)
}

pub fn parse_ledgers(xml: &str) -> anyhow::Result<Vec<TallyLedger>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut ledgers = Vec::new();

    loop {
        match reader.read_event()? {
            Event::Start(element) if element.name().as_ref().eq_ignore_ascii_case(b"LEDGER") => {
                let name = attr_value(&element, b"NAME");
                ledgers.push(parse_ledger(&mut reader, name)?);
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(ledgers)
}

pub fn parse_vouchers(xml: &str) -> anyhow::Result<Vec<TallyVoucher>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut vouchers = Vec::new();

    loop {
        match reader.read_event()? {
            Event::Start(element) if element.name().as_ref().eq_ignore_ascii_case(b"VOUCHER") => {
                let id = attr_value(&element, b"REMOTEID")
                    .or_else(|| attr_value(&element, b"GUID"))
                    .or_else(|| attr_value(&element, b"MASTERID"));
                vouchers.push(parse_voucher(&mut reader, id)?);
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(vouchers)
}

fn parse_company_info(reader: &mut Reader<&[u8]>) -> anyhow::Result<TallyCompany> {
    let mut company = TallyCompany {
        name: String::new(),
        state: None,
        phone: None,
        email: None,
        mobile: None,
        pincode: None,
        gst_number: None,
    };

    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();

                match name.as_slice() {
                    b"COMPANYNAMEFIELD" => {
                        company.name =
                            read_optional_text(reader, element.name())?.unwrap_or_default()
                    }
                    b"STATEFIELD" => company.state = read_optional_text(reader, element.name())?,
                    b"PHONEFIELD" => company.phone = read_optional_text(reader, element.name())?,
                    b"EMAILFIELD" => company.email = read_optional_text(reader, element.name())?,
                    b"MOBILEFIELD" | b"MOBILENUMBERSFIELD" => {
                        company.mobile = read_optional_text(reader, element.name())?
                    }
                    b"PINCODEFIELD" => {
                        company.pincode = read_optional_text(reader, element.name())?
                    }
                    b"GSTNUMBERFIELD" | b"GSTREGNUMBERFIELD" => {
                        company.gst_number = read_optional_text(reader, element.name())?
                    }
                    _ => {}
                }
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(b"COMPANYINFO") => {
                break
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(company)
}

fn parse_ledger(reader: &mut Reader<&[u8]>, name: Option<String>) -> anyhow::Result<TallyLedger> {
    let mut ledger = TallyLedger {
        name: name.unwrap_or_default(),
        parent: None,
        party_gstin: None,
        opening_balance: None,
    };

    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();

                match name.as_slice() {
                    b"PARENT" => ledger.parent = read_optional_text(reader, element.name())?,
                    b"PARTYGSTIN" => {
                        ledger.party_gstin = read_optional_text(reader, element.name())?
                    }
                    b"OPENINGBALANCE" => {
                        ledger.opening_balance = read_optional_text(reader, element.name())?
                    }
                    _ => {}
                }
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(b"LEDGER") => break,
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(ledger)
}

fn parse_voucher(reader: &mut Reader<&[u8]>, id: Option<String>) -> anyhow::Result<TallyVoucher> {
    let mut voucher = TallyVoucher {
        id,
        date: None,
        voucher_type: None,
        voucher_number: None,
        party_ledger_name: None,
    };

    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();

                match name.as_slice() {
                    b"DATE" => voucher.date = read_optional_text(reader, element.name())?,
                    b"VOUCHERTYPENAME" => {
                        voucher.voucher_type = read_optional_text(reader, element.name())?
                    }
                    b"VOUCHERNUMBER" => {
                        voucher.voucher_number = read_optional_text(reader, element.name())?
                    }
                    b"PARTYLEDGERNAME" => {
                        voucher.party_ledger_name = read_optional_text(reader, element.name())?
                    }
                    _ => {}
                }
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(b"VOUCHER") => {
                break
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(voucher)
}

fn attr_value(element: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attr| attr.key.as_ref().eq_ignore_ascii_case(key))
        .and_then(|attr| String::from_utf8(attr.value.as_ref().to_vec()).ok())
        .filter(|value| !value.trim().is_empty())
}

fn read_optional_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> anyhow::Result<Option<String>> {
    let value = reader.read_text(name)?;
    let decoded = value.decode()?;
    Ok(empty_to_none(decoded.trim()))
}

fn empty_to_none(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_companies, parse_ledgers, parse_vouchers};

    #[test]
    fn parses_company_info_rows() {
        let xml = r#"
<ENVELOPE>
  <COMPANYINFO>
    <COMPANYNAMEFIELD>Demo Pvt Ltd</COMPANYNAMEFIELD>
    <STATEFIELD>Maharashtra</STATEFIELD>
    <EMAILFIELD>accounts@example.com</EMAILFIELD>
    <PINCODEFIELD>400001</PINCODEFIELD>
  </COMPANYINFO>
</ENVELOPE>
"#;

        let companies = parse_companies(xml).expect("companies should parse");

        assert_eq!(companies.len(), 1);
        assert_eq!(companies[0].name, "Demo Pvt Ltd");
        assert_eq!(companies[0].state.as_deref(), Some("Maharashtra"));
        assert_eq!(companies[0].email.as_deref(), Some("accounts@example.com"));
    }

    #[test]
    fn parses_ledger_rows() {
        let xml = r#"
<ENVELOPE><BODY><DATA><COLLECTION>
  <LEDGER NAME="Customer A">
    <ADDRESS.LIST>
      <ADDRESS>Line 1</ADDRESS>
    </ADDRESS.LIST>
    <PARENT>Sundry Debtors</PARENT>
    <EMAIL>a@example.com</EMAIL>
    <PARTYGSTIN>27ABCDE1234F1Z5</PARTYGSTIN>
  </LEDGER>
</COLLECTION></DATA></BODY></ENVELOPE>
"#;

        let ledgers = parse_ledgers(xml).expect("ledgers should parse");

        assert_eq!(ledgers.len(), 1);
        assert_eq!(ledgers[0].name, "Customer A");
        assert_eq!(ledgers[0].parent.as_deref(), Some("Sundry Debtors"));
    }

    #[test]
    fn parses_voucher_rows() {
        let xml = r#"
<ENVELOPE><BODY><DATA><COLLECTION>
  <VOUCHER REMOTEID="abc">
    <DATE>20260401</DATE>
    <VOUCHERTYPENAME>Sales</VOUCHERTYPENAME>
    <VOUCHERNUMBER>1</VOUCHERNUMBER>
    <PARTYLEDGERNAME>Customer A</PARTYLEDGERNAME>
    <ALLLEDGERENTRIES.LIST>
      <LEDGERNAME>Customer A</LEDGERNAME>
      <ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>
      <AMOUNT>-1180.00</AMOUNT>
    </ALLLEDGERENTRIES.LIST>
  </VOUCHER>
</COLLECTION></DATA></BODY></ENVELOPE>
"#;

        let vouchers = parse_vouchers(xml).expect("vouchers should parse");

        assert_eq!(vouchers.len(), 1);
        assert_eq!(vouchers[0].voucher_type.as_deref(), Some("Sales"));
        let serialized = serde_json::to_string(&vouchers[0]).expect("serialize voucher");
        assert!(!serialized.contains("1180"));
    }

    #[test]
    fn ignores_nested_voucher_sections_while_parsing_known_fields() {
        let xml = r#"
<ENVELOPE><BODY><DATA><COLLECTION>
  <VOUCHER REMOTEID="abc">
    <DATE>20260401</DATE>
    <INVENTORYENTRIES.LIST>
      <STOCKITEMNAME>Item A</STOCKITEMNAME>
      <BATCHALLOCATIONS.LIST>
        <GODOWNNAME>Main Location</GODOWNNAME>
      </BATCHALLOCATIONS.LIST>
    </INVENTORYENTRIES.LIST>
    <ALLLEDGERENTRIES.LIST>
      <LEDGERNAME>Customer A</LEDGERNAME>
      <AMOUNT>-1180.00</AMOUNT>
      <BILLALLOCATIONS.LIST>
        <NAME>Bill 1</NAME>
      </BILLALLOCATIONS.LIST>
    </ALLLEDGERENTRIES.LIST>
  </VOUCHER>
</COLLECTION></DATA></BODY></ENVELOPE>
"#;

        let vouchers =
            parse_vouchers(xml).expect("nested voucher sections should not break parsing");

        assert_eq!(vouchers.len(), 1);
        assert_eq!(vouchers[0].date.as_deref(), Some("20260401"));
        let serialized = serde_json::to_string(&vouchers[0]).expect("serialize voucher");
        assert!(!serialized.contains("Customer A"));
        assert!(!serialized.contains("1180"));
    }
}
