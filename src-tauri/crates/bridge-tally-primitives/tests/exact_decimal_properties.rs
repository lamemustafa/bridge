use bridge_tally_primitives::ExactDecimal;
use proptest::{
    prelude::*,
    test_runner::{Config, RngSeed},
};

/// This seed is deliberately committed: a failing generated decimal must be
/// reproducible without relying on an ephemeral test-runner seed.
const PROPTEST_SEED: u64 = 0xB71D_6E0D_2026_0801;

fn config() -> Config {
    Config {
        cases: 512,
        rng_seed: RngSeed::Fixed(PROPTEST_SEED),
        ..Config::default()
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn exact_decimal_round_trips_every_grammar_accepted_lexeme(
        negative in any::<bool>(),
        whole in "[0-9]{1,255}",
        fractional in proptest::option::of("[0-9]{1,128}"),
    ) {
        let sign = if negative { "-" } else { "" };
        let value = match fractional {
            Some(fractional) => format!("{sign}{whole}.{fractional}"),
            None => format!("{sign}{whole}"),
        };
        prop_assume!(value.len() <= 256);
        let parsed = ExactDecimal::parse(value.clone()).expect("generated grammar is accepted");
        prop_assert_eq!(parsed.as_str(), value);
    }

    #[test]
    fn exact_decimal_rejects_lexemes_outside_its_grammar(
        prefix in ".{0,24}",
        excluded in prop_oneof![Just("+"), Just("e"), Just("E"), Just(" "), Just("\t"), Just("_"), Just("/"), Just("a")],
        suffix in ".{0,24}",
    ) {
        let value = format!("{prefix}{excluded}{suffix}");
        prop_assert!(ExactDecimal::parse(value).is_err());
    }
}
