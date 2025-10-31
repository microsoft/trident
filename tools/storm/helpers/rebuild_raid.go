package helpers

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"
	"tridenttools/storm/utils"
	"tridenttools/storm/utils/env"
	check "tridenttools/storm/utils/ssh/check"
	sshclient "tridenttools/storm/utils/ssh/client"
	sshconfig "tridenttools/storm/utils/ssh/config"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
	"gopkg.in/yaml.v2"
	"libvirt.org/go/libvirtxml"
)

type RebuildRaidHelper struct {
	args struct {
		sshconfig.SshCliSettings `embed:""`
		env.EnvCliSettings       `embed:""`
		TridentConfigPath        string `help:"Path to the Trident configuration file." type:"string"`
		DeploymentEnvironment    string `help:"Deployment environment (e.g., bareMetal, virtualMachine)." type:"string" default:"virtualMachine"`
		VmName                   string `help:"Name of VM." type:"string" default:"virtdeploy-vm-0"`
		Disk                     string `help:"Disk to fail in RAID array." type:"string" default:"/dev/sdb"`
		SkipRebuildRaid          bool   `help:"Skip the rebuild RAID step." type:"bool" default:"false"`
		ArtifactsFolder          string `help:"Folder to copy log files into." type:"string" default:""`
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
	r.RegisterTestCase("fail-bm-raids", h.failBaremetalRaids)
	r.RegisterTestCase("shutdown-vm", h.shutdownVirtualMachine)
	r.RegisterTestCase("check-ssh", h.checkTridentServiceWithSsh)
	r.RegisterTestCase("rebuild-raid", h.rebuildRaid)
	return nil
}

func (h *RebuildRaidHelper) FailFromError(tc storm.TestCase, err error) {
	h.failed = true
	tc.FailFromError(err)
}

func (h *RebuildRaidHelper) checkIfNeeded(tc storm.TestCase) error {
	h.failed = false

	tridentConfigContents, err := os.ReadFile(h.args.TridentConfigPath)
	if err != nil {
		logrus.Tracef("Failed to read trident config file %s: %v", h.args.TridentConfigPath, err)
		h.FailFromError(tc, err)
	}
	tridentConfig := make(map[string]interface{})
	err = yaml.UnmarshalStrict(tridentConfigContents, &tridentConfig)
	if err != nil {
		logrus.Tracef("Failed to parse trident config file %s: %v", h.args.TridentConfigPath, err)
		h.FailFromError(tc, err)
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

func (h *RebuildRaidHelper) failBaremetalRaids(tc storm.TestCase) error {
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

	// # Set up SSH client
	// connection = create_ssh_connection(ip_address, user_name, keys_file_path)
	var err error
	client, err := sshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		tc.Error(err)
	}

	session, err := client.NewSession()
	if err != nil {
		tc.Error(err)
	}
	defer session.Close()

	// ssh -o StrictHostKeyChecking=no -i ${{ parameters.sshKeyPath }} ${{ parameters.userName }}@${{ parameters.hostIp }} "sudo dd if=/dev/zero of=/dev/sdb bs=512 count=1"
	output, err := session.CombinedOutput("sudo dd if=/dev/zero of=/dev/sdb bs=512 count=1")
	if err != nil {
		tc.Error(err)
	}
	logrus.Debugf("Output of zeroing /dev/sdb:\n%s", string(output))
	// ssh -o StrictHostKeyChecking=no -i ${{ parameters.sshKeyPath }} ${{ parameters.userName }}@${{ parameters.hostIp }} "echo 'label: gpt' | sudo sfdisk /dev/sdb --force"
	output, err = session.CombinedOutput("echo 'label: gpt' | sudo sfdisk /dev/sdb --force")
	if err != nil {
		tc.Error(err)
	}
	logrus.Debugf("Output of partitioning /dev/sdb:\n%s", string(output))

	// Fail the RAID devices
	// python3 $(Build.SourcesDirectory)/tests/e2e_tests/helpers/fail_raid_devices.py \
	// 	--ip-address ${{ parameters.hostIp }} \
	// 	--user-name ${{ parameters.userName }} \
	// 	--keys-file-path ${{ parameters.sshKeyPath }}
	// def get_raid_arrays(connection):
	// 	"""
	// 	Get the list of RAID arrays and their devices on the host.
	// 	"""
	// 	try:
	// 		# Getting the list of RAID arrays
	// 		result = run_ssh_command(
	// 			connection,
	// 			"mdadm --detail --scan",
	// 			use_sudo=True,
	// 		)
	output, err = session.CombinedOutput("sudo mdadm --detail --scan")
	if err != nil {
		tc.Error(err)
	}
	logrus.Debugf("Output of mdadm --detail --scan:\n%s", string(output))
	// 		# Sample output:
	// 		#  ARRAY /dev/md/esp-raid metadata=1.0 name=trident-mos-testimage:esp-raid
	// 		#  UUID=42dd297c:7e0c5a24:6b792c94:238a99f5

	// 		raid_arrays = []
	raidArrays := []string{}
	// 		for line in result.splitlines():
	for _, line := range strings.Split(string(output), "\n") {
		// 			if line.strip().startswith("ARRAY"):
		if strings.HasPrefix(strings.TrimSpace(line), "ARRAY") {
			// 			parts = line.split()
			parts := strings.Fields(line)
			// 			if len(parts) > 1 and parts[0] == "ARRAY":
			if len(parts) > 1 && parts[0] == "ARRAY" {
				// 				raid_arrays.append(parts[1])
				raidArrays = append(raidArrays, parts[1])
			}
			// 			if len(parts) > 1 and parts[0] == "ARRAY":
			if len(parts) > 1 && parts[0] == "ARRAY" {
				// 				raid_arrays.append(parts[1])
				raidArrays = append(raidArrays, parts[1])
			}
		}
	}
	// 		raid_details = {}
	raidDetails := make(map[string][]string)
	// 		for raid in raid_arrays:
	for _, raid := range raidArrays {
		// 			# Getting detailed information for each RAID array
		// 			array_result = run_ssh_command(
		// 				connection,
		// 				f"mdadm --detail {raid}",
		// 				use_sudo=True,
		// 			)
		arrayResult, err := session.CombinedOutput(
			"sudo mdadm --detail " + raid,
		)
		if err != nil {
			tc.Error(err)
		}
		// 			# Sample output:

		// 			# /dev/md/esp-raid:
		// 			#            Version : 1.0
		// 			#      Creation Time : Thu Nov 14 18:17:50 2024
		// 			#         Raid Level : raid1
		// 			#         Array Size : 1048512 (1023.94 MiB 1073.68 MB)
		// 			#      Used Dev Size : 1048512 (1023.94 MiB 1073.68 MB)
		// 			#       Raid Devices : 2
		// 			#      Total Devices : 2
		// 			#        Persistence : Superblock is persistent

		// 			#        Update Time : Thu Nov 14 18:18:49 2024
		// 			#              State : clean
		// 			#     Active Devices : 2
		// 			#    Working Devices : 2
		// 			#     Failed Devices : 0
		// 			#      Spare Devices : 0

		// 			# Consistency Policy : resync

		// 			#               Name : trident-mos-testimage:esp-raid
		// 			#               UUID : 6d52553e:ee0662a3:24761c4b:e3e6885b
		// 			#             Events : 19

		// 			#     Number   Major   Minor   RaidDevice State
		// 			#        0       8        1        0      active sync   /dev/sda1
		// 			#        1       8       17        1      active sync   /dev/sdb1

		// 			details = array_result.splitlines()
		details := strings.Split(string(arrayResult), "\n")
		// 			# Extracting devices
		// 			devices = []
		devices := []string{}
		// 			devices_section = False
		devicesSection := false
		// 			for line in details:
		for _, line := range details {
			// 				if line.strip().startswith("Number"):
			if strings.HasPrefix(strings.TrimSpace(line), "Number") {
				// 					devices_section = True
				devicesSection = true
				// 					continue
				continue
			}
			// 				if devices_section and line.strip():
			if devicesSection && strings.TrimSpace(line) != "" {
				// 					parts = line.split()
				parts := strings.Fields(line)
				// 					if (
				// 						len(parts) >= 7
				// 					):  # Ensure we have enough parts to avoid index errors
				if len(parts) >= 7 {
					// 						devices.append(parts[6])
					devices = append(devices, parts[6])
				}
			}
		}
		// 			raid_details[raid] = devices
		raidDetails[raid] = devices
	}
	// 		return raid_details

	// 	except Exception as e:
	// 		raise Exception(f"Error getting RAID arrays: {e}")

	// def fail_raid_array(connection, raid, device):
	failRaidArray := func(raid string, device string) error {
		// 	"""
		// 	Fail a device in a RAID array.
		// 	"""
		// 	try:
		// 		run_ssh_command(
		// 			connection,
		// 			f"mdadm --fail {raid} {device}",
		// 			use_sudo=True,
		// 		)
		output, err := session.CombinedOutput(
			"sudo mdadm --fail " + raid + " " + device,
		)
		if err != nil {
			return fmt.Errorf("failed to fail device %s in RAID array %s: %w\nOutput: %s", device, raid, err, string(output))
		}
		// 		print(f"Device {device} failed in RAID array {raid}")
		logrus.Infof("Device %s failed in RAID array %s", device, raid)

		// 	except Exception as e:
		// 		raise Exception(f"Error failing RAID array {raid}: {e}")
		return nil
	}
	// raid_arrays = get_raid_arrays(connection)
	// if raid_arrays:
	if len(raidDetails) > 0 {
		// 	for raid, devices in raid_arrays.items():
		for raid, devices := range raidDetails {
			// 		for device in devices:
			for _, device := range devices {
				// 			if device.startswith(disk):
				if strings.HasPrefix(device, h.args.Disk) {
					// 				# fail the device in the RAID array
					// 				fail_raid_array(connection, raid, device)
					err := failRaidArray(raid, device)
					if err != nil {
						tc.Error(err)
					}
				}
			}
		}
		// else:
	} else {
		// 	print("No RAID arrays found on the host.")
		logrus.Infof("No RAID arrays found on the host.")
	}

	// ssh -o StrictHostKeyChecking=no -i ${{ parameters.sshKeyPath }} ${{ parameters.userName }}@${{ parameters.hostIp }} "sudo reboot"
	output, err = session.CombinedOutput("sudo reboot")
	logrus.Tracef("Output of `sudo reboot` (%+v):\n%s", err, string(output))
	return nil
}

func (h *RebuildRaidHelper) shutdownVirtualMachine(tc storm.TestCase) error {
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
	client, err := sshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		tc.Error(err)
		return err
	}

	session, err := client.NewSession()
	if err != nil {
		tc.Error(err)
		return err
	}
	defer session.Close()

	//   echo "Efibootmgr entries in the VM."
	logrus.Info("Efibootmgr entries in the VM.")
	//   ssh -o StrictHostKeyChecking=no -i ${{ parameters.sshKeyPath }} ${{ parameters.userName }}@${{ parameters.hostIp }} "sudo efibootmgr"
	output, err := session.CombinedOutput("sudo efibootmgr")
	if err != nil {
		tc.Error(err)
		return err
	}
	logrus.Infof("Output of efibootmgr:\n%s", string(output))

	//   sudo virsh shutdown virtdeploy-vm-0
	virshOutput, virshErr := exec.Command("sudo", "virsh", "shutdown", h.args.VmName).CombinedOutput()
	logrus.Tracef("virsh shutdown output: %s\n%v", string(virshOutput), virshErr)
	if virshErr != nil {
		tc.Error(virshErr)
		return err
	}

	//   sudo rm -f /var/lib/libvirt/images/virtdeploy-pool/virtdeploy-vm-0-1-volume.qcow2
	rmOutput, rmErr := exec.Command("sudo", "rm", "-f", fmt.Sprintf("/var/lib/libvirt/images/virtdeploy-pool/%s-1-volume.qcow2", h.args.VmName)).CombinedOutput()
	logrus.Tracef("rm volume output: %s\n%v", string(rmOutput), rmErr)
	if rmErr != nil {
		tc.Error(rmErr)
		return err
	}
	//   sudo qemu-img create -f qcow2 /var/lib/libvirt/images/virtdeploy-pool/virtdeploy-vm-0-1-volume.qcow2 16G
	createOutput, createErr := exec.Command("sudo", "qemu-img", "create", "-f", "qcow2", fmt.Sprintf("/var/lib/libvirt/images/virtdeploy-pool/%s-1-volume.qcow2", h.args.VmName), "16G").CombinedOutput()
	logrus.Tracef("qemu-img create output: %s\n%v", string(createOutput), createErr)
	if createErr != nil {
		tc.Error(createErr)
		return err
	}

	//   # Name of the domain
	//   DOMAIN_NAME="virtdeploy-vm-0"

	//   # Initial sleep time
	//   sleep_time=10
	sleepTime := time.Duration(10) * time.Second

	//   # Check the state of the domain and run the loop
	//   for (( i=1; i<=30; i++ )); do
	domainShutdown := false
	domainStarted := false
	for i := 1; i <= 30; i++ {
		//       domain_state=$(sudo virsh domstate $DOMAIN_NAME)
		domstateOutput, domstateErr := exec.Command("sudo", "virsh", "domstate", h.args.VmName).CombinedOutput()
		if domstateErr != nil {
			tc.Error(domstateErr)
			return err
		}
		logrus.Infof("Domain state attempt %d: %s", i, strings.TrimSpace(string(domstateOutput)))

		//       if [[ $domain_state == "shut off" ]]; then
		if strings.TrimSpace(string(domstateOutput)) == "shut off" {
			//           echo "The domain is shut off. Starting the domain..."
			domainShutdown = true
			logrus.Info("The domain is shut off. Starting the domain...")
			//           sudo virsh start $DOMAIN_NAME
			startOutput, startErr := exec.Command("sudo", "virsh", "start", h.args.VmName).CombinedOutput()
			logrus.Tracef("virsh start output: %s\n%v", string(startOutput), startErr)
			if startErr != nil {
				tc.Error(startErr)
				return startErr
			}
			//           echo "The domain has been started."
			domainStarted = true
			logrus.Info("The domain has been started.")
			//           exit 0
			break
			//       else
		} else {
			//           echo "The domain is still running. Waiting for $sleep_time seconds..."
			logrus.Infof("The domain is still running. Waiting for %d seconds...", i*10)
			//           sleep $sleep_time
			time.Sleep(sleepTime)
			//           sleep_time=$((sleep_time + 10))
			sleepTime += 10 * time.Second
			//       fi
		}
		//   done
	}

	//   echo "The domain did not shut down after 30 attempts."
	if !domainShutdown {
		tc.Error(fmt.Errorf("the domain did not shut down after 30 attempts"))
		return nil
	}
	if !domainStarted {
		tc.Error(fmt.Errorf("the domain did not start after 30 attempts"))
		return nil
	}

	//   # Name of the domain
	//   DOMAIN_NAME="virtdeploy-vm-0"

	//   # Get the VM serial log file path
	//   VM_SERIAL_LOG=$(sudo virsh dumpxml $DOMAIN_NAME | grep -A 1 console | grep source | cut -d"'" -f2)
	dumpxmlOutput, dumpxmlErr := exec.Command("sudo", "virsh", "dumpxml", h.args.VmName).CombinedOutput()
	if dumpxmlErr != nil {
		tc.Error(dumpxmlErr)
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

	err = utils.WaitForLoginMessageInSerialLog(vmSerialLog, true, 1, fmt.Sprintf("%s/serial.log", h.args.ArtifactsFolder))
	if err != nil {
		tc.Error(err)
	}
	return nil
}

func (h *RebuildRaidHelper) checkTridentServiceWithSsh(tc storm.TestCase) error {
	if h.failed {
		tc.Skip("Previous step failed; skipping this test case.")
		return nil
	}
	if h.args.SkipRebuildRaid {
		tc.Skip("Skipping trident service check step")
		return nil
	}
	err := check.CheckTridentService(
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

// def check_file_exists(connection: Connection, file_path: str) -> bool:
func (h *RebuildRaidHelper) checkFileExists(client *ssh.Client, filePath string) (bool, error) {
	clientSession, err := client.NewSession()
	if err != nil {
		return false, err
	}
	defer clientSession.Close()
	// 	"""
	// 	Checks if a file exists at the specified path on the host.
	// 	"""
	// 	command = f"test -f {file_path}"
	command := fmt.Sprintf("test -f %s", filePath)
	// 	result = _connection_run_command(connection, command)
	err = clientSession.Run(command)
	if err != nil {
		return false, nil
	}
	return true, nil
}

// def trident_rebuild_raid(connection, trident_config, runtime_env):
func (h *RebuildRaidHelper) tridentRebuildRaid(client *ssh.Client, tridentConfig string) error {
	clientSession, err := client.NewSession()
	if err != nil {
		return err
	}
	defer clientSession.Close()

	// 	Args:
	// 		connection : The SSH connection to the host.
	// 		trident_config : The full path to the Trident config on the host.

	// 	"""
	// 	# Provide -c arg, the full path to the RW Trident config.
	// 	trident_return_code, trident_stdout, trident_stderr = trident_run(
	// 		connection, f"rebuild-raid -v trace", runtime_env
	// 	)
	command := fmt.Sprintf("rebuild-raid -c %s -v trace", tridentConfig)
	output, err := trident.InvokeTrident(h.args.Env, client, []string{}, command)
	if err != nil {
		return fmt.Errorf("failed to invoke Trident: %w", err)
	}
	if err := output.Check(); err != nil {
		logrus.Errorf("Trident rebuild-raid stderr:\n%s", output.Stderr)
		return fmt.Errorf("failed to run trident to get host config: %w", err)
	}

	// 	trident_output = trident_stdout + trident_stderr
	// 	print("Trident rebuild-raid output {}".format(trident_output))
	logrus.Infof("Trident rebuild-raid output:\n%s\n%s", output.Stdout, output.Stderr)

	// 	# Check the exit code: if 0, Trident rebuild-raid succeeded.
	// 	if trident_return_code == 0:
	// 		print(
	// 			"Received expected output with exit code 0. Trident rebuild-raid succeeded."
	// 		)
	// 	else:
	// 		raise Exception(
	// 			f"Command unexpectedly returned with exit code {trident_return_code} and output {trident_output}"
	// 		)
	if err != nil {
		return err
	}
	logrus.Info("Trident rebuild-raid succeeded")
	// 	return
	return nil
}

// def copy_host_config(connection, trident_config):
func (h *RebuildRaidHelper) copyHostConfig(client *ssh.Client, tridentConfig string) error {
	clientSession, err := client.NewSession()
	if err != nil {
		return err
	}
	defer clientSession.Close()
	// 	"""
	// 	Copies the Trident config to the host.

	// 	Args:
	// 		connection : The SSH connection to the host.
	// 		trident_config : The full path to the Trident config on the host.

	// 	"""
	// 	# If file at path trident_config does not exist, copy it over from LOCAL_TRIDENT_CONFIG_PATH
	// 	if not check_file_exists(connection, trident_config):
	fileExists, err := h.checkFileExists(client, tridentConfig)
	if err != nil {
		return err
	}
	if !fileExists {
		// 		print(
		// 			f"File {trident_config} does not exist. Copying from {LOCAL_TRIDENT_CONFIG_PATH}"
		// 		)
		LOCAL_TRIDENT_CONFIG_PATH := "/etc/trident/config.yaml"
		logrus.Infof("File %s does not exist. Copying from %s", tridentConfig, LOCAL_TRIDENT_CONFIG_PATH)
		// 		run_ssh_command(
		// 			connection,
		// 			f"cp {LOCAL_TRIDENT_CONFIG_PATH} {trident_config}",
		// 			use_sudo=True,
		// 		)
		copyCommand := fmt.Sprintf("sudo cp %s %s", LOCAL_TRIDENT_CONFIG_PATH, tridentConfig)
		output, err := clientSession.CombinedOutput(copyCommand)
		if err != nil {
			logrus.Errorf("Failed to copy Trident config to host: %s\n%s", err, string(output))
			// return err
		}
	}
	// 	trident_config_output = run_ssh_command(
	// 		connection,
	// 		f"cat {trident_config}",
	// 		use_sudo=True,
	// 	).strip()
	catCommand := fmt.Sprintf("sudo cat %s", tridentConfig)
	tridentConfigOutput, err := clientSession.CombinedOutput(catCommand)
	if err != nil {
		logrus.Errorf("Failed to read Trident config on host: %s\n%s", err, string(tridentConfigOutput))
		// return err
	}
	// 	print("Trident configuration:\n", trident_config_output)
	logrus.Infof("Trident configuration:\n%s", string(tridentConfigOutput))
	return nil
}

// def trigger_rebuild_raid(
//
//	ip_address,
//	user_name,
//	keys_file_path,
//	runtime_env,
//	trident_config,
//
// ):
func (h *RebuildRaidHelper) triggerRebuildRaid(tridentConfig string) error {
	// 	"""Connects to the host via SSH, copies the Trident config to the host, and runs Trident rebuild-raid.

	// 	Args:
	// 		ip_address : The IP address of the host.
	// 		user_name : The user name to ssh into the host with.
	// 		keys_file_path : The full path to the file containing the host ssh keys.
	// 		trident_config : The full path to the Trident config on the host.
	// 	"""
	// 	# Set up SSH client
	// 	connection = create_ssh_connection(ip_address, user_name, keys_file_path)
	client, err := sshclient.OpenSshClient(h.args.SshCliSettings)
	if err != nil {
		return err
	}
	defer client.Close()

	// 	# Copy the Trident config to the host
	// 	copy_host_config(connection, trident_config)
	err = h.copyHostConfig(client, tridentConfig)
	if err != nil {
		return err
	}

	// 	# Re-build Trident and capture logs
	// 	print("Re-building Trident", flush=True)
	logrus.Info("Re-building Trident")
	// 	trident_rebuild_raid(connection, trident_config, runtime_env)
	err = h.tridentRebuildRaid(client, tridentConfig)
	if err != nil {
		return err
	}
	// 	connection.close()
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
	// def main():
	// 	# Setting argument_default=argparse.SUPPRESS means that the program will
	// 	# halt attribute creation if no values provided for arg-s
	// 	parser = argparse.ArgumentParser(
	// 		allow_abbrev=True, argument_default=argparse.SUPPRESS
	// 	)
	// 	parser.add_argument(
	// 		"-i",
	// 		"--ip-address",
	// 		type=str,
	// 		help="IP address of the host.",
	// 	)
	// 	parser.add_argument(
	// 		"-u",
	// 		"--user-name",
	// 		type=str,
	// 		help="User name to ssh into the host with.",
	// 	)
	// 	parser.add_argument(
	// 		"-k",
	// 		"--keys-file-path",
	// 		type=str,
	// 		help="Full path to the file containing the host ssh keys.",
	// 	)
	// 	parser.add_argument(
	// 		"-e",
	// 		"--runtime-env",
	// 		action="store",
	// 		type=str,
	// 		choices=["host", "container"],
	// 		default="host",
	// 		help="Runtime environment for trident: 'host' or 'container'. Default is 'host'.",
	// 	)
	// 	parser.add_argument(
	// 		"-c",
	// 		"--trident-config",
	// 		type=str,
	// 		help="File name of the custom read-write Trident config on the host to point Trident to.",
	// 	)

	// 	args = parser.parse_args()

	// 	# Call helper func that runs Trident rebuild-raid
	// 	trigger_rebuild_raid(
	// 		args.ip_address,
	// 		args.user_name,
	// 		args.keys_file_path,
	// 		args.runtime_env,
	// 		args.trident_config,
	// 	)
	err := h.triggerRebuildRaid(
		"/var/lib/trident/config.yaml",
	)
	if err != nil {
		h.FailFromError(tc, err)
	}

	return nil
}
