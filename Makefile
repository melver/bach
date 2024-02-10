.PHONY: test
test:
	cargo test
	cargo run --bin bach-play 120 48 examples/simple.ch > /dev/null
	cargo run --bin bach-vm 120 48 examples/simple.bch > /dev/null
