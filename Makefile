.PHONY: test
test:
	cargo test
	cargo run --bin bach-play 2000 48 examples/simple.ch - > /dev/null
	cargo run --bin bach-vm 2000 48 examples/simple.bch > /dev/null
