package validate

import (
	"bufio"
	"encoding/json"
	"fmt"
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

// ValidateSelinuxDenials reports SELinux denials recorded in the host's audit
// log. Matching the legacy helper, denials do not fail the check -- they are
// surfaced for human inspection.
//
// Denials are counted by scanning the audit log directly rather than by
// trusting audit2allow, because audit2allow cannot report them on either image
// variant: it is absent from some (it ships in setools-console, which is
// installed in the installer image but not in every deployed test image), and
// on the others it exits 1 with "You must specify the -p option with the path
// to the policy file" because the compiled policy it needs to render rules is
// not in the image. The legacy helper ignored the exit status, so both cases
// were recorded as "no SELinux violations found".
// audit2allow is still used, best effort, to render the matching policy rules.
func ValidateSelinuxDenials(sa *SoftAsserter, client *ssh.Client) {
	// grep exits 0 when it matches, 1 when it does not, and >1 on a real error
	// such as the audit log being absent.
	count, err := sshutils.RunCommand(client,
		fmt.Sprintf("sudo grep -c -E 'avc:[[:space:]]+denied' %s", sshutils.ShellQuote(auditLogPath)))
	if err != nil {
		sa.Fail("selinux/denials", err)
		return
	}
	if count.Status > 1 {
		// An absent or unreadable audit log is an image gap, not a regression
		// this suite should fail on -- but it must not look like a clean scan.
		sa.Passf("selinux/denials", "not verified: could not read %s: %s",
			auditLogPath, strings.TrimSpace(count.Stderr))
		return
	}
	if count.Status == 1 {
		sa.Pass("selinux/denials")
		return
	}

	sa.Passf("selinux/denials", "%s reports %s SELinux denial(s)%s",
		auditLogPath, strings.TrimSpace(count.Stdout), audit2allowDetail(client))
}

// audit2allowDetail renders the denials as policy rules when audit2allow is
// usable, and explains itself when it is not. It never fails the check: the
// denial count above is the authoritative signal.
func audit2allowDetail(client *ssh.Client) string {
	out, err := sshutils.RunCommand(client, "sudo audit2allow -i "+sshutils.ShellQuote(auditLogPath))
	switch {
	case err != nil:
		return fmt.Sprintf("; audit2allow could not be run: %v", err)
	case out.Status != 0:
		return fmt.Sprintf("; audit2allow exited %d: %s", out.Status, strings.TrimSpace(out.Stderr))
	case strings.TrimSpace(out.Stdout) == "":
		return ""
	default:
		return ":\n" + out.Stdout
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
	// A non-zero journalctl exit would otherwise be reported as "metric not
	// found", pointing at Trident instead of the failed query.
	if out.Status != 0 {
		sa.Failf("tracing/journald", "journalctl exited %d: %s", out.Status, strings.TrimSpace(out.Stderr))
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
