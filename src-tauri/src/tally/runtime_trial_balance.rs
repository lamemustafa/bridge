//! One native report operation shared by desktop and MCP callers.
use super::*;
use bridge_tally_protocol::native_trial_balance::{
    parse_native_trial_balance, render_native_trial_balance_request, NativeTrialBalance,
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
                        let extent = client
                            .fetch_company_book_extent(
                                identity.display_name(),
                                identity.company_guid(),
                            )
                            .await?;
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
                        // Reuse the observed single-INR admission used by existing
                        // monetary reports. Multiple masters cannot establish base currency.
                        CompanyCurrencyRead {
                            currency: currency.clone(),
                            extent: extent.clone(),
                            evidence: evidence.clone(),
                        }
                        .admit_inr()
                        .map_err(TrialBalanceReadError::Currency)?;

                        let request =
                            render_native_trial_balance_request(identity.display_name(), &period);
                        let (xml, bytes, hash) = client
                            .fetch_native_report_paired(request.clone())
                            .await?
                            .require_stable(PairedReadValidationError::NativeLedgerCollection)?;
                        evidence = evidence
                            .clone()
                            .combine(RuntimeReadEvidence::paired(&request, hash, bytes));
                        let report = parse_native_trial_balance(&xml, identity.company_guid())?;
                        let totals = crate::reports::trial_balance::observed_totals(&report)?;
                        let closing_extent = client
                            .fetch_company_book_extent(
                                identity.display_name(),
                                identity.company_guid(),
                            )
                            .await?;
                        if closing_extent != extent {
                            return Err(PairedReadValidationError::NativeLedgerExtent.into());
                        }
                        bracket_verified_company_identity(&client, &identity).await?;
                        evidence = evidence
                            .clone()
                            .combine(confirm_read_boundary(&client, profile).await?);
                        Ok(TrialBalanceRead {
                            company_guid: identity.company_guid().to_string(),
                            company_name: identity.display_name().to_string(),
                            from,
                            to,
                            currency,
                            report,
                            totals,
                            read_at: chrono::Utc::now().to_rfc3339(),
                            evidence: evidence.clone(),
                        })
                    }
                    .await;
                    result.map_err(|error| with_read_evidence(error, evidence))
                }
            },
        )
        .await
    }
}
