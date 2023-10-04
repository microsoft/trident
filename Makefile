.PHONY: all
all: check build test rpm

.PHONY: check
check:
	cargo check
	cargo clippy -- -D warnings
	cargo fmt -- --check

.PHONY: build
build:
	cargo build --release

.PHONY: test
test:
	cargo test --all

.PHONE: coverage
coverage:
	CARGO_INCREMENTAL=0 RUSTFLAGS='-Cinstrument-coverage' LLVM_PROFILE_FILE='cargo-test-%p-%m.profraw' cargo test --all
	# cargo install grcov
	mkdir -p target/coverage
	grcov . --binary-path ./target/debug/deps/ -s . -t html,lcov,covdir --branch --ignore-not-existing --ignore '../*' --ignore "/*" -o target/coverage
	jq .coveragePercent target/coverage/covdir

.PHONY: rpm
rpm:
	docker build -t trident/trident:latest .
	mkdir -p bin/
	id=$$(docker create trident/trident:latest) && \
	docker cp $$id:/work/trident.tar.gz bin/ && \
	docker rm -v $$id && \
	tar xf bin/trident.tar.gz -C bin/

.PHONY: clean
clean:
	cargo clean
	rm -rf bin/
	find . -name "*.profraw" -type f -delete

.PHONY: setsail-docs
setsail-docs:
	cargo build --release --package setsail --bin docbuilder --features tera,itertools
	mkdir -p target/setsail-docs
	target/release/docbuilder -o target/setsail-docs
	@echo Wrote docs to target/setsail-docs