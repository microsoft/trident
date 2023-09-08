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