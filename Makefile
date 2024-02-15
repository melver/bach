.PHONY: test
test:
	cargo test
	cargo run --bin bach-play 1000 48 examples/simple.ch > /dev/null
	cargo run --bin bach-vm 1000 48 examples/simple.bch > /dev/null
