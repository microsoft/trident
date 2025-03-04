# Storm

Storm is a golang scenario-based imperative sequential testing library.

Meaning:

- golang: written in and for golang.
- scenario-based: tests are organized as scenarios.
- imperative: tests are written in an imperative style.
- sequential: tests are defined in a strict sequence and they are dependent on
  all previous tests.

## Contents

- [Storm](#storm)
  - [Contents](#contents)
  - [Concepts](#concepts)
  - [How do I use it?](#how-do-i-use-it)
    - [CLI Usage](#cli-usage)
    - [Entry Point Definition](#entry-point-definition)
    - [Scenarios](#scenarios)
    - [Helpers](#helpers)
    - [Defining Runtime Args for Scenarios and Helpers](#defining-runtime-args-for-scenarios-and-helpers)
    - [The `Run` Method](#the-run-method)
    - [Logging](#logging)
    - [Test Cases](#test-cases)
      - [Test Case Logging](#test-case-logging)
  - [Hello World Example](#hello-world-example)
  - [What Are All These Folders?\\](#what-are-all-these-folders)
  - [TODOs](#todos)

## Concepts

- **Suite**: A suite is a collection of scenarios and helpers. It is the main
  entry point for a storm-based binary.

- **Scenario**: A scenario is a large collection of sequential tests. These will
  generally cover end-to-end testing for a specific feature or component. It may
  also include setup and cleanup logic.

- **Helper**: A helper is a small(er) piece of code that may be invoked
  individually. Their main function is to provide an easy way to write Go-based
  test code as opposed to Python or Bash.

A helper may include a set of tests, but it is not required.

## How do I use it?

Every test suite is a standalone binary: `storm-<suite-name>`. You can run it
like any other go binary.

### CLI Usage

```text
Usage: storm-<suite-name> <command> [flags]

Flags:
  -h, --help              Show context-sensitive help.
  -v, --verbosity=info    Set log level

Commands:
  list scenarios [flags]
    List available scenarios

  list tags
    List all tags

  list stage-paths [flags]
    List all stage paths

  list helpers
    List all helpers

  run <scenario> [<scenario-args> ...] [flags]
    Run a specific scenario

  helper <helper> [<helper-args> ...] [flags]
    Run a specific helper
```

### Entry Point Definition

The entry point for each suite is the `main` function defined in `cmd/<suite-name>/main.go`.

This is a sample main function:

```go
package main

import (
    "storm/pkg/storm"
)

func main() {
    storm := storm.CreateSuite("trident")

    // Add your scenarios/helpers to the suite here!

    storm.Run()
}
```

### Scenarios

Scenarios should be defined inside `suites/<suite-name>/`.

A scenario is a struct that implements the `storm.Scenario` interface.
It is recommended to compose the `storm.BaseScenario` struct to get the default
implementation of the interface.

The bare minimum for a scenario is to implement the `Name` and `Run` methods.

### Helpers

Helpers should be defined inside `suites/<suite-name>/`. *Preferably* in a
helpers module.

A helper is a struct that implements the `storm.Helper` interface.
It is recommended to compose the `storm.BaseHelper` struct to get the default
implementation of the interface.

### Defining Runtime Args for Scenarios and Helpers

Both the `storm.Scenario` and `storm.Helper` interfaces include an `Args` method
that MUST return a pointer to a [kong](github.com/alecthomas/kong)-annotated struct.

Example from the `helloworld` suite:

```go
type HelloWorldHelper struct {
    args struct {
        Name string `arg:"" help:"Name of the helper" default:"default" required:""`
    }
}

func (h *HelloWorldHelper) Args() any {
    // 👆 IMPORTANT: Note that the receiver is a POINTER! If you receive by 
    // value, a copy of the struct is made so the returned pointer will point
    // to a copy of the struct and not the original struct.

    //    👇 Note that the returned value is a POINTER too!
    return &h.args
}
```

### The `Run` Method

The `Run` method contains the actual logic for a given scenario or helper.

```go
// For SCENARIOS, the signature is:
func (s MyScenario) Run(ctx storm.Context) error {
    // Your scenario logic here
}

// For HELPERS, the signature is:
func (h MyHelper) Run(ctx storm.HelperContext) error {
    // Your helper logic here
}
```

### Logging

The `storm.Context` and `storm.HelperContext` types provide a `Logger` method
that returns pointer to a [`logrus.Logger`
object](https://github.com/sirupsen/logrus). This object can be used to log
messages to the global suite logger.

```go
func (s MyScenario) Run(ctx storm.Context) error {
    ctx.Logger().Info("Hello, world!")
}
```

### Test Cases

Both `storm.Context` and `storm.HelperContext` provide a `NewTestCase(name
string)` method that returns a new implementation of the [`storm.TestCase` interface](pkg/storm/core/reporter.go).

Test cases MUST have unique names within each scenario or helper, and ideally
across the entire suite, unless the same test case is performed in multiple
scenarios/helpers.

```go
func (s MyScenario) Run(ctx storm.Context) error {
    tc := ctx.NewTestCase("my-test-case")
    // Your test case logic here
}
```

The `TestCase` interface behaves similarly to the `testing.T` interface in the
standard library. It provides the following methods for reporting results:

- `Pass()`: Explicitly marks the test case as passed. This is not generally
  required as the reporter will automatically mark the test case as passed if no
  errors are reported, or if the next test case is started.
- `Fail(reason string)`: Marks the test case as failed and stop execution of the
  current goroutine.
- `FailFromError(err error)`: Same as `Fail`, but the reason is set to the error
  message.
- `Skip(reason string)`: Marks the test case as skipped and stop execution of the
  current goroutine.

```go
func (s MyScenario) Run(ctx storm.Context) error {
    tc := ctx.NewTestCase("my-test-case")
    err := someFunction()
    if err != nil {
        tc.FailFromError(err)
    }
}
```

#### Test Case Logging

Additionally, the `TestCase` interface provides a `Logger()` method that returns a
pointer to a `logrus.Logger` object owned by the test case itself. This logger
can be used to log messages specific to the test case.

```go
func (s MyScenario) Run(ctx storm.Context) error {
    tc := ctx.NewTestCase("my-test-case")
    tc.Logger().Info("Hello, world!")
    tc.Pass()
}
```

Logs entries from the test case logger will be shown in the global suite logger
if they meet the log level threshold with a prefix in the form `[index:name] >
<LOG-BODY>`. In the background, logs of ALL levels are recorded to be shown
should the test case fail.

## Hello World Example

See the `helloworld` suite for a simple example of how to use Storm.

- [Entry point](cmd/storm-helloworld/main.go)
- [Scenario/Helper](suites/helloworld/helloworld.go)

## What Are All These Folders?\

- `cmd`: Contains the entry point for each suite.
- `pkg/storm`: Contains the public storm library.
- `internal`: Contains logic internal to the storm library.
- `suites`: Contains the scenarios and helpers for each suite.

## TODOs

- [ ] Document tags
- [ ] Document stage paths
- [ ] Document the `storm` package
- [X] Create a common runner for helpers and scenarios
- [X] Finish scenario runner
- [ ] Port the existing tests to Storm
