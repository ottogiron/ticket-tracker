.PHONY: build release test fmt fmt-check clippy check clean verify lint-md install

# ── Build ────────────────────────────────────────────────────────────
build:
	cargo build

release:
	cargo build --release

check:
	cargo check

clean:
	cargo clean

# ── Quality ──────────────────────────────────────────────────────────
test:
	cargo test

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets -- -D warnings

lint-md:
	npx markdownlint-cli2 "**/*.md"

# ── Quality gate (fmt-check + clippy + test + markdown lint) ─────────
verify: fmt-check clippy test lint-md

# ── Install ──────────────────────────────────────────────────────────
install:
	cargo install --path .
	@echo "Installed ticket to ~/.cargo/bin/"
