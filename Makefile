.PHONY: help check clippy fmt fmt-check test clean all

help:
	@echo "Available targets:"
	@echo "  make check      - Run cargo check on all workspace members"
	@echo "  make clippy     - Run clippy with project-specific lints"
	@echo "  make fmt        - Format code using rustfmt"
	@echo "  make fmt-check  - Check formatting without modifying files"
	@echo "  make test       - Run all tests"
	@echo "  make all        - Run check, clippy, fmt-check, and test"
	@echo "  make clean      - Clean build artifacts"

check:
	cargo check --workspace --all-targets --all-features

clippy:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

fmt:
	cargo +nightly fmt --all

fmt-check:
	cargo +nightly fmt --all -- --check

test:
	cargo test --workspace --all-features

clean:
	cargo clean

all: check clippy fmt-check test
