# Reviewing test code coverage

You can collect the data for computing UT code coverage by running `make
ut-coverage`. This will produce `*.profraw` files under `target/coverage`.

You can collect both the UT and functional test code coverage by running `make
functional-test` or `make patch-functional-test`. This will produce `*.profraw`
files under `target/coverage`.

To view the code coverage report, run `make coverage-report`. This will look for
all `*.profraw` files and produce several coverage reports under
`target/coverage`. It will also print out to standard output the overall code
coverage from the available `*.profraw` files. We are currently producing the
following reports: `html`, `lcov`, `covdir`, `cobertura`:

- The `html` report is the easiest to view:
  [target/coverage/html/index.html](target/coverage/html/index.html) (note that
  this file is not checked in, only generated on demand by running `make
  coverage-report`). You can look at [Documentation](../README.md#documentation) section for
  more details on viewing the `html` remotely through VSCode.
- The `lcov` is used by `Coverage Gutters` VSCode extension to show code
  coverage directly over the code in the VSCode editor, which helps to see which
  lines are covered and which not.
- The `covdir` report is in the JSON format, so easy for automated processing.
  The `coverage-report` target actually prints the overall coverage as extracted
  from the `covdir` report.
- The `cobertura` report is something that ADO understands and is published
  during pipeline run to ADO, so that we can see code coverage as part of
  pipeline run results.
