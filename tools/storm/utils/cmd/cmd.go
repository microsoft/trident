package cmd

import (
	"bytes"
	"fmt"
	"io"
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

// Output runs the command and returns its standard output, regardless of exit
// code.
func Output(name string, arg ...string) (string, error) {
	return Cmd(name, arg...).Output()
}

// CombinedOutput runs the command and returns its combined standard output and
// standard error, regardless of exit code.
func CombinedOutput(name string, arg ...string) (string, error) {
	return Cmd(name, arg...).CombinedOutput()
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
	_, err := c.run()
	return err
}

// Output runs the command and returns its standard output.
// Output is returned in all cases, but error is also returned to indicate success or failure (i.e., exit code is not ignored).
func (c *CommandRunner) Output() (string, error) {
	p, err := c.run()
	return p.stdout.String(), err
}

// CombinedOutput runs the command and returns its combined standard output and standard error.
// Output is returned in all cases, but error is also returned to indicate success or failure (i.e., exit code is not ignored).
func (c *CommandRunner) CombinedOutput() (string, error) {
	p, err := c.run()
	return p.combined.String(), err
}

func (c *CommandRunner) run() (*cmdOutProcessor, error) {
	cmd := exec.Command(c.name, c.args...)
	p := newCmdOutProcessor()
	cmd.Stdout = p.StdoutWriter()
	cmd.Stderr = p.StderrWriter()

	if strings.HasSuffix(c.name, "sudo") {
		log.Infof("Running command with elevated privileges: %s %v", c.name, c.args)
	} else {
		log.Tracef("Running command: %s %v", c.name, c.args)
	}

	err := cmd.Run()
	if err != nil {
		log.Errorf("Process '%s' with args %v failed. Command output:\n%s", c.name, c.args, p.combined.String())
		return p, fmt.Errorf("process '%s' failed: %v", c.name, err)
	}

	return p, nil
}

type cmdOutProcessor struct {
	combined bytes.Buffer
	stdout   bytes.Buffer
	stderr   bytes.Buffer
}

func newCmdOutProcessor() *cmdOutProcessor {
	return &cmdOutProcessor{}
}

func (p *cmdOutProcessor) StdoutWriter() io.Writer {
	return &multiWriter{writers: []*bytes.Buffer{&p.combined, &p.stdout}}
}

func (p *cmdOutProcessor) StderrWriter() io.Writer {
	return &multiWriter{writers: []*bytes.Buffer{&p.combined, &p.stderr}}
}

type multiWriter struct {
	writers []*bytes.Buffer
}

func (mw *multiWriter) Write(p []byte) (n int, err error) {
	for _, w := range mw.writers {
		// Writing to bytes.Buffer never fails
		_, _ = w.Write(p)
	}
	return len(p), nil
}
