.DEFAULT_GOAL := help
.PHONY: help native-build native-check setup doctor check docs check-docs
help:
	@printf '%s\n' 'Native CLI:' '  make native-build  Build the standalone release binary' '  make native-check  Format check, Clippy, and Rust contracts' '' 'Compatibility platform and docs:' '  make setup         Install locked Python tooling with uv' '  make docs          Regenerate workflow reference and check links' '  make check-docs    Verify generated reference and local links' '  make check         Run policy, syntax, docs, and Python regressions'

native-build:
	cd rust && cargo build --locked --release

native-check:
	cd rust && cargo fmt --check
	cd rust && cargo clippy --locked --all-targets -- -D warnings
	cd rust && cargo test --locked

setup:
	python3 bin/ios-release setup

doctor:
	python3 bin/ios-release doctor

check:
	python3 bin/ios-release check --syntax

docs:
	python3 bin/ios-release docs

check-docs:
	python3 bin/ios-release docs --check
