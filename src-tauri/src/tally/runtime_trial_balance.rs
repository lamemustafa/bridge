//! One native report operation shared by desktop and MCP callers.
use super::*;
use bridge_tally_protocol::native_statement_reports::{
    parse_native_statement, render_native_statement_request, NativeStatement, NativeStatementKind,
};
use bridge_tally_protocol::native_trial_balance::{
    parse_native_trial_balance, render_native_trial_balance_request, NativeTrialBalance,
};
use bridge_tally_protocol::TallyNamedMaster;

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

/// A Profit and Loss or Balance Sheet read: the Trial Balance it derives from,
/// the group tree that classifies it, and Tally's own Balance Sheet (the gate)
/// and, for a P&L, Tally's own Profit and Loss, all read inside one identity and
/// book-extent bracket (#692).
#[derive(Debug, Clone, Serialize)]
pub struct StatementsRead {
    pub trial_balance: TrialBalanceRead,
    pub derived: crate::reports::statements::DerivedStatements,
}

/// A Trial Balance whose company passed the single-INR admission: every ledger
/// of a book with one currency master. What guarantees that is that only this
/// module constructs it, in one place, right after `admit_inr` succeeds in the
/// same bracket; the admission argument records the dependency, and cannot by
/// itself prove it. A read of a several-currency book can never become one. The statement derivation accepts nothing else (#692; the
/// currency-scope work in #715 keeps its partial read a different type).
#[derive(Debug, Clone)]
pub(crate) struct SingleCurrencyTrialBalance(NativeTrialBalance);

impl SingleCurrencyTrialBalance {
    fn admitted(report: NativeTrialBalance, _admission: &PartyLedgerMasterCurrencyAssertion) -> Self {
        Self(report)
    }

    pub(crate) fn report(&self) -> &NativeTrialBalance {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn admitted_for_tests(report: NativeTrialBalance) -> Self {
        Self(report)
    }
}

/// What a statement read adds to a Trial Balance read.
struct StatementSources {
    trial_balance: SingleCurrencyTrialBalance,
    groups: Vec<TallyNamedMaster>,
    balance_sheet: NativeStatement,
    profit_and_loss: Option<NativeStatement>,
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
        self.fetch_trial_balance_with_extent(config, identity, period)
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
    ) -> anyhow::Result<(TrialBalanceRead, CompanyBookExtent)> {
        self.fetch_trial_balance_sources(config, identity, period, None)
            .await
            .map(|(read, _, extent)| (read, extent))
    }

    /// Tally's `kind` statement derived from the Trial Balance and group tree.
    /// Tally's own Balance Sheet for the same window gates every result, and for
    /// a P&L Tally's own Profit and Loss gates gross and net as well.
    pub(crate) async fn fetch_statements(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        period: TrialBalancePeriod,
        kind: NativeStatementKind,
    ) -> anyhow::Result<StatementsRead> {
        let (trial_balance, sources, _) = self
            .fetch_trial_balance_sources(config, identity, period, Some(kind))
            .await?;
        let sources = sources.ok_or_else(|| anyhow::anyhow!("statement_sources_not_read"))?;
        let derived = crate::reports::statements::derive_statements(
            &sources.trial_balance,
            &sources.groups,
            &sources.balance_sheet,
            sources.profit_and_loss.as_ref(),
        )?;
        Ok(StatementsRead {
            trial_balance,
            derived,
        })
    }

    async fn fetch_trial_balance_sources(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        period: TrialBalancePeriod,
        statement: Option<NativeStatementKind>,
    ) -> anyhow::Result<(TrialBalanceRead, Option<StatementSources>, CompanyBookExtent)> {
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
                        // Reuse the observed single-INR admission used by existing
                        // monetary reports. Multiple masters cannot establish base currency.
                        let admission = CompanyCurrencyRead {
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
                        let sources = match statement {
                            None => None,
                            Some(kind) => {
                                let request =
                                    render_native_group_snapshot_request(identity.display_name());
                                let (xml, bytes, hash) = client
                                    .fetch_native_report_paired(request.clone())
                                    .await?
                                    .require_stable(PairedReadValidationError::NativeLedgerGroup)?;
                                evidence = evidence
                                    .clone()
                                    .combine(RuntimeReadEvidence::paired(&request, hash, bytes));
                                let groups =
                                    parse_native_group_snapshot(&xml, identity.company_guid())?;
                                // Tally's own Balance Sheet gates both tools; its own
                                // Profit and Loss is read only for a P&L's report.
                                let request = render_native_statement_request(
                                    NativeStatementKind::BalanceSheet,
                                    identity.display_name(),
                                    &period,
                                );
                                let (xml, bytes, hash) = client
                                    .fetch_native_report_paired(request.clone())
                                    .await?
                                    .require_stable(PairedReadValidationError::NativeStatement)?;
                                evidence = evidence
                                    .clone()
                                    .combine(RuntimeReadEvidence::paired(&request, hash, bytes));
                                let balance_sheet =
                                    parse_native_statement(NativeStatementKind::BalanceSheet, &xml)?;
                                let profit_and_loss = if kind == NativeStatementKind::ProfitAndLoss {
                                    let request = render_native_statement_request(
                                        kind,
                                        identity.display_name(),
                                        &period,
                                    );
                                    let (xml, bytes, hash) = client
                                        .fetch_native_report_paired(request.clone())
                                        .await?
                                        .require_stable(PairedReadValidationError::NativeStatement)?;
                                    evidence = evidence
                                        .clone()
                                        .combine(RuntimeReadEvidence::paired(&request, hash, bytes));
                                    Some(parse_native_statement(kind, &xml)?)
                                } else {
                                    None
                                };
                                Some(StatementSources {
                                    trial_balance: SingleCurrencyTrialBalance::admitted(
                                        report.clone(),
                                        &admission,
                                    ),
                                    groups,
                                    balance_sheet,
                                    profit_and_loss,
                                })
                            }
                        };
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
                            },
                            sources,
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
