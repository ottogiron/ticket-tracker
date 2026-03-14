.PHONY: build test fmt fmt-check clippy verify clean

build:
	cargo build

test:
	cargo test

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets -- -D warnings

verify: fmt-check clippy test

clean:
	cargo clean
