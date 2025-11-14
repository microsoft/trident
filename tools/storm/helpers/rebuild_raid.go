package helpers

import (
	"fmt"
	"os"
	"os/exec"
	"path"
	"strings"
	"time"

	stormutils "tridenttools/storm/utils"
	stormenv "tridenttools/storm/utils/env"
	stormsshcheck "tridenttools/storm/utils/ssh/check"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	stormtrident "tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
	"gopkg.in/yaml.v2"
	"libvirt.org/go/libvirtxml"
)

type RebuildRaidHelper struct {
	args struct {
		stormsshconfig.SshCliSettings `embed:""`
		stormenv.EnvCliSettings       `embed:""`
		TridentConfigPath             string `help:"Path to the Trident configuration file." type:"string"`
		DeploymentEnvironment         string `help:"Deployment environment (e.g., bareMetal, virtualMachine)." type:"string" default:"virtualMachine"`
		VmName                        string `help:"Name of VM." type:"string" default:"virtdeploy-vm-0"`
		Disk                          string `help:"Disk to fail in RAID array." type:"string" default:"/dev/sdb"`
		SkipRebuildRaid               bool   `help:"Skip the rebuild RAID step." type:"bool" default:"false"`
	}

	failed bool
}

func (h RebuildRaidHelper) Name() string {
	return "rebuild-raid"
}

func (h *RebuildRaidHelper) Args() any {
	return &h.args
}

func (h *RebuildRaidHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("check-if-needed", h.checkIfNeeded)
	r.RegisterTestCase("stop-bm-raids", h.stopBaremetalRaids)
	r.RegisterTestCase("stop-vm-raids", h.stopVmRaids)
	r.RegisterTestCase("check-ssh", h.checkTridentServiceWithSsh)
	r.RegisterTestCase("rebuild-raid", h.rebuildRaid)
	return nil
}

func (h *RebuildRaidHelper) FailFromError(tc storm.TestCase, err error) {
	h.failed = true
	tc.FailFromError(err)
}

// Check if RAID testing is needed based on whether the Trident configuration
// is set up using RAID storage and not using usr-verity.
func (h *RebuildRaidHelper) checkIfNeeded(tc storm.TestCase) error {
	h.failed = false

	tridentConfigContents, err := os.ReadFile(h.args.TridentConfigPath)
	if err != nil {
		logrus.Tracef("Failed to read trident config file %s: %v", h.args.TridentConfigPath, err)
		h.FailFromError(tc, err)
		return err
	}
	tridentConfig := make(map[string]interface{})
	err = yaml.UnmarshalStrict(tridentConfigContents, &tridentConfig)
	if err != nil {
		logrus.Tracef("Failed to parse trident config file %s: %v", h.args.TridentConfigPath, err)
		h.FailFromError(tc, err)
		return err
	}

	raidExists := false
	usrVerity := false

	storage, ok := tridentConfig["storage"].(map[interface{}]interface{})
	if ok {
		raidExists = storage["raid"] != nil
		if verityList, ok := storage["verity"].([]interface{}); ok {
			if len(verityList) > 0 {
				usrVerity = verityList[0].(map[interface{}]interface{})["name"] == "usr"
			}
		}
	}

	// TODO (12277): Support for UKI + Rebuild
	if raidExists && !usrVerity {
		logrus.Infof("Trident config requires Rebuild testing")
	} else {
		logrus.Infof("Trident config does not require Rebuild testing")
		h.args.SkipRebuildRaid = true
	}
	return nil
}

// Get list of RAID arrays and their devices on the host and fail a device in each array.
func (h *RebuildRaidHelper) stopBaremetalRaids(tc storm.TestCase) error {
	if h.failed {
		tc.Skip("Previous step failed; skipping this test case.")
		return nil
	}
	if h.args.SkipRebuildRaid {
		tc.Skip("Skipping fail bare metal raids step")
		return nil
	}
	if h.args.DeploymentEnvironment != "bareMetal" {
		tc.Skip(fmt.Sprintf("Skipping fail bare metal raids step for deployment environment: %s", h.args.DeploymentEnvironment))
		return nil
	}
	logrus.Infof("Failing bare metal raids")

	// Set up SSH client
	var err error
	client, err := stormsshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		tc.Error(err)
	}
	defer client.Close()

	output, err := stormsshclient.CommandOutput(client, "sudo dd if=/dev/zero of=/dev/sdb bs=512 count=1")
	if err != nil {
		tc.Error(err)
	}
	logrus.Debugf("Output of zeroing /dev/sdb:\n%s", string(output))
	output, err = stormsshclient.CommandOutput(client, "echo 'label: gpt' | sudo sfdisk /dev/sdb --force")
	if err != nil {
		tc.Error(err)
	}
	logrus.Debugf("Output of partitioning /dev/sdb:\n%s", string(output))

	output, err = stormsshclient.CommandOutput(client, "sudo mdadm --detail --scan")
	if err != nil {
		tc.Error(err)
	}
	logrus.Debugf("Output of mdadm --detail --scan:\n%s", string(output))
	// Sample output:
	//  ARRAY /dev/md/esp-raid metadata=1.0 name=trident-mos-testimage:esp-raid
	//  UUID=42dd297c:7e0c5a24:6b792c94:238a99f5

	raidArrays := []string{}
	for _, line := range strings.Split(string(output), "\n") {
		if strings.HasPrefix(strings.TrimSpace(line), "ARRAY") {
			parts := strings.Fields(line)
			if len(parts) > 1 && parts[0] == "ARRAY" {
				raidArrays = append(raidArrays, parts[1])
			}
		}
	}
	raidDetails := make(map[string][]string)
	for _, raid := range raidArrays {
		arrayResult, err := stormsshclient.CommandOutput(client, "sudo mdadm --detail "+raid)
		if err != nil {
			tc.Error(err)
		}
		// Sample output:
		// /dev/md/esp-raid:
		//            Version : 1.0
		//      Creation Time : Thu Nov 14 18:17:50 2024
		//         Raid Level : raid1
		//         Array Size : 1048512 (1023.94 MiB 1073.68 MB)
		//      Used Dev Size : 1048512 (1023.94 MiB 1073.68 MB)
		//       Raid Devices : 2
		//      Total Devices : 2
		//        Persistence : Superblock is persistent

		//        Update Time : Thu Nov 14 18:18:49 2024
		//              State : clean
		//     Active Devices : 2
		//    Working Devices : 2
		//     Failed Devices : 0
		//      Spare Devices : 0

		// Consistency Policy : resync

		//               Name : trident-mos-testimage:esp-raid
		//               UUID : 6d52553e:ee0662a3:24761c4b:e3e6885b
		//             Events : 19

		//     Number   Major   Minor   RaidDevice State
		//        0       8        1        0      active sync   /dev/sda1
		//        1       8       17        1      active sync   /dev/sdb1

		details := strings.Split(string(arrayResult), "\n")
		// Extracting devices
		devices := []string{}
		devicesSection := false
		for _, line := range details {
			if strings.HasPrefix(strings.TrimSpace(line), "Number") {
				devicesSection = true
				continue
			}
			if devicesSection && strings.TrimSpace(line) != "" {
				parts := strings.Fields(line)
				// Ensure we have enough parts to avoid index errors
				if len(parts) >= 7 {
					devices = append(devices, parts[6])
				}
			}
		}
		raidDetails[raid] = devices
	}

	failRaidArray := func(raid string, device string) error {
		output, err := stormsshclient.CommandOutput(client, "sudo mdadm --fail "+raid+" "+device)
		if err != nil {
			return fmt.Errorf("failed to fail device %s in RAID array %s: %w\nOutput: %s", device, raid, err, string(output))
		}
		logrus.Infof("Device %s failed in RAID array %s", device, raid)

		return nil
	}

	if len(raidDetails) > 0 {
		for raid, devices := range raidDetails {
			for _, device := range devices {
				if strings.HasPrefix(device, h.args.Disk) {
					// fail the device in the RAID array
					err := failRaidArray(raid, device)
					if err != nil {
						tc.Error(err)
					}
				}
			}
		}
	} else {
		logrus.Infof("No RAID arrays found on the host.")
	}

	output, err = stormsshclient.CommandOutput(client, "sudo reboot")
	logrus.Tracef("Output of `sudo reboot` (%+v):\n%s", err, string(output))
	return nil
}

// Replace the test disk with a new disk
func (h *RebuildRaidHelper) stopVmRaids(tc storm.TestCase) error {
	if h.failed {
		tc.Skip("Previous step failed; skipping this test case.")
		return nil
	}
	if h.args.SkipRebuildRaid {
		tc.Skip("Skipping virtual machine shutdown step")
		return nil
	}
	if h.args.DeploymentEnvironment != "virtualMachine" {
		tc.Skip(fmt.Sprintf("Skipping shutdown VM step for deployment environment: %s", h.args.DeploymentEnvironment))
		return nil
	}
	logrus.Infof("Shutting down virtual machine %s", h.args.VmName)

	var err error
	client, err := stormsshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		tc.Error(err)
		return err
	}
	defer client.Close()

	logrus.Info("Efibootmgr entries in the VM.")
	output, err := stormsshclient.CommandOutput(client, "sudo efibootmgr")
	if err != nil {
		tc.Error(err)
		return err
	}
	logrus.Infof("Output of efibootmgr:\n%s", string(output))

	virshOutput, virshErr := exec.Command("sudo", "virsh", "shutdown", h.args.VmName).CombinedOutput()
	logrus.Tracef("virsh shutdown output: %s\n%v", string(virshOutput), virshErr)
	if virshErr != nil {
		tc.Error(virshErr)
		return virshErr
	}

	rmOutput, rmErr := exec.Command("sudo", "rm", "-f", fmt.Sprintf("/var/lib/libvirt/images/virtdeploy-pool/%s-1-volume.qcow2", h.args.VmName)).CombinedOutput()
	logrus.Tracef("rm volume output: %s\n%v", string(rmOutput), rmErr)
	if rmErr != nil {
		tc.Error(rmErr)
		return rmErr
	}

	createOutput, createErr := exec.Command("sudo", "qemu-img", "create", "-f", "qcow2", fmt.Sprintf("/var/lib/libvirt/images/virtdeploy-pool/%s-1-volume.qcow2", h.args.VmName), "16G").CombinedOutput()
	logrus.Tracef("qemu-img create output: %s\n%v", string(createOutput), createErr)
	if createErr != nil {
		tc.Error(createErr)
		return createErr
	}

	sleepTime := time.Duration(10) * time.Second

	// Check the state of the domain and run the loop
	domainShutdown := false
	domainStarted := false
	for i := 1; i <= 30; i++ {
		domstateOutput, domstateErr := exec.Command("sudo", "virsh", "domstate", h.args.VmName).CombinedOutput()
		if domstateErr != nil {
			tc.Error(domstateErr)
			return domstateErr
		}
		logrus.Infof("Domain state attempt %d: %s", i, strings.TrimSpace(string(domstateOutput)))

		if strings.TrimSpace(string(domstateOutput)) == "shut off" {
			domainShutdown = true
			logrus.Info("The domain is shut off. Starting the domain...")
			startOutput, startErr := exec.Command("sudo", "virsh", "start", h.args.VmName).CombinedOutput()
			logrus.Tracef("virsh start output: %s\n%v", string(startOutput), startErr)
			if startErr != nil {
				tc.Error(startErr)
				return startErr
			}

			domainStarted = true
			logrus.Info("The domain has been started.")
			break
		} else {
			logrus.Infof("The domain is still running. Waiting for %d seconds...", i*10)
			time.Sleep(sleepTime)
			sleepTime += 10 * time.Second
		}
	}

	if !domainShutdown {
		tc.Error(fmt.Errorf("the domain did not shut down after 30 attempts"))
		return nil
	}
	if !domainStarted {
		tc.Error(fmt.Errorf("the domain did not start after 30 attempts"))
		return nil
	}

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
		tc.Error(fmt.Errorf("failed to find VM serial log path"))
	}

	tempDir, err := os.MkdirTemp("", "rebuild-raid-*")
	if err != nil {
		return fmt.Errorf("failed to create temp dir: %w", err)
	}
	defer os.RemoveAll(tempDir)

	serialLog := "serial.log"
	outputPath := path.Join(tempDir, serialLog)
	err = stormutils.WaitForLoginMessageInSerialLog(vmSerialLog, true, 1, outputPath, time.Minute*5)
	tc.ArtifactBroker().PublishLogFile(serialLog, outputPath)
	if err != nil {
		tc.Error(err)
		return err
	}
	return nil
}

// Wait for machine to come back online and check Trident service status via SSH.
func (h *RebuildRaidHelper) checkTridentServiceWithSsh(tc storm.TestCase) error {
	if h.failed {
		tc.Skip("Previous step failed; skipping this test case.")
		return nil
	}
	if h.args.SkipRebuildRaid {
		tc.Skip("Skipping trident service check step")
		return nil
	}
	err := stormsshcheck.CheckTridentService(
		h.args.SshCliSettings,
		h.args.EnvCliSettings,
		true,
		h.args.TimeoutDuration(),
		tc,
	)
	if err != nil {
		logrus.Errorf("Trident service check via SSH failed: %s", err)
		h.FailFromError(tc, err)
	}
	return nil
}

// Checks if a file exists at the specified path on the host.
func (h *RebuildRaidHelper) checkFileExists(client *ssh.Client, filePath string) (bool, error) {
	clientSession, err := client.NewSession()
	if err != nil {
		return false, err
	}
	defer clientSession.Close()

	command := fmt.Sprintf("test -f %s", filePath)
	output, err := clientSession.CombinedOutput(command)
	logrus.Tracef("check file exists output: %s\n%v", string(output), err)
	if err != nil {
		return false, nil
	}
	return true, nil
}

// Runs "trident rebuild-raid" to trigger rebuilding RAID and checks if RAID was rebuilt successfully.
func (h *RebuildRaidHelper) tridentRebuildRaid(client *ssh.Client) error {
	output, err := stormtrident.InvokeTrident(h.args.Env, client, []string{}, "rebuild-raid -v trace")
	if err != nil {
		logrus.Errorf("Failed to invoke Trident: %v", err)
		return err
	}
	if err := output.Check(); err != nil {
		logrus.Errorf("Trident rebuild-raid stderr:\n%s", output.Stderr)
		return err
	}

	logrus.Info("Trident rebuild-raid succeeded")
	logrus.Tracef("Trident rebuild-raid output:\n%s\n%s", output.Stdout, output.Stderr)
	return nil
}

// Copy the Trident config to the host if it isn't already there.
func (h *RebuildRaidHelper) copyHostConfig(client *ssh.Client, tridentConfig string) error {
	fileExists, err := h.checkFileExists(client, tridentConfig)
	if err != nil {
		return err
	}
	if !fileExists {
		LOCAL_TRIDENT_CONFIG_PATH := "/etc/trident/config.yaml"
		logrus.Infof("File %s does not exist. Copying from %s", tridentConfig, LOCAL_TRIDENT_CONFIG_PATH)
		copyCommand := fmt.Sprintf("sudo cp %s %s", LOCAL_TRIDENT_CONFIG_PATH, tridentConfig)
		output, err := stormsshclient.CommandOutput(client, copyCommand)
		if err != nil {
			logrus.Errorf("Failed to copy Trident config to host: %s\n%s", err, string(output))
			// Maintaining previous behavior: error is ignored here
		}
	}

	catCommand := fmt.Sprintf("sudo cat %s", tridentConfig)
	tridentConfigOutput, err := stormsshclient.CommandOutput(client, catCommand)
	if err != nil {
		logrus.Errorf("Failed to read Trident config on host: %s\n%s", err, string(tridentConfigOutput))
		// Maintaining previous behavior: error is ignored here
	}

	logrus.Infof("Trident configuration:\n%s", string(tridentConfigOutput))
	return nil
}

// Connects to the host via SSH, copies the Trident config to the host, and runs Trident rebuild-raid.
func (h *RebuildRaidHelper) triggerRebuildRaid(tridentConfig string) error {
	client, err := stormsshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		return err
	}
	defer client.Close()

	// Copy the Trident config to the host
	err = h.copyHostConfig(client, tridentConfig)
	if err != nil {
		return err
	}

	// Re-build RAID and capture logs
	logrus.Info("Re-building RAID")
	err = h.tridentRebuildRaid(client)
	if err != nil {
		return err
	}
	return nil
}

func (h *RebuildRaidHelper) rebuildRaid(tc storm.TestCase) error {
	if h.failed {
		tc.Skip("Previous step failed; skipping this test case.")
		return nil
	}
	if h.args.SkipRebuildRaid {
		tc.Skip("Skipping rebuild RAID step")
		return nil
	}

	err := h.triggerRebuildRaid("/var/lib/trident/config.yaml")
	if err != nil {
		h.FailFromError(tc, err)
	}

	return nil
}
