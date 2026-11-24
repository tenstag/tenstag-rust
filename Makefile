# Checks to run before every commit. `make check` runs them all, and any warning counts as an error.
#
#   fmt     the code is formatted with rustfmt
#   clippy  clippy and the crate's lints (see [lints] in Cargo.toml) pass
#   doc     the API docs build, with no broken links and no missing docs
#   test    unit and doc tests pass, the tenstag specification's test vectors among them
#
# `make cross` (not part of check: it adds Rust targets) builds and lints the code for Windows and macOS, so that
# their code paths at least compile; nothing is run there.
#
# `make spec` (not part of check: it needs a tenstag specification
# checkout, TENSTAG_SPEC=<path>) checks that every document and rule the code cites exists at the commit in
# spec-pin.toml, and that tests/vectors/ are the specification's test vectors at its commit.

.PHONY: check fmt clippy doc test spec cross

check: fmt clippy doc test

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets -- -D warnings

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

test:
	cargo test

spec:
	python3 tools/check_spec_citations.py

cross:
	sh tools/cross_check.sh
