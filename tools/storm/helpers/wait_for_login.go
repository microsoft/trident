package helpers

import (
	"fmt"
	"os"
	"os/exec"
	"time"

	stormutils "tridenttools/storm/utils"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"libvirt.org/go/libvirtxml"
)

type WaitForLoginHelper struct {
	args struct {
		VmName           string `help:"Name of VM." type:"string" default:"virtdeploy-vm-0"`
		ArtifactsFolder  string `help:"Folder to copy log files into." type:"string" default:""`
		TimeoutInSeconds int    `help:"Maximum time to wait for VM to reach login prompt." default:"1200"`
	}
}

func (h WaitForLoginHelper) Name() string {
	return "wait-for-login"
}

func (h *WaitForLoginHelper) Args() any {
	return &h.args
}

func (h *WaitForLoginHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("wait-for-login", h.waitForLogin)
	return nil
}

// Watch VM serial log and wait for login prompt to appear.
func (h *WaitForLoginHelper) waitForLogin(tc storm.TestCase) error {
	// Get the VM serial log file path
	dumpxmlOutput, dumpxmlErr := exec.Command("sudo", "virsh", "dumpxml", h.args.VmName).CombinedOutput()
	if dumpxmlErr != nil {
		tc.Error(dumpxmlErr)
		return dumpxmlErr
	}
	parsedDomainXml := &libvirtxml.Domain{}
	if err := parsedDomainXml.Unmarshal(string(dumpxmlOutput)); err != nil {
		return fmt.Errorf("failed to parse domain XML: %w", err)
	}
	var vmSerialLog string
	if parsedDomainXml.Devices != nil {
		for _, console := range parsedDomainXml.Devices.Consoles {
			if console.Log != nil {
				logrus.Infof("VM serial log file path: %s", console.Log.File)
				vmSerialLog = console.Log.File
				break
			}
		}
	}
	if vmSerialLog == "" {
		err := fmt.Errorf("failed to find VM serial log path")
		tc.FailFromError(err)
		return err
	}

	timeout := time.Duration(h.args.TimeoutInSeconds) * time.Second
	startTime := time.Now()
	for {
		if time.Since(startTime) >= timeout {
			err := fmt.Errorf("timed out waiting for serial log file to be created after %d seconds", h.args.TimeoutInSeconds)
			tc.FailFromError(err)
			return err
		}
		// Wait for the serial log to be created
		logrus.Infof("Waiting for VM serial log file to be created...")
		stat, err := os.Stat(vmSerialLog)
		if err == nil && stat.Size() > 0 {
			logrus.Infof("VM serial log file found: %s", vmSerialLog)
			break
		}
		time.Sleep(100 * time.Millisecond)
	}

	// Wait for login prompt in the serial log
	logrus.Infof("Waiting for login prompt in VM serial log...")

	err := stormutils.WaitForLoginMessageInSerialLog(vmSerialLog, true, 1, fmt.Sprintf("%s/serial.log", h.args.ArtifactsFolder), time.Minute*5)
	if err != nil {
		tc.FailFromError(err)
		return err
	}
	return nil
}
