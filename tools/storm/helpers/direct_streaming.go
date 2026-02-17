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

	// Create Host Configuration with image information for direct streaming test
	hostConfig, err := h.createTempHostConfig()
	if err != nil {
		return fmt.Errorf("failed to create host config: %w", err)
	}
	defer func() {
		tc.ArtifactBroker().PublishLogFile("hostconfig.yaml", hostConfig)
		os.Remove(hostConfig)
	}()

	// Create adjusted netlaunch config with direct streaming image
	netlaunchConfig, err := h.createNetlaunchConfig(hostConfig)
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
	// For local runs, if serial log already exists, delete it.
	if _, err := os.Stat(vmSerialLog); err == nil {
		logrus.Infof("VM serial log file (%s) already exists, delete it.", vmSerialLog)
		if removeErr := os.Remove(vmSerialLog); removeErr != nil {
			return fmt.Errorf("failed to remove existing VM serial log file: %w", removeErr)
		}
	}
	defer tc.ArtifactBroker().PublishLogFile("vm-serial.log", vmSerialLog)

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

func (h *DirectStreamingHelper) createTempHostConfig() (string, error) {
	hash := h.args.CosiMetadataSha384
	if hash != "ignored" && !strings.HasPrefix(hash, "sha384:") {
		hash = "sha384:" + hash
	}
	hostConfig := map[string]any{
		"image": map[string]any{
			"url":    fmt.Sprintf("http://NETLAUNCH_HOST_ADDRESS/files/%s", filepath.Base(h.args.CosiFile)),
			"sha384": hash,
		},
	}
	hostConfigBytes, err := yaml.Marshal(hostConfig)
	if err != nil {
		return "", fmt.Errorf("failed to marshal host config to YAML: %w", err)
	}
	hostConfigFile, err := os.CreateTemp("", "host-config-*.yaml")
	if err != nil {
		return "", fmt.Errorf("failed to create temp file for host config: %w", err)
	}
	defer hostConfigFile.Close()
	_, err = hostConfigFile.Write(hostConfigBytes)
	if err != nil {
		return "", fmt.Errorf("failed to write host config to file: %w", err)
	}
	return hostConfigFile.Name(), nil
}

func (h *DirectStreamingHelper) createNetlaunchConfig(hostConfig string) (*netlaunch.NetLaunchConfig, error) {
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

	netlaunchConfig.HostConfigFile = hostConfig
	netlaunchConfig.ListenPort = h.args.Port
	netlaunchConfig.ServeDirectory = filepath.Dir(h.args.CosiFile)
	netlaunchConfig.IsoPath = h.args.IsoFile
	netlaunchConfig.EnableSecureBoot = h.args.EnableSecureBoot
	netlaunchConfig.LogstreamFile = "/tmp/netlaunch.log"
	netlaunchConfig.TracestreamFile = "/tmp/netlaunch-trace.jsonl"
	netlaunchConfig.Rcp = &netlaunch.RcpConfiguration{
		GrpcMode: false,
		Scenario: netlaunch.RcpScenarioStreamImage,
	}

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
