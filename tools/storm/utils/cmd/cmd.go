package cmd

import (
	"bytes"
	"fmt"
	"os/exec"
	"strings"

	log "github.com/sirupsen/logrus"
)

// Wrapper over exec.Command for script-like usage.
//
// It runs the command with the given name and arguments,
// waits for it to finish, and returns an error if the command
// fails to run or doesn't complete successfully.
//
// Output will be captured and logged in case of failure.
func Run(name string, arg ...string) error {
	return Cmd(name, arg...).Run()
}

type CommandRunner struct {
	name string
	args []string
}

func Cmd(name string, arg ...string) *CommandRunner {
	return &CommandRunner{
		name: name,
		args: arg,
	}
}

func RunGroup(commands ...*CommandRunner) error {
	for _, command := range commands {
		err := command.Run()
		if err != nil {
			return err
		}
	}

	return nil
}

func (c *CommandRunner) Run() error {
	var output bytes.Buffer
	cmd := exec.Command(c.name, c.args...)
	cmd.Stdout = &output
	cmd.Stderr = &output

	if strings.HasSuffix(c.name, "sudo") {
		log.Infof("Running command with elevated privileges: %s %v", c.name, c.args)
	} else {
		log.Tracef("Running command: %s %v", c.name, c.args)
	}

	err := cmd.Run()
	if err != nil {
		log.Errorf("Process '%s' with args %v failed. Command output:\n%s", c.name, c.args, output.String())
		return fmt.Errorf("process '%s' failed: %v", c.name, err)
	}

	return nil
}
