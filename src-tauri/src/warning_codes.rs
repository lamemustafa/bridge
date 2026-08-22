use serde::{Deserialize, Serialize};

macro_rules! declared_warning_codes {
    ($($variant:ident => $code:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum WarningCode {
            $($variant),+
        }

        impl WarningCode {
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $code),+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|warning| warning.as_str() == value)
            }
        }
    };
}

// This is the single declaration for every warning code that can reach a
// durable proof export. Keeping it below both `sync` and `tally` preserves
// N14's exhaustive export contract without creating a tally-to-sync edge.
declared_warning_codes! {
    AdaptiveWindowSplit => "adaptive_window_split",
    ForeignMasterTextRenderingDegraded => "foreign_master_text_rendering_degraded",
    NativeOutstandingsAsOfUnconfirmedWithoutEffectiveDateEvidence => "native_outstandings_as_of_unconfirmed_without_effective_date_evidence",
    NativeOutstandingsAsOfUnconfirmedWithoutBillReferences => "native_outstandings_as_of_unconfirmed_without_bill_references",
}
