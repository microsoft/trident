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
	cargo test

.PHONY: rpm
rpm:
	docker build -t trident/trident:latest .
	id=$$(docker create trident/trident:latest) && \
	docker cp $$id:/usr/src/mariner/RPMS/x86_64/trident-0.1.0-1.cm2.x86_64.rpm . && \
	docker rm -v $$id
