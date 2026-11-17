package scenario

import (
	"context"
	"errors"
	"fmt"
	"io"
	"time"

	"tridenttools/storm/utils/sshutils"
	"tridenttools/storm/utils/trident"

	"github.com/klauspost/compress/zstd"
	"github.com/microsoft/storm"
	"github.com/microsoft/storm/pkg/storm/artifacts"
	log "github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
)

const (
	containerLogTimeout  = 30 * time.Second
	diskShutdownTimeout  = 30 * time.Second
	diskShutdownPoll     = time.Second
	failureDiskArtifact  = "failure/os-disk.qcow2.zst"
	failureKeyArtifact   = "failure/ssh-key"
	failureErrorArtifact = "failure/disk-capture-error.txt"
)

func (s *TridentE2EScenario) OnFailure(fc storm.FailureContext) error {
	if s.testHost == nil {
		return nil
	}

	broker := fc.ArtifactBroker()
	var captureErrors []error
	if err := s.captureContainerLogs(context.Background(), broker); err != nil {
		log.WithError(err).Warn("Failed to capture container logs")
		captureErrors = append(captureErrors, err)
	}

	if fc.CleanupPaused() {
		log.Warn("Skipping disk capture: --pause-cleanup preserves the live VM")
		return errors.Join(captureErrors...)
	}

	if vm := s.testHost.VmInfo(); vm != nil {
		if err := captureOSDisk(broker, vm); err != nil {
			broker.PublishArtifactData(failureErrorArtifact, []byte(err.Error()))
			captureErrors = append(captureErrors, err)
		}
		if len(s.sshPrivateKey) != 0 {
			broker.PublishArtifactData(failureKeyArtifact, s.sshPrivateKey)
		}
	}
	return errors.Join(captureErrors...)
}

func (s *TridentE2EScenario) collectContainerLogs(tc storm.TestCase) error {
	if s.runtime != trident.RuntimeTypeContainer {
		tc.Skip("not container runtime")
	}
	return s.captureContainerLogs(tc.Context(), tc.ArtifactBroker())
}

func (s *TridentE2EScenario) captureContainerLogs(ctx context.Context, broker artifacts.ArtifactBroker) error {
	if s.runtime != trident.RuntimeTypeContainer {
		return nil
	}
	if len(s.sshPrivateKey) == 0 {
		return fmt.Errorf("cannot capture container logs: VM SSH key is unavailable")
	}

	ctx, cancel := context.WithTimeout(ctx, containerLogTimeout)
	defer cancel()
	client, err := sshutils.CreateSshClient(ctx, sshutils.SshClientConfig{
		Host:       s.testHost.IPAddress().String(),
		User:       testingUsername,
		PrivateKey: s.sshPrivateKey,
		Timeout:    containerLogTimeout,
	})
	if err != nil {
		return fmt.Errorf("connect for container logs: %w", err)
	}
	defer client.Close()
	stop := context.AfterFunc(ctx, func() { client.Close() })
	defer stop()

	var logErrors []error
	for _, service := range []string{"docker.service", "trident-container.service"} {
		if err := captureServiceLog(client, broker, service); err != nil {
			logErrors = append(logErrors, err)
		}
	}
	return errors.Join(logErrors...)
}

func captureServiceLog(client *ssh.Client, broker artifacts.ArtifactBroker, service string) error {
	session, err := client.NewSession()
	if err != nil {
		return fmt.Errorf("open SSH session for %s logs: %w", service, err)
	}
	defer session.Close()

	writer := broker.StreamArtifactData("container/" + service + ".log")
	session.Stdout = writer
	session.Stderr = writer
	runErr := session.Run("sudo -n journalctl --no-pager -u " + sshutils.ShellQuote(service))
	if err := errors.Join(runErr, writer.Close()); err != nil {
		return fmt.Errorf("capture %s logs: %w", service, err)
	}
	return nil
}

func captureOSDisk(broker artifacts.ArtifactBroker, vm testVmHostInfo) error {
	writer := broker.StreamArtifactData(failureDiskArtifact)
	encoder, err := zstd.NewWriter(writer, zstd.WithEncoderConcurrency(1))
	if err != nil {
		return errors.Join(fmt.Errorf("create disk compressor: %w", err), writer.Close())
	}

	log.Infof("Capturing OS disk for VM %q", vm.VmName())
	copyErr := vm.WriteOSDisk(encoder)
	if err := errors.Join(copyErr, encoder.Close(), writer.Close()); err != nil {
		return fmt.Errorf("capture OS disk for VM %q: %w", vm.VmName(), err)
	}
	return nil
}

func (t *testHostVirtDeploy) WriteOSDisk(writer io.Writer) error {
	if err := t.stopForDiskCapture(); err != nil {
		return err
	}
	_, vol, err := t.diskVolume(0)
	if err != nil {
		return err
	}
	// Download through libvirt so capture does not depend on host file permissions.
	if err := t.lv.StorageVolDownload(vol, writer, 0, 0, 0); err != nil {
		return fmt.Errorf("download OS disk volume %q: %w", vol.Name, err)
	}
	return nil
}

func (t *testHostVirtDeploy) stopForDiskCapture() error {
	active, err := t.lv.DomainIsActive(t.vm.Domain)
	if err != nil {
		return fmt.Errorf("check VM %q before disk capture: %w", t.vm.Name, err)
	}
	if active == 0 {
		return nil
	}

	if err := t.lv.DomainShutdown(t.vm.Domain); err != nil {
		log.WithError(err).Warnf("Graceful shutdown of VM %q failed", t.vm.Name)
	} else {
		deadline := time.NewTimer(diskShutdownTimeout)
		defer deadline.Stop()
		poll := time.NewTicker(diskShutdownPoll)
		defer poll.Stop()
	wait:
		for {
			select {
			case <-deadline.C:
				break wait
			case <-poll.C:
				active, err = t.lv.DomainIsActive(t.vm.Domain)
				if err != nil {
					return fmt.Errorf("wait for VM %q to shut down: %w", t.vm.Name, err)
				}
				if active == 0 {
					return nil
				}
			}
		}
	}

	// A hung guest may ignore ACPI; never copy a disk while QEMU can still write it.
	log.Warnf("Forcing VM %q off for disk capture", t.vm.Name)
	if err := t.lv.DomainDestroy(t.vm.Domain); err != nil {
		return fmt.Errorf("stop VM %q for disk capture: %w", t.vm.Name, err)
	}
	return nil
}
