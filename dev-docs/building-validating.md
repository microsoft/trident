# Building and Validating

Build instructions: `cargo build`.

Run UTs: `make test`. Run UTs with code coverage: `make ut-coverage`.

Collect code coverage report: `make coverage-report`. You can also run `make
coverage' to execute UTs and collect code coverage report. More on that below in
section [Reviewing test code coverage](code-coverage.md).

Run functional tests: `make functional-test`. Rerun tests: `make
patch-functional-test`. More details can be found in the [Functional Tests
section](testing.md#functional-tests). If you want to validate the functional test
building, run `make build-functional-test`. `functional-test` and
`patch-functional-test` will automatically ather code coverage data, which can
be viewed using `make coverage-report`.

Validate many steps done by pipelines: `make`.

Rebuild trident_api documentation: `make build-api-docs`.
