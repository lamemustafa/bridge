//! A party ledger's dated GST registration history (bridge#624).
//!
//! A ledger's GSTIN can live only in its `LEDGSTREGDETAILS.LIST` entries, each
//! dated by `APPLICABLEFROM`, while the flat `PARTYGSTIN` is empty. The oldest
//! entry may carry no GSTIN at all. Captured on licensed TallyPrime 7.1
//! (2026-09-24, synthetic company): a gateway import keeps the dated list as
//! sent and does not copy a flat `PARTYGSTIN` into it, and a `List of Ledgers`
//! read returns the list only when its FETCH names it.
//!
//! The history is parsed once, here, and only [`GstRegistrationHistory::in_force`]
//! answers "which GSTIN applies on a date": there is no "first entry" accessor,
//! because reading the first entry reports a registered party as unregistered.
use bridge_tally_primitives::TallyDate;
use serde::{Deserialize, Serialize};

/// One dated registration entry, as Tally returned it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct GstRegistrationEntry {
    /// `YYYYMMDD`, as Tally writes `APPLICABLEFROM`.
    pub applicable_from: String,
    /// `None` when the entry names no GSTIN (for example an unregistered period).
    pub gstin: Option<String>,
    /// `GSTREGISTRATIONTYPE`, verbatim, when returned.
    pub registration_type: Option<String>,
}

/// Why a returned registration history cannot be read as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GstRegistrationDefect {
    /// An entry carries a GSTIN or a registration type but no date.
    EntryWithoutDate,
    /// An `APPLICABLEFROM` that is not a real `YYYYMMDD` calendar date.
    DateInvalid,
    /// One entry carried the same field twice.
    EntryRepeatsAField,
    /// Two entries share a date but say different things.
    ConflictingEntriesOnOneDate,
    /// A GSTIN that is not fifteen ASCII upper-case letters and digits.
    GstinMalformed,
}

/// What a party-master read observed of the registration history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "observation", rename_all = "snake_case")]
pub enum GstRegistrationHistory {
    /// The response carried no `LEDGSTREGDETAILS.LIST` element at all, so the
    /// history was not read. Not the same as an empty history.
    #[default]
    NotObserved,
    /// The history as returned: sorted by `applicable_from`, strictly
    /// increasing. Empty when Tally sent only an empty placeholder.
    Entries { entries: Vec<GstRegistrationEntry> },
    /// The history was returned but cannot be relied on.
    Unreadable { defect: GstRegistrationDefect },
}

/// One `LEDGSTREGDETAILS.LIST` element's fields before validation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawGstRegistrationEntry {
    pub applicable_from: Option<String>,
    pub gstin: Option<String>,
    pub registration_type: Option<String>,
    /// The element carried one of these fields more than once.
    pub repeated_field: bool,
}

impl RawGstRegistrationEntry {
    fn is_placeholder(&self) -> bool {
        !self.repeated_field
            && self.applicable_from.is_none()
            && self.gstin.is_none()
            && self.registration_type.is_none()
    }
}

impl GstRegistrationHistory {
    /// Validate the entries of one ledger. A defect here fails closed for this
    /// ledger only, so it never refuses the rest of a book (bridge#490). XML
    /// that is not well formed still fails the whole read, as for every field.
    pub fn from_raw(raw: Vec<RawGstRegistrationEntry>) -> Self {
        let mut entries = Vec::new();
        for entry in raw.into_iter().filter(|entry| !entry.is_placeholder()) {
            if entry.repeated_field {
                return Self::unreadable(GstRegistrationDefect::EntryRepeatsAField);
            }
            let Some(applicable_from) = entry.applicable_from else {
                return Self::unreadable(GstRegistrationDefect::EntryWithoutDate);
            };
            if TallyDate::parse(applicable_from.as_str()).is_err() {
                return Self::unreadable(GstRegistrationDefect::DateInvalid);
            }
            if entry
                .gstin
                .as_deref()
                .is_some_and(|gstin| !is_gstin_shaped(gstin))
            {
                return Self::unreadable(GstRegistrationDefect::GstinMalformed);
            }
            entries.push(GstRegistrationEntry {
                applicable_from,
                gstin: entry.gstin,
                registration_type: entry.registration_type,
            });
        }
        // Document order is not assumed to be date order.
        entries.sort_by(|a, b| a.applicable_from.cmp(&b.applicable_from));
        let mut deduplicated: Vec<GstRegistrationEntry> = Vec::with_capacity(entries.len());
        for entry in entries {
            match deduplicated.last() {
                Some(last) if last.applicable_from == entry.applicable_from => {
                    if *last != entry {
                        return Self::unreadable(
                            GstRegistrationDefect::ConflictingEntriesOnOneDate,
                        );
                    }
                }
                _ => deduplicated.push(entry),
            }
        }
        Self::Entries {
            entries: deduplicated,
        }
    }

    fn unreadable(defect: GstRegistrationDefect) -> Self {
        Self::Unreadable { defect }
    }

    /// The entry in force on `as_of` (`YYYYMMDD`): the one with the latest
    /// `applicable_from` on or before it. `None` when no entry applies yet, or
    /// when the history was not observed or is unreadable.
    pub fn in_force(&self, as_of: &str) -> Option<&GstRegistrationEntry> {
        match self {
            Self::Entries { entries } => entries
                .iter()
                .rev()
                .find(|entry| entry.applicable_from.as_str() <= as_of),
            Self::NotObserved | Self::Unreadable { .. } => None,
        }
    }
}

/// Fifteen ASCII upper-case letters and digits. A shape check only: the
/// checksum is not verified.
fn is_gstin_shaped(value: &str) -> bool {
    value.len() == 15
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

#[cfg(test)]
#[path = "gst_registration_tests.rs"]
mod tests;
