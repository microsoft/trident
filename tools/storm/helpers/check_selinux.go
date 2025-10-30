package helpers

import (
	"os"
	"strings"
	"time"
	"tridenttools/storm/utils/env"
	stormenv "tridenttools/storm/utils/env"
	"tridenttools/storm/utils/retry"
	sshclient "tridenttools/storm/utils/ssh/client"
	sshconfig "tridenttools/storm/utils/ssh/config"

	"github.com/microsoft/storm"

	"github.com/sirupsen/logrus"
)

type CheckSelinuxHelper struct {
	args struct {
		sshconfig.SshCliSettings `embed:""`
		env.EnvCliSettings       `embed:""`
		AuditFile                string `required:"" help:"Audit logs file." type:"path"`
	}
}

func (h CheckSelinuxHelper) Name() string {
	return "check-selinux"
}

func (h *CheckSelinuxHelper) Args() any {
	return &h.args
}

func (h *CheckSelinuxHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("check-selinux-denials", h.checkSelinuxDenials)
	return nil
}

func (h *CheckSelinuxHelper) checkSelinuxDenials(tc storm.TestCase) error {
	if h.args.Env == stormenv.TridentEnvironmentNone {
		tc.Skip("No Trident environment specified")
	}

	logrus.Infof("== Checking for SELinux violations with audit2allow ==")
	err := h.checkAudit2Allow()
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}

	logrus.Infof("== VM audit logs ==")
	auditLogsOutput, err := retry.Retry(
		time.Second*time.Duration(h.args.Timeout),
		time.Second*5,
		func(attempt int) (*string, error) {
			client, err := sshclient.OpenSshClient(h.args.SshCliSettings)
			if err != nil {
				return nil, err
			}
			defer client.Close()

			auditLogsResult, err := sshclient.RunCommand(client, "sudo cat /var/log/audit/audit.log")
			if err != nil {
				return nil, err
			}

			return &auditLogsResult.Stdout, nil
		},
	)
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}

	// Display the audit logs
	if auditLogsOutput != nil && strings.TrimSpace(*auditLogsOutput) != "" {
		logrus.Infof("\n%s", *auditLogsOutput)

		// Save to audit file if specified
		if h.args.AuditFile != "" {
			err = os.WriteFile(h.args.AuditFile, []byte(*auditLogsOutput), 0644)
			if err != nil {
				logrus.Errorf("Failed to write audit logs to %s: %v", h.args.AuditFile, err)
			}
		}
	} else {
		logrus.Infof("Audit log is empty")
	}

	return nil
}

func (h *CheckSelinuxHelper) checkAudit2Allow() error {
	client, err := sshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		return err
	}

	audit2AllowResult, err := sshclient.RunCommand(client, "sudo audit2allow -i /var/log/audit/audit.log")
	if err != nil {
		logrus.Errorf("Failed to run audit2allow: %v", err)
		return err
	}

	// Print output from audit2allow
	if strings.TrimSpace(audit2AllowResult.Stdout) != "" {
		logrus.Infof("SELinux audit2allow output:\n%s", audit2AllowResult.Stdout)
	} else {
		logrus.Infof("audit2allow had no output (no SELinux violations found)")
	}

	return nil
}
