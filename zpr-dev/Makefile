# The four standard targets every ZPR Rust repository exposes
# (docs/BUILD.md, "Common conventions"). `make check` is the local build gate:
# formatting must be clean and warnings are errors.

.PHONY: all build test check clean

all: build

build:
	cargo build --all-targets

test:
	cargo test

check:
	cargo fmt --check && cargo build --all-targets --config 'build.rustflags = ["-D", "warnings"]'

clean:
	cargo clean

.DEFAULT_GOAL := all
