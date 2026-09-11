package scenario

import (
	"context"
	"fmt"
	"strings"
	"time"

	"tridenttools/storm/utils/metrics"
	"tridenttools/storm/utils/sshutils"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

// Operations legacy recorded boot timings for, kept verbatim so storm rows
// group with the existing ones in Kusto. Legacy only ever measured the clean
// install and the first A/B update; the extra servicing phases storm runs are
// deliberately not measured, to avoid introducing operation values no dashboard
// knows about.
const (
	bootMetricsInstallOperation  = "install"
	bootMetricsAbUpdateOperation = "update1"

	// How long to wait for the host to be reachable. The host has already been
	// verified reachable by the preceding check, so this only absorbs a slow
	// reconnect rather than a full boot.
	bootMetricsConnectTimeout = time.Minute
)

// servicingTraceFile is where netlisten writes the trace-stream metrics for a
// servicing operation driven by the named test case. Boot metrics are appended
// to the same file, so the two must agree.
func servicingTraceFile(caseName string) string {
	return fmt.Sprintf("metrics-%s.jsonl", caseName)
}

// collectInstallBootMetrics records the boot timings for the clean install,
// folding the pipeline's post-install `storm-trident helper boot-metrics`
// invocation into the scenario.
func (s *TridentE2EScenario) collectInstallBootMetrics(tc storm.TestCase) error {
	return s.collectBootMetrics(tc, s.cleanInstallTraceFile(), bootMetricsInstallOperation)
}

// collectAbUpdateBootMetrics records the boot timings for the first A/B update,
// which is the only update legacy measured.
func (s *TridentE2EScenario) collectAbUpdateBootMetrics(updateCaseName string) storm.TestCaseFunction {
	return func(tc storm.TestCase) error {
		return s.collectBootMetrics(tc, servicingTraceFile(updateCaseName), bootMetricsAbUpdateOperation)
	}
}

// collectBootMetrics appends a boot timing record to the trace-stream file the
// servicing operation produced, so the timings are ingested alongside the
// metrics Trident itself reported for that operation.
//
// Every failure path skips rather than returning an error. These cases are
// pure telemetry, but storm treats a returned error as a bail condition and
// marks every remaining case "not run" — so propagating one would let a
// missing boot timing cancel all the product validation that follows, and
// report a config red that installed perfectly well.
func (s *TridentE2EScenario) collectBootMetrics(tc storm.TestCase, metricsFile string, operation string) error {
	connCtx, cancel := context.WithTimeout(tc.Context(), bootMetricsConnectTimeout)
	defer cancel()
	if err := s.populateSshClient(connCtx); err != nil {
		return skipBootMetrics(tc, err)
	}

	out, err := sshutils.RunCommand(s.sshClient, metrics.SystemdAnalyzeCommand)
	if err != nil {
		return skipBootMetrics(tc, fmt.Errorf("failed to read boot timings from the host: %w", err))
	}
	if out.Status != 0 {
		// systemd-analyze reports "Bootup is not yet finished" (and similar) on
		// stderr with a non-zero status and no usable stdout.
		return skipBootMetrics(tc, fmt.Errorf("systemd-analyze exited %d: %s", out.Status, strings.TrimSpace(out.Stderr)))
	}

	value, err := metrics.ParseBootMetric(operation, out.Stdout)
	if err != nil {
		return skipBootMetrics(tc, err)
	}

	if err := metrics.AppendBootMetrics(metricsFile, value); err != nil {
		return skipBootMetrics(tc, err)
	}

	logrus.Infof("Recorded %s boot timings in '%s': %+v", operation, metricsFile, value)
	return nil
}

// skipBootMetrics ends the case as skipped, logging why. The warning keeps a
// genuine collection failure visible in the logs even though it does not fail
// the run.
//
// tc.Skip does not return (it calls runtime.Goexit), but an error is returned
// anyway so callers can write `return skipBootMetrics(...)`: that keeps the
// control flow obvious at the call site and safe regardless of how Skip is
// implemented, rather than silently relying on it never returning.
func skipBootMetrics(tc storm.TestCase, err error) error {
	logrus.Warnf("Not recording boot metrics: %v", err)
	tc.Skip(fmt.Sprintf("boot metrics unavailable: %v", err))
	return nil
}
