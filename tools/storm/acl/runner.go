package acl

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"golang.org/x/crypto/ssh"
	stormretry "tridenttools/storm/utils/retry"
	stormsshcheck "tridenttools/storm/utils/ssh/check"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	stormsftp "tridenttools/storm/utils/ssh/sftp"
	stormtrident "tridenttools/storm/utils/trident"
	qemu "tridenttools/storm/utils/vm/qemu"
)

// runner owns guest transport, VM control and evidence output for one scenario.
type runner struct {
	args       scenarioArgs
	vmName, ip string
	qemu       qemu.QemuConfig
	log        *os.File
	client     *ssh.Client
}

// prepareUpdate creates the host configuration and uploads it with the COSI for later updates.
func (r *runner) prepareUpdate() error {
	if _, err := r.command("mkdir -p " + guestDir); err != nil {
		return err
	}
	config := filepath.Join(r.args.OutputPath, "update.yaml")
	if err := os.WriteFile(config, []byte(updateConfig()), 0600); err != nil {
		return err
	}
	if err := r.upload(config, guestDir+"/update.yaml"); err != nil {
		return err
	}
	return r.upload(filepath.Join(r.args.ArtifactsDir, "update.cosi"), guestDir+"/update.cosi")
}

// settings selects the test user's SSH credentials and current guest address.
func (r *runner) settings() stormsshconfig.SshCliSettings {
	return stormsshconfig.SshCliSettings{PrivateKeyPath: filepath.Join(r.args.ArtifactsDir, "id_rsa"), Host: r.ip, User: "testuser", Port: 22, Timeout: 5}
}

// closeClient releases the cached SSH connection before reconnecting or cleaning up.
func (r *runner) closeClient() {
	if r.client != nil {
		_ = r.client.Close()
		r.client = nil
	}
}

// withClient reuses successful connections without replaying failed operations.
func (r *runner) withClient(operation func(context.Context, *ssh.Client) error) error {
	ctx, cancel := context.WithTimeout(context.Background(), r.args.OperationTimeout)
	defer cancel()
	if r.client == nil {
		connect, done := context.WithTimeout(ctx, 10*time.Second)
		var err error
		r.client, err = stormsshclient.OpenSshClientContext(connect, r.settings())
		done()
		if err != nil {
			return err
		}
	}
	client := r.client
	stop := context.AfterFunc(ctx, func() { _ = client.Close() })
	defer stop()
	err := operation(ctx, client)
	stop()
	if err != nil || ctx.Err() != nil {
		r.closeClient()
	}
	return errors.Join(err, ctx.Err())
}

// command runs a privileged guest shell command and records its output.
func (r *runner) command(command string) (string, error) {
	command = "sudo -n sh -c '" + strings.ReplaceAll(command, "'", "'\"'\"'") + "'"
	var out *stormsshconfig.SshCmdOutput
	err := r.withClient(func(ctx context.Context, client *ssh.Client) error {
		var err error
		out, err = stormsshclient.RunCommandContext(ctx, client, command)
		if _, logErr := fmt.Fprintf(r.log, "$ %s\n%s\nerror: %v\n", command, out.Report(), err); logErr != nil {
			return errors.Join(err, logErr)
		}
		return stormtrident.CheckCommandResult(out, err, false)
	})
	if out == nil {
		return "", err
	}
	return strings.TrimSpace(out.Stdout), err
}

// trident captures command evidence and handles expected reboot disconnects.
func (r *runner) trident(arguments, logName string, expectReboot bool) (string, error) {
	var out *stormsshconfig.SshCmdOutput
	err := r.withClient(func(ctx context.Context, client *ssh.Client) error {
		var invokeErr error
		var result error
		if expectReboot && r.args.Variant == variantMSHV {
			out, invokeErr = r.tridentWithRestart(ctx, client, arguments, logName)
			result = invokeErr
		} else {
			out, invokeErr = stormtrident.InvokeTrident(stormtrident.RuntimeTypeHost, client, nil, arguments)
			result = stormtrident.CheckCommandResult(out, invokeErr, expectReboot)
		}
		report := fmt.Sprintf("$ trident %s\nerror: %v\n", arguments, invokeErr)
		if out != nil {
			report = fmt.Sprintf("$ trident %s\n%s\nerror: %v\n", arguments, out.Report(), invokeErr)
		}
		_, logErr := io.WriteString(r.log, report)
		return errors.Join(result, logErr, r.save(logName+".log", []byte(report)))
	})
	if out == nil {
		return "", err
	}
	return out.Stdout, err
}

// upload copies a local input into the guest through privileged SFTP.
func (r *runner) upload(source, dest string) error {
	return r.withClient(func(_ context.Context, client *ssh.Client) error {
		return stormsftp.UploadLocalFile(client, source, dest)
	})
}

// waitBoot reconnects to an SSH-ready guest whose boot ID differs from previousBootID.
// NO_PREVIOUS_BOOT_ID accepts the initial boot.
func (r *runner) waitBoot(previousBootID string) (string, error) {
	r.closeClient()
	ctx, cancel := context.WithTimeout(context.Background(), r.args.BootTimeout)
	defer cancel()
	var id string
	var err error
	if r.ip == "" {
		boot, err := stormretry.RetryContext(ctx, 2*time.Second, func(ctx context.Context, _ int) (*bootConnection, error) {
			ips, err := r.qemu.GetVmIPAddresses(ctx, r.vmName)
			if err != nil {
				return nil, err
			}
			attempt, done := context.WithTimeout(ctx, 10*time.Second)
			defer done()
			settings := r.settings()
			return waitForBootCandidates(attempt, ips, func(ctx context.Context, ip string) (*ssh.Client, string, error) {
				candidate := settings
				candidate.Host = ip
				return stormsshcheck.WaitForBoot(ctx, candidate, previousBootID)
			})
		})
		if err != nil {
			return "", err
		}
		r.ip, r.client, id = boot.ip, boot.client, boot.id
	} else {
		r.client, id, err = stormsshcheck.WaitForBoot(ctx, r.settings(), previousBootID)
		if err != nil {
			return "", err
		}
	}
	_, err = fmt.Fprintf(r.log, "boot: %s (previous %q)\n", id, previousBootID)
	return id, err
}

// reboot requests a guest restart, using poweroff followed by domain start for MSHV.
func (r *runner) reboot() error {
	command := "sudo -n systemctl reboot --no-block"
	if r.args.Variant == variantMSHV {
		command = "sudo -n systemctl poweroff --no-block"
	}
	err := r.withClient(func(ctx context.Context, client *ssh.Client) error {
		out, err := stormsshclient.RunCommandContext(ctx, client, command)
		_, logErr := fmt.Fprintf(r.log, "$ %s\n%s\nerror: %v\n", command, out.Report(), err)
		var disconnected *ssh.ExitMissingError
		if errors.As(err, &disconnected) {
			// waitBoot still requires a new boot ID before servicing continues.
			return logErr
		}
		return errors.Join(stormtrident.CheckCommandResult(out, err, false), logErr)
	})
	r.closeClient()
	if err != nil || r.args.Variant != variantMSHV {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), r.args.BootTimeout)
	defer cancel()
	if _, err := fmt.Fprintln(r.log, "waiting for guest shutoff before libvirt start"); err != nil {
		return err
	}
	if err := r.qemu.StartQemuVMWhenShutoff(ctx, r.vmName); err != nil {
		return err
	}
	r.ip = ""
	_, err = fmt.Fprintln(r.log, "started existing domain after guest shutoff")
	return err
}

// save writes named evidence beneath the scenario's output directory.
func (r *runner) save(name string, data []byte) error {
	file := filepath.Join(r.args.OutputPath, "evidence", filepath.FromSlash(name))
	if err := os.MkdirAll(filepath.Dir(file), 0700); err != nil {
		return err
	}
	return os.WriteFile(file, data, 0600)
}
