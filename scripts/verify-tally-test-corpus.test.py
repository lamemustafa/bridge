"""Offline contract tests for the corpus-qualification boundary."""

import importlib.util
import pathlib


SCRIPT = pathlib.Path(__file__).resolve().parent / "verify-tally-test-corpus.py"


def main():
    spec = importlib.util.spec_from_file_location("verify_tally_test_corpus", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    assert module.main([]) == 2


if __name__ == "__main__":
    main()
