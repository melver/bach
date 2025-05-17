# SPDX-License-Identifier: Apache-2.0
# Copyright (C) 2024, Marco Elver <me@marcoelver.com>

.PHONY: all
all: format test clippy

.PHONY: release
release:
	cargo build --release

.PHONY: format
format:
	cargo fmt --check

.PHONY: test
test:
	cargo test
	cargo run --bin bach-play 8000 48 examples/simple.ch - > /dev/null
	cargo run --bin bach-vm 8000 48 examples/simple.bch > /dev/null
	printf 's 1,@examples/simple.ch\nc\nF 5\nq\n' | cargo run --bin bach-evolve examples/evolve.cfg 8000 48 /dev/null > /dev/null
	printf 'a 3\npfx examples/simple.ch\nc\nq\n' | cargo run --bin bach-evolve - 8000 48 /dev/null > /dev/null

.PHONY: clippy
clippy:
	cargo clippy -- \
		-A clippy::too_many_arguments \
		-A clippy::comparison_chain \
		-A clippy::manual_range_contains \
		-A clippy::type_complexity \
		-D warnings

.PHONY: clean
clean:
	cargo clean
