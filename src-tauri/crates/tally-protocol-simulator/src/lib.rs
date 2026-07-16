//! Deterministic, synthetic Tally protocol fixtures and a one-request loopback server.
//!
//! This crate never launches Tally and cannot bind a non-loopback interface.

mod fixtures;
mod generator;
mod master_generator;
mod server;

pub use fixtures::{
    decode, encode, Delivery, Fixture, ProductStatus, ResponseContentEncoding, ResponseFraming,
    ScenarioPlan, WireEncoding,
};
pub use generator::{
    generate_voucher_window, GeneratedWindow, VoucherCorpusSpec, VoucherGenerationError,
    VoucherWindowSpec, MAX_GENERATED_RECORDS, MAX_GENERATED_RECORDS_PER_WINDOW,
    MAX_GENERATED_WINDOW_BYTES,
};
pub use master_generator::{
    generate_master_corpus, GeneratedMasterCorpus, MasterCorpusSpec, MasterGenerationError,
    MAX_GENERATED_MASTERS, MAX_MASTER_TEXT_WIDTH,
};
pub use server::{ObservedRequest, SequenceSimulator, Simulator, MAX_SEQUENCE_REQUESTS};
