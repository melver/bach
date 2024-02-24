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
	echo $$'c\nF 5\nq' | cargo run --bin bach-evolve examples/evolve.cfg 8000 48 /dev/null > /dev/null
	echo $$'a 3\nq' | cargo run --bin bach-evolve - 8000 48 /dev/null > /dev/null

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
