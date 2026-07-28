// Package validate holds the E2E host-state validation logic ported from the
// Python pytest suite (tests/e2e_tests/*.py). Validations run over SSH against
// the installed VM and assert that the real host state matches both the Host
// Configuration and what Trident reports in its Host Status.
package validate

import (
	"errors"
	"fmt"
	"strings"

	"github.com/sirupsen/logrus"
)

// SoftAsserter accumulates sub-check results within a single storm test case so
// that every sub-check runs (and is logged individually) even after an earlier
// one fails, instead of bailing on the first failure via runtime.Goexit(). At
// the end of the case, call Summary() to log every sub-check that ran and
// Err() to pass the combined failure to tc.FailFromError.
//
// This is the interim approach (stage 1) chosen while storm lacks native
// soft-assert / subtest support: all sub-checks are reported as a single test
// case rather than per-sub-check JUnit rows, but every sub-check (pass and
// fail) is recorded and surfaced through Summary() so the exact set of checks
// performed is visible even when the case passes. See the E2E storm-port plan
// for the deferred per-subtest reporting enhancement.
type SoftAsserter struct {
	results []subCheck
}

// subCheck is the outcome of a single named sub-check. A nil err means the
// sub-check passed.
type subCheck struct {
	name string
	err  error
}

// Check runs fn and records its outcome under name. fn is always executed; a
// failure never stops subsequent checks.
func (s *SoftAsserter) Check(name string, fn func() error) {
	if err := fn(); err != nil {
		s.record(name, err)
	} else {
		s.pass(name)
	}
}

// Pass records and logs a passing sub-check. Use it to make an explicitly
// verified condition visible in the summary when the check is not expressed via
// Check/Assert (e.g. after a guard-style branch).
func (s *SoftAsserter) Pass(name string) {
	s.pass(name)
}

// Fail records and logs a failure for the named sub-check.
func (s *SoftAsserter) Fail(name string, err error) {
	s.record(name, err)
}

// Failf records and logs a formatted failure for the named sub-check.
func (s *SoftAsserter) Failf(name, format string, args ...any) {
	s.record(name, fmt.Errorf(format, args...))
}

// Assert records a pass if cond is true, otherwise a failure with the given
// message.
func (s *SoftAsserter) Assert(name string, cond bool, msgFormat string, args ...any) {
	if cond {
		s.pass(name)
	} else {
		s.record(name, fmt.Errorf(msgFormat, args...))
	}
}

func (s *SoftAsserter) pass(name string) {
	logrus.Debugf("validation sub-check passed: %s", name)
	s.results = append(s.results, subCheck{name: name})
}

func (s *SoftAsserter) record(name string, err error) {
	logrus.Errorf("validation sub-check failed: %s: %v", name, err)
	s.results = append(s.results, subCheck{name: name, err: err})
}

// failures returns each failed sub-check as an error prefixed with its name.
func (s *SoftAsserter) failures() []error {
	var errs []error
	for _, r := range s.results {
		if r.err != nil {
			errs = append(errs, fmt.Errorf("%s: %w", r.name, r.err))
		}
	}
	return errs
}

// HasFailures reports whether any sub-check has failed.
func (s *SoftAsserter) HasFailures() bool {
	return len(s.failures()) > 0
}

// Failures returns the number of failed sub-checks.
func (s *SoftAsserter) Failures() int {
	return len(s.failures())
}

// Summary returns a human-readable, ordered report of every sub-check that ran,
// each marked PASS or FAIL, prefixed with an aggregate count. It is intended to
// be logged at the end of a validation case so the exact set of checks
// performed is visible even when the case passes.
func (s *SoftAsserter) Summary() string {
	if len(s.results) == 0 {
		return "no validation sub-checks ran"
	}
	failed := len(s.failures())
	var b strings.Builder
	fmt.Fprintf(&b, "%d validation sub-check(s): %d passed, %d failed",
		len(s.results), len(s.results)-failed, failed)
	for _, r := range s.results {
		if r.err != nil {
			fmt.Fprintf(&b, "\n  FAIL  %s: %v", r.name, r.err)
		} else {
			fmt.Fprintf(&b, "\n  PASS  %s", r.name)
		}
	}
	return b.String()
}

// Err returns the combined error of all failed sub-checks, or nil if none
// failed. The result is suitable to pass directly to tc.FailFromError.
func (s *SoftAsserter) Err() error {
	errs := s.failures()
	if len(errs) == 0 {
		return nil
	}
	return fmt.Errorf("%d validation sub-check(s) failed: %w", len(errs), errors.Join(errs...))
}
