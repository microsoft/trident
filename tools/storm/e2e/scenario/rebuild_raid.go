package scenario

import (
	"context"
	"fmt"
	"time"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"

	"tridenttools/storm/utils/trident"
)

// raidMemberDiskIndex is the 0-based index of the VM data disk failed to
// simulate a degraded RAID array. Disk 0 is the OS disk; the RAID member disks
// start at index 1.
const raidMemberDiskIndex uint = 1

// addRebuildRaidTests registers the rebuild-raid test cases. They simulate a
// failed RAID member disk, boot the (now degraded) host, and run `trident
// rebuild-raid` to rebuild the array onto a fresh disk. This ports the legacy
// VM-only `storm-trident helper rebuild-raid` step and self-selects with the
// HasRebuildableRaid() && IsVM() gate at the call site.
func (s *TridentE2EScenario) addRebuildRaidTests(r storm.TestRegistrar) {
	r.RegisterTestCase("rebuild-raid-fail-disk", s.rebuildRaidFailDisk)
	r.RegisterTestCase("rebuild-raid", s.rebuildRaid)
	r.RegisterTestCase("validate-rebuild-raid", s.validateHostState)
}

// rebuildRaidFailDisk powers off the VM, replaces one RAID member disk with a
// blank one, boots the host back up (now with a degraded array), and waits for
// it to reach the login prompt.
func (s *TridentE2EScenario) rebuildRaidFailDisk(tc storm.TestCase) error {
	vmInfo := s.testHost.VmInfo()
	if vmInfo == nil {
		return fmt.Errorf("rebuild-raid requires a VM test host")
	}

	// Serve phonehome + capture the serial log across the reboot that follows
	// the disk replacement.
	monitorCtx, cancel := context.WithCancel(tc.Context())
	defer cancel()
	monWaitChan, monErr := s.spawnVMSerialMonitor(monitorCtx, tc.ArtifactBroker().StreamArtifactData(tc.Name()+"/serial.log"))
	if monErr != nil {
		return fmt.Errorf("failed to start VM serial monitor: %w", monErr)
	}
	defer func() {
		select {
		case <-time.After(time.Minute):
			logrus.Infof("Waited 1 minute for serial monitor to reach login prompt, cancelling monitor.")
			cancel()
		case <-monWaitChan:
		}
	}()

	// Drop the current SSH client: the VM is about to be powered off.
	if s.sshClient != nil {
		s.sshClient.Close()
		s.sshClient = nil
	}

	logrus.Infof("Failing RAID member disk %d and rebooting the host...", raidMemberDiskIndex)
	if err := vmInfo.FailAndReplaceDataDisk(raidMemberDiskIndex); err != nil {
		return fmt.Errorf("failed to replace RAID member disk: %w", err)
	}

	// Reconnect once the degraded host is back up, and confirm Trident is
	// healthy (the previous servicing commit is unchanged by a disk failure).
	connCtx, cancel := context.WithTimeout(tc.Context(), time.Minute*5)
	defer cancel()
	if err := s.populateSshClient(connCtx); err != nil {
		return fmt.Errorf("failed to reconnect after RAID member disk failure: %w", err)
	}
	logrus.Info("Reacquired SSH connection to degraded host.")

	if err := trident.CheckTridentService(s.sshClient, s.runtime, time.Minute*2, true); err != nil {
		tc.FailFromError(err)
	}

	return nil
}

// rebuildRaid runs `trident rebuild-raid` on the degraded host to rebuild the
// RAID array onto the replacement disk.
func (s *TridentE2EScenario) rebuildRaid(tc storm.TestCase) error {
	if err := s.populateSshClient(tc.Context()); err != nil {
		return fmt.Errorf("failed to connect before rebuild-raid: %w", err)
	}

	logrus.Info("Running Trident rebuild-raid...")
	out, err := trident.InvokeTrident(s.runtime, s.sshClient, nil, "rebuild-raid -v trace")
	if err != nil {
		return fmt.Errorf("failed to invoke Trident rebuild-raid: %w", err)
	}
	if err := out.Check(); err != nil {
		tc.FailFromError(fmt.Errorf("Trident rebuild-raid failed: %s", out.Report()))
		return nil
	}
	logrus.Info("Trident rebuild-raid succeeded.")

	return nil
}
