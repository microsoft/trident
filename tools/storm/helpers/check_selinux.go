package helpers

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"regexp"
	"storm"
	"strconv"
	"strings"
	"time"
	"tridenttools/storm/utils"

	"github.com/sirupsen/logrus"
)

type CheckSelinuxHelper struct {
	args struct {
		utils.SshCliSettings `embed:""`
		utils.EnvCliSettings `embed:""`
		MetricsFile          string `required:"" help:"Metrics file." type:"path"`
		MetricsOperation     string `required:"" help:"Metrics operation."`
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
	if h.args.Env == utils.TridentEnvironmentNone {
		tc.Skip("No Trident environment specified")
	}

	logrus.Infof("Checking for SELinux violations with audit2allow")
	err := h.checkAudit2Allow()
	if err != nil {
		// Log this as a test failure
		tc.FailFromError(err)
	}

	return nil
}
			

func (h *CheckSelinuxHelper) checkAudit2Allow() error {
	client, err := utils.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		return err
	}

	audit2AllowResult, err := utils.RunCommand(client, "audit2allow -i /var/log/audit/audit.log")
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