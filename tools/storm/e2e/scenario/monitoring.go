package scenario

import (
	"context"
	"fmt"
	"io"

	"github.com/sirupsen/logrus"

	"tridenttools/pkg/ref"
	"tridenttools/storm/utils/libvirtutils"
)

// spawnVMSerialMonitor starts the VM serial monitor for the test host IF it is a
// VM.
//
// The output of the serial monitor will be written live to the provided
// io.WriteCloser. The WriteCloser will be closed when the monitor exits.
//
// If an error occurs while starting the monitor, that error is returned.
//
// The returned channel can be used to wait for the monitor to finish by waiting
// on it for a value. The channel will be closed once the monitor has finished.
// If the monitor doesn't run (because the hardware type is not VM), the channel
// will receive a value immediately.
//
// The monitor will run until the context is cancelled or the login prompt is
// detected.
func (s *TridentE2EScenario) spawnVMSerialMonitor(ctx context.Context, output io.WriteCloser) (<-chan bool, error) {
	// Channel to signal when the monitor is done. Buffered with size 1 to avoid
	// deadlocks when we exit early and send a message to it immediately.
	doneChannel := make(chan bool, 1)

	// Only spawn the VM serial logger if the hardware type is VM. Otherwise, do
	// nothing.
	if s.hardware != HardwareTypeVM {
		// Immediately signal that the monitor is done
		doneChannel <- true
		close(doneChannel)
		output.Close()
		return doneChannel, nil
	}

	// Get VM info
	vmInfo := s.testHost.VmInfo()
	if ref.IsNilInterface(vmInfo) {
		close(doneChannel)
		output.Close()
		return doneChannel, fmt.Errorf("vm host info not set")
	}

	go func() {
		defer func() {
			// On exit signal that we're done and close the channel.
			doneChannel <- true
			close(doneChannel)
		}()
		defer output.Close()

		err := libvirtutils.WaitForVmSerialLogLoginLibvirt(ctx, vmInfo.Lv(), vmInfo.LvDomain(), output)
		if err != nil {
			errStr := fmt.Sprintf("VM serial log monitor ended with error: %v", err)
			logrus.Error(errStr)

			// Best effort write to output
			_, writeErr := output.Write([]byte(fmt.Sprintf("ERROR: %s", errStr)))
			if writeErr != nil {
				logrus.Errorf("failed to write error to VM serial log output: %v", writeErr)
			}
		} else {
			logrus.Infof("VM serial log monitor ended successfully")
		}
	}()

	return doneChannel, nil
}