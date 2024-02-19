.PHONY: test
test:
	cargo test
	cargo run --bin bach-play 8000 48 examples/simple.ch - > /dev/null
	cargo run --bin bach-vm 8000 48 examples/simple.bch > /dev/null
	echo -e 'c\nF 5\nq\n' | cargo run --bin bach-evolve - 8000 48 /dev/null > /dev/null
