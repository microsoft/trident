package virtdeploy

import "os/exec"

func sudoCommand(cmd string, args []string) *exec.Cmd {
	return exec.Command("sudo", append([]string{cmd}, args...)...)
}
