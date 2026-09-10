package scenario

import (
	"context"
	"fmt"
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
func (s *TridentE2EScenario) collectBootMetrics(tc storm.TestCase, metricsFile string, operation string) error {
	connCtx, cancel := context.WithTimeout(tc.Context(), bootMetricsConnectTimeout)
	defer cancel()
	if err := s.populateSshClient(connCtx); err != nil {
		return err
	}

	out, err := sshutils.RunCommand(s.sshClient, metrics.SystemdAnalyzeCommand)
	if err != nil {
		return fmt.Errorf("failed to read boot timings from the host: %w", err)
	}

	value, err := metrics.ParseBootMetric(operation, out.Stdout)
	if err != nil {
		return err
	}

	if err := metrics.AppendBootMetrics(metricsFile, value); err != nil {
		return err
	}

	logrus.Infof("Recorded %s boot timings in '%s': %+v", operation, metricsFile, value)
	return nil
}
