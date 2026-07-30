package validate

import (
	"bufio"
	"encoding/json"
	"os"
	"strings"

	"golang.org/x/crypto/ssh"

	"tridenttools/storm/utils/sshutils"
)

// Diagnostic metric identifiers, mirroring the defaults of the legacy
// check-tracing / check-selinux helpers. These validations only apply to the
// host runtime (SELinux enforcement and Trident's journald tracing are host
// concerns), and are scoped to the clean install.
const (
	// tridentTracingSyslogIdentifier is the journald syslog identifier Trident
	// tags its tracing metrics with.
	tridentTracingSyslogIdentifier = "trident-tracing"
	// tridentStartMetric is a metric emitted by Trident's commit that must
	// appear in the journald tracing stream.
	tridentStartMetric = "trident_start"
	// hostConfigFeatureUsageMetric is a metric collected throughout servicing
	// that must appear in the captured trace-stream file.
	hostConfigFeatureUsageMetric = "host_config_feature_usage"

	auditLogPath = "/var/log/audit/audit.log"
)

// ValidateSelinuxDenials ports the check-selinux helper. It runs `audit2allow`
// against the host's audit log and surfaces any SELinux denials. Matching the
// legacy helper, it fails only when the command cannot be run (not when
// denials are present) — the denials are logged for human inspection.
func ValidateSelinuxDenials(sa *SoftAsserter, client *ssh.Client) {
	out, err := sshutils.RunCommand(client, "sudo audit2allow -i "+auditLogPath)
	if err != nil {
		sa.Fail("selinux/audit2allow", err)
		return
	}
	if strings.TrimSpace(out.Stdout) != "" {
		sa.Passf("selinux/audit2allow", "audit2allow reported potential denials:\n%s", out.Stdout)
	} else {
		sa.Pass("selinux/audit2allow")
	}
}

// ValidateJournaldTracing ports check-tracing's check-journald. It confirms the
// Trident tracing metric emitted by commit (trident_start) is present in the
// host's journald logs under the trident-tracing syslog identifier.
func ValidateJournaldTracing(sa *SoftAsserter, client *ssh.Client) {
	out, err := sshutils.RunCommand(client, "sudo journalctl -t "+tridentTracingSyslogIdentifier+" -o json")
	if err != nil {
		sa.Fail("tracing/journald", err)
		return
	}

	scanner := bufio.NewScanner(strings.NewReader(out.Stdout))
	scanner.Buffer(make([]byte, 0, 1024*1024), 8*1024*1024)
	for scanner.Scan() {
		var entry map[string]interface{}
		if err := json.Unmarshal(scanner.Bytes(), &entry); err != nil {
			continue
		}
		if entry["F_METRIC_NAME"] == tridentStartMetric {
			sa.Pass("tracing/journald")
			return
		}
	}
	sa.Failf("tracing/journald", "metric %q not found in journald logs for identifier %q",
		tridentStartMetric, tridentTracingSyslogIdentifier)
}

// ValidateTraceFileMetric ports check-tracing's check-trace-file. It confirms
// the feature-usage metric collected during servicing is present in the local
// trace-stream file that netlisten captured for the install. An empty path
// (no trace file configured) is skipped rather than failed, matching the
// helper.
func ValidateTraceFileMetric(sa *SoftAsserter, traceFilePath string) {
	if traceFilePath == "" {
		return
	}

	f, err := os.Open(traceFilePath)
	if err != nil {
		sa.Fail("tracing/trace-file", err)
		return
	}
	defer f.Close()

	dec := json.NewDecoder(f)
	for dec.More() {
		var entry map[string]interface{}
		if err := dec.Decode(&entry); err != nil {
			sa.Fail("tracing/trace-file", err)
			return
		}
		if entry["metric_name"] == hostConfigFeatureUsageMetric {
			sa.Pass("tracing/trace-file")
			return
		}
	}
	sa.Failf("tracing/trace-file", "metric %q not found in trace file %q",
		hostConfigFeatureUsageMetric, traceFilePath)
}
