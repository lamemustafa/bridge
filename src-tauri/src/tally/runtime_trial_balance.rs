//! One native report operation shared by desktop and MCP callers.
use super::*;
use bridge_tally_protocol::native_trial_balance::{
    parse_native_trial_balance, parse_native_trial_balance_with_currency,
    render_native_trial_balance_request, render_native_trial_balance_request_with_currency,
    NativeTrialBalance,
};

/// A completed observation, not a reusable admission for a later write.
#[derive(Debug, Clone, Serialize)]
pub struct TrialBalanceRead {
    pub company_guid: String,
    pub company_name: String,
    pub from: TallyDate,
    pub to: TallyDate,
    pub currency: CompanyCurrency,
    pub report: NativeTrialBalance,
    pub totals: crate::reports::trial_balance::TrialBalanceTotals,
    pub read_at: String,
    pub evidence: RuntimeReadEvidence,
    /// Which ledgers `report` and `totals` cover. Only the MCP read asks for a
    /// several-currency book's base-currency ledgers; the desktop screen, which
    /// cannot show what was left out, refuses such a book instead (bridge#551).
    #[serde(skip)]
    pub ledger_scope: TrialBalanceLedgerScope,
}

/// Whether a caller can present a Trial Balance that covers only part of the
/// book. A caller that cannot show the ledgers left out must never receive one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrialBalanceCurrencyScope {
    /// One Currency master only; several refuse (`company_base_currency_undetermined`).
    SingleCurrency,
    /// Several masters admitted through the identified INR base: the plain
    /// base-currency ledgers are read, the rest set aside by name.
    BaseCurrencyLedgersOnly,
}

/// The ledgers a Trial Balance read covers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TrialBalanceLedgerScope {
    /// Every ledger of a book with one Currency master.
    #[default]
    AllLedgers,
    /// A several-currency book's plain base-currency ledgers. Totals are not
    /// expected to balance and no balanced check is ever made over them.
    BaseCurrencyLedgersOnly {
        /// The identified base master's NAME (the one its ledgers carry).
        base_name: String,
        decimal_places: u8,
        foreign: Vec<bridge_tally_protocol::native_outstandings::ForeignCurrencyLedger>,
        mixed: Vec<String>,
    },
}

/// An ordered caller-selected range. Profile-specific boundary admission stays
/// inside the identity-bracketed runtime read.
#[derive(Debug, Clone)]
pub struct TrialBalancePeriod {
    from: TallyDate,
    to: TallyDate,
}

impl TrialBalancePeriod {
    pub(crate) fn new(from: TallyDate, to: TallyDate) -> Result<Self, TrialBalanceReadError> {
        if from > to {
            return Err(TrialBalanceReadError::Period(
                bridge_tally_protocol::native_outstandings::NativeLedgerSnapshotPeriodError::InvalidRange,
            ));
        }
        Ok(Self { from, to })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TrialBalanceReadError {
    #[error("trial_balance_education_unqualified")]
    EducationUnqualified,
    #[error("trial_balance_before_books")]
    BeforeBooks,
    #[error("trial_balance_period_not_honoured")]
    Period(bridge_tally_protocol::native_outstandings::NativeLedgerSnapshotPeriodError),
    #[error("{0}")]
    Currency(&'static str),
}

impl TrialBalanceReadError {
    pub(crate) fn safe_code(&self) -> &'static str {
        match self {
            Self::EducationUnqualified => "trial_balance_education_unqualified",
            Self::BeforeBooks => "trial_balance_before_books",
            Self::Period(_) => "trial_balance_period_not_honoured",
            Self::Currency(code) => code,
        }
    }
}

impl TallyRuntime {
    /// Native ledger-wise Trial Balance, with the same queue and read/write
    /// barrier as other monetary reads. Paired responses and stable book extent
    /// detect observed drift; they do not establish an atomic Tally snapshot.
    pub async fn fetch_trial_balance(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        period: TrialBalancePeriod,
    ) -> anyhow::Result<TrialBalanceRead> {
        self.fetch_trial_balance_with_extent(
            config,
            identity,
            period,
            TrialBalanceCurrencyScope::SingleCurrency,
        )
        .await
        .map(|(read, _)| read)
    }

    /// As [`Self::fetch_trial_balance`], also returning the book extent the
    /// read was pinned under: its opening and closing extents were equal, or
    /// the read refused. A caller can tell later whether the book has moved
    /// since (#630).
    pub(crate) async fn fetch_trial_balance_with_extent(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        period: TrialBalancePeriod,
        scope: TrialBalanceCurrencyScope,
    ) -> anyhow::Result<(TrialBalanceRead, CompanyBookExtent)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::MasterExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let identity = identity.clone();
                let from = period.from.clone();
                let to = period.to.clone();
                async move {
                    let mut evidence = RuntimeReadEvidence::empty();
                    let result = async {
                        let (profile, mode_evidence) = observe_read_boundary(&client).await?;
                        evidence = mode_evidence;
                        if profile == DateBoundaryProfile::EducationRestricted {
                            return Err(TrialBalanceReadError::EducationUnqualified.into());
                        }
                        let period =
                            NativeLedgerSnapshotPeriod::new(profile, from.clone(), to.clone())
                                .map_err(TrialBalanceReadError::Period)?;
                        bracket_verified_company_identity(&client, &identity).await?;
                        let extent = client.fetch_company_book_extent(&identity).await?;
                        if from < *extent.books_from() {
                            return Err(TrialBalanceReadError::BeforeBooks.into());
                        }

                        let currency_request =
                            render_company_currency_request(identity.display_name());
                        let (currency_xml, bytes, hash) = client
                            .fetch_native_report_paired(currency_request.clone())
                            .await?
                            .require_stable(PairedReadValidationError::CurrencyMaster)?;
                        evidence = evidence.clone().combine(RuntimeReadEvidence::paired(
                            &currency_request,
                            hash,
                            bytes,
                        ));
                        let currency = parse_company_currency(&currency_xml)?;
                        // A several-currency book is admitted only for a caller
                        // that can show the ledgers left out, through the base
                        // Tally identifies (bridge#551). Every other read keeps
                        // the single-INR admission of existing monetary reports.
                        let base = if scope == TrialBalanceCurrencyScope::BaseCurrencyLedgersOnly
                            && currency.currency_count > 1
                        {
                            let masters = parse_currency_master_list(&currency_xml)?;
                            let identified = identify_base_among_several(
                                &client,
                                &identity,
                                &mut evidence,
                                masters,
                            )
                            .await?
                            .ok_or(TrialBalanceReadError::Currency(
                                "company_base_currency_undetermined",
                            ))?;
                            if !identified.is_inr() {
                                return Err(TrialBalanceReadError::Currency(
                                    "company_base_currency_not_inr",
                                )
                                .into());
                            }
                            Some(identified)
                        } else {
                            CompanyCurrencyRead {
                                currency: currency.clone(),
                                extent: extent.clone(),
                                evidence: evidence.clone(),
                            }
                            .admit_inr()
                            .map_err(TrialBalanceReadError::Currency)?;
                            None
                        };

                        let request = match &base {
                            Some(_) => render_native_trial_balance_request_with_currency(
                                identity.display_name(),
                                &period,
                            ),
                            None => render_native_trial_balance_request(
                                identity.display_name(),
                                &period,
                            ),
                        };
                        let (xml, bytes, hash) = client
                            .fetch_native_report_paired(request.clone())
                            .await?
                            .require_stable(PairedReadValidationError::NativeLedgerCollection)?;
                        evidence = evidence
                            .clone()
                            .combine(RuntimeReadEvidence::paired(&request, hash, bytes));
                        let (report, ledger_scope) = match &base {
                            Some(identified) => {
                                let scoped = parse_native_trial_balance_with_currency(
                                    &xml,
                                    identity.company_guid(),
                                    identified.base(),
                                )?;
                                (
                                    scoped.report,
                                    TrialBalanceLedgerScope::BaseCurrencyLedgersOnly {
                                        base_name: identified.base().name().to_string(),
                                        decimal_places: identified.decimal_places(),
                                        foreign: scoped.foreign_currency_ledgers,
                                        mixed: scoped.mixed_currency_ledgers,
                                    },
                                )
                            }
                            None => (
                                parse_native_trial_balance(&xml, identity.company_guid())?,
                                TrialBalanceLedgerScope::AllLedgers,
                            ),
                        };
                        let totals = crate::reports::trial_balance::observed_totals(&report)?;
                        let closing_extent = client.fetch_company_book_extent(&identity).await?;
                        if closing_extent != extent {
                            return Err(PairedReadValidationError::NativeLedgerExtent.into());
                        }
                        bracket_verified_company_identity(&client, &identity).await?;
                        evidence = evidence
                            .clone()
                            .combine(confirm_read_boundary(&client, profile).await?);
                        Ok((
                            TrialBalanceRead {
                                company_guid: identity.company_guid().to_string(),
                                company_name: identity.display_name().to_string(),
                                from,
                                to,
                                currency,
                                report,
                                totals,
                                read_at: chrono::Utc::now().to_rfc3339(),
                                evidence: evidence.clone(),
                                ledger_scope,
                            },
                            extent,
                        ))
                    }
                    .await;
                    result.map_err(|error| with_read_evidence(error, evidence))
                }
            },
        )
        .await
    }
}
