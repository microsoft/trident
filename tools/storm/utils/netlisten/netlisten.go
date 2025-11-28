package netlisten

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"strings"

	"github.com/sirupsen/logrus"
)

func KillUpdateServer(port int) error {
	logrus.Tracef("Kill process found using port %d", port)
	cmd := exec.Command("lsof", "-ti", fmt.Sprintf("tcp:%d", port))
	var out bytes.Buffer
	cmd.Stdout = &out
	if err := cmd.Run(); err != nil {
		// No process found is not an error for our use case
		logrus.Tracef("No process found on port %d", port)
		return nil
	}
	pids := strings.Fields(out.String())
	for _, pid := range pids {
		logrus.Tracef("Kill process %v", pid)
		killCmd := exec.Command("kill", "-9", pid)
		_ = killCmd.Run() // Ignore errors for robustness
	}
	return nil
}

func StartNetListenAndWait(ctx context.Context, port int, pathToServe string, logName string, startedChannel chan bool) error {
	cmdPath := "bin/netlisten"
	if _, err := os.Stat(cmdPath); os.IsNotExist(err) {
		logrus.Error("bin/netlisten not found")
		return fmt.Errorf("netlisten not found at %s: %w", cmdPath, err)
	}

	cmdArgs := []string{
		"-p", fmt.Sprint(port),
		"-s", pathToServe,
		"--force-color",
		"--full-logstream", logName,
	}
	logrus.Tracef("netlisten started with args: %v", cmdArgs)
	cmd := exec.CommandContext(ctx, cmdPath, cmdArgs...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	if err := cmd.Start(); err != nil {
		return fmt.Errorf("failed to start netlisten for port %d: %w", port, err)
	}

	// Signal that netlisten has started
	startedChannel <- true

	// Wait for the command to finish
	if err := cmd.Wait(); err != nil {
		return fmt.Errorf("netlisten for port %d failed: %w", port, err)
	}
	logrus.Tracef("netlisten for port %d exited", port)

	return nil
}
