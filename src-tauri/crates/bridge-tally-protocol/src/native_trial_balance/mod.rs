//! Native Trial Balance request, model, and strict wire parser.

mod wire;

pub use wire::{
    parse_native_trial_balance, render_native_trial_balance_request, NativeTrialBalance,
    NativeTrialBalanceAmount, NativeTrialBalanceError, NativeTrialBalanceRow,
};

#[cfg(test)]
mod tests;
