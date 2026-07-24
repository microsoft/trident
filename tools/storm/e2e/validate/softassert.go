// Package validate holds the E2E host-state validation logic ported from the
// Python pytest suite (tests/e2e_tests/*.py). Validations run over SSH against
// the installed VM and assert that the real host state matches both the Host
// Configuration and what Trident reports in its Host Status.
package validate

import (
	"errors"
	"fmt"

	"github.com/sirupsen/logrus"
)

// SoftAsserter accumulates sub-check failures within a single storm test case
// so that every sub-check runs (and is logged individually) even after an
// earlier one fails, instead of bailing on the first failure via
// runtime.Goexit(). At the end of the case, call Err() and pass the result to
// tc.FailFromError.
//
// This is the interim approach (stage 1) chosen while storm lacks native
// soft-assert / subtest support: all sub-checks run and each failure is logged,
// but they are reported as a single failed test case (one combined message)
// rather than per-sub-check JUnit rows. See the E2E storm-port plan for the
// deferred per-subtest reporting enhancement.
type SoftAsserter struct {
	errs []error
}

// Check runs fn and, if it returns an error, records and logs it prefixed with
// name. fn is always executed; a failure never stops subsequent checks.
func (s *SoftAsserter) Check(name string, fn func() error) {
	if err := fn(); err != nil {
		s.record(name, err)
	}
}

// Fail records and logs a failure for the named sub-check.
func (s *SoftAsserter) Fail(name string, err error) {
	s.record(name, err)
}

// Failf records and logs a formatted failure for the named sub-check.
func (s *SoftAsserter) Failf(name, format string, args ...any) {
	s.record(name, fmt.Errorf(format, args...))
}

// Assert records a failure with the given message if cond is false.
func (s *SoftAsserter) Assert(name string, cond bool, msgFormat string, args ...any) {
	if !cond {
		s.record(name, fmt.Errorf(msgFormat, args...))
	}
}

func (s *SoftAsserter) record(name string, err error) {
	wrapped := fmt.Errorf("%s: %w", name, err)
	logrus.Errorf("validation sub-check failed: %v", wrapped)
	s.errs = append(s.errs, wrapped)
}

// HasFailures reports whether any sub-check has failed.
func (s *SoftAsserter) HasFailures() bool {
	return len(s.errs) > 0
}

// Failures returns the number of failed sub-checks.
func (s *SoftAsserter) Failures() int {
	return len(s.errs)
}

// Err returns the combined error of all failed sub-checks, or nil if none
// failed. The result is suitable to pass directly to tc.FailFromError.
func (s *SoftAsserter) Err() error {
	if len(s.errs) == 0 {
		return nil
	}
	return fmt.Errorf("%d validation sub-check(s) failed: %w", len(s.errs), errors.Join(s.errs...))
}
