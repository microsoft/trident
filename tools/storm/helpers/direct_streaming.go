package helpers

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
	"tridenttools/pkg/netlaunch"
	stormutils "tridenttools/storm/utils"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v3"
	"libvirt.org/go/libvirtxml"
)

type DirectStreamingHelper struct {
	args struct {
		VmName              string `help:"Name of VM." type:"string" default:"virtdeploy-vm-0"`
		CosiFile            string `required:"" help:"Path to the COSI file to read." type:"existingfile"`
		CosiMetadataSha384  string `name:"cosi-metadata-sha384" help:"Expected sha384 hash of the COSI metadata." default:"ignored"`
		IsoFile             string `required:"" help:"Path to the ISO file to read." type:"existingfile"`
		Port                uint16 `required:"" help:"Port on which the netlaunch server is running."`
		NetlaunchConfigFile string `required:"" help:"Path to the netlaunch config file." type:"existingfile"`
		EnableSecureBoot    bool   `help:"Whether to enable secure boot for the netlaunch server." default:"false"`
		TimeoutInSeconds    uint   `help:"Timeout in seconds to wait for login message in VM serial log." default:"1200"`
	}
}

func (h DirectStreamingHelper) Name() string {
	return "direct-streaming"
}

func (h *DirectStreamingHelper) Args() any {
	return &h.args
}

func (h *DirectStreamingHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("direct-streaming", h.directStreaming)
	return nil
}

func (h *DirectStreamingHelper) directStreaming(tc storm.TestCase) error {
	startTime := time.Now()
	// Create adjusted netlaunch config with direct streaming image
	netlaunchConfig, err := h.createNetlaunchConfig()
	if err != nil {
		return fmt.Errorf("failed to create netlaunch config: %w", err)
	}
	defer tc.ArtifactBroker().PublishLogFile("netlaunch.log", "/tmp/netlaunch.log")
	defer tc.ArtifactBroker().PublishLogFile("netlaunch-trace.jsonl", "/tmp/netlaunch-trace.jsonl")

	// Get the VM serial log file path
	vmSerialLog, err := h.findVmSerialLogFile()
	if err != nil {
		tc.FailFromError(err)
		return err
	}
	defer tc.ArtifactBroker().PublishLogFile("vm-serial.log", vmSerialLog)

	if _, err := os.Stat(vmSerialLog); err == nil {
		logrus.Infof("VM serial log file (%s) already exists, delete it.", vmSerialLog)
		if removeErr := os.Remove(vmSerialLog); removeErr != nil {
			return fmt.Errorf("failed to remove existing VM serial log file: %w", removeErr)
		}
	}

	// Start netlaunch in background because the VM will not connect back to
	// netlaunch and we need the file server to continue running until the image
	// has been pulled and deployed. Netlaunch is intended to run until the test
	// ends.
	netlaunchContext := context.Background()
	go func() {
		logrus.Info("Starting netlaunch...")
		netlaunchErr := netlaunch.RunNetlaunch(netlaunchContext, netlaunchConfig)
		logrus.Info("netlaunch stopped.")
		if netlaunchErr != nil {
			tc.FailFromError(netlaunchErr)
		}
	}()

	time.Sleep(10 * time.Second) // Give netlaunch some time to start

	// Wait for login message in serial log
	remainingTimeout := (time.Duration(h.args.TimeoutInSeconds) * time.Second) - time.Since(startTime)
	err = stormutils.WaitForLoginMessageInSerialLog(vmSerialLog, true, 1, "/tmp/serial.log", remainingTimeout)
	tc.ArtifactBroker().PublishLogFile("serial.log", "/tmp/serial.log")
	if err != nil {
		logrus.Errorf("Failed to find login message in VM serial log: %v", err)
		tc.FailFromError(err)
	}

	logrus.Info("Direct streaming test completed successfully")
	return nil
}

func (h *DirectStreamingHelper) createNetlaunchConfig() (*netlaunch.NetLaunchConfig, error) {
	// Create adjusted netlaunch config with direct streaming image
	netlaunchConfigBytes, err := os.ReadFile(h.args.NetlaunchConfigFile)
	if err != nil {
		return nil, fmt.Errorf("failed to read netlaunch config file: %w", err)
	}

	var netlaunchConfig netlaunch.NetLaunchConfig
	err = yaml.Unmarshal(netlaunchConfigBytes, &netlaunchConfig)
	if err != nil {
		return nil, fmt.Errorf("failed to parse netlaunch config file: %w", err)
	}

	if netlaunchConfig.Iso.DirectStreaming == nil {
		netlaunchConfig.Iso.DirectStreaming = &netlaunch.DirectStreaming{}
	}
	netlaunchConfig.Iso.DirectStreaming.Image = filepath.Base(h.args.CosiFile)
	hash := h.args.CosiMetadataSha384
	if hash != "ignored" && !strings.HasPrefix(hash, "sha384:") {
		hash = "sha384:" + hash
	}
	netlaunchConfig.Iso.DirectStreaming.Hash = hash
	netlaunchConfig.ListenPort = h.args.Port
	netlaunchConfig.ServeDirectory = filepath.Dir(h.args.CosiFile)
	netlaunchConfig.IsoPath = h.args.IsoFile
	netlaunchConfig.EnableSecureBoot = h.args.EnableSecureBoot
	netlaunchConfig.LogstreamFile = "/tmp/netlaunch.log"
	netlaunchConfig.TracestreamFile = "/tmp/netlaunch-trace.jsonl"

	return &netlaunchConfig, nil
}

func (h *DirectStreamingHelper) findVmSerialLogFile() (string, error) {
	dumpxmlOutput, dumpxmlErr := exec.Command("sudo", "virsh", "dumpxml", h.args.VmName).CombinedOutput()
	if dumpxmlErr != nil {
		return "", fmt.Errorf("failed to dump VM XML: %w", dumpxmlErr)
	}
	parsedDomainXml := &libvirtxml.Domain{}
	if err := parsedDomainXml.Unmarshal(string(dumpxmlOutput)); err != nil {
		return "", fmt.Errorf("failed to parse domain XML: %w", err)
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
		return "", fmt.Errorf("failed to find VM serial log path")
	}
	return vmSerialLog, nil
}
