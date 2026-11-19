package acl

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path"
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

type runner struct {
	args       scenarioArgs
	vmName, ip string
	qemu       qemu.QemuConfig
	log        *os.File
	client     *ssh.Client
	snapshots  map[string]snapshot
}

func (r *runner) settings() stormsshconfig.SshCliSettings {
	return stormsshconfig.SshCliSettings{PrivateKeyPath: filepath.Join(r.args.ArtifactsDir, "id_rsa"), Host: r.ip, User: "testuser", Port: 22, Timeout: 5}
}

func (r *runner) closeClient() {
	if r.client != nil {
		_ = r.client.Close()
		r.client = nil
	}
}

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
	if ctx.Err() != nil {
		r.closeClient()
		return errors.Join(err, ctx.Err())
	}
	return err
}

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

func (r *runner) trident(arguments, logName string, expectReboot bool) (string, error) {
	var out *stormsshconfig.SshCmdOutput
	err := r.withClient(func(ctx context.Context, client *ssh.Client) error {
		var invokeErr error
		var result error
		if expectReboot && r.args.Variant == "mshv" {
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

func (r *runner) upload(source, dest string) error {
	return r.withClient(func(_ context.Context, client *ssh.Client) error {
		return stormsftp.UploadLocalFile(client, source, dest)
	})
}

func (r *runner) waitBoot(previous string) (string, error) {
	r.closeClient()
	ctx, cancel := context.WithTimeout(context.Background(), r.args.BootTimeout)
	defer cancel()
	if r.ip == "" {
		address, err := stormretry.RetryContext(ctx, 2*time.Second, func(context.Context, int) (*string, error) {
			ips, err := r.qemu.GetVmIPAddresses(r.vmName)
			if err != nil {
				return nil, err
			}
			if len(ips) != 1 {
				return nil, fmt.Errorf("expected one guest address, got %v", ips)
			}
			return &ips[0], nil
		})
		if err != nil {
			return "", err
		}
		r.ip = *address
	}
	client, id, err := stormsshcheck.WaitForBoot(ctx, r.settings(), previous)
	if err != nil {
		return "", err
	}
	r.client = client
	_, err = fmt.Fprintf(r.log, "boot: %s (previous %q)\n", id, previous)
	return id, err
}

func (r *runner) reboot() error {
	command := "sudo -n systemctl reboot --no-block"
	if r.args.Variant == "mshv" {
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
	if err != nil || r.args.Variant != "mshv" {
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

func (r *runner) save(name string, data []byte) error {
	file := filepath.Join(r.args.OutputPath, "evidence", filepath.FromSlash(name))
	if err := os.MkdirAll(filepath.Dir(file), 0700); err != nil {
		return err
	}
	return os.WriteFile(file, data, 0600)
}

func (r *runner) readBootTrees(out *snapshot) error {
	return r.withClient(func(_ context.Context, client *ssh.Client) error {
		remote, err := stormsftp.NewSftpSudoClient(client)
		if err != nil {
			return err
		}
		defer remote.Close()
		if st, err := remote.Stat("/dev/mshv"); err == nil {
			out.MSHVPresent = true
			out.MSHVCharacter = st.Mode()&os.ModeCharDevice != 0
		} else if !os.IsNotExist(err) {
			return err
		}
		for _, tree := range []string{"BOOT", "AZLA", "AZLB"} {
			base := "/boot/EFI/" + tree
			st, err := remote.Stat(base)
			if os.IsNotExist(err) {
				continue
			}
			if err != nil {
				return err
			}
			if !st.IsDir() {
				return fmt.Errorf("%s is not a directory", base)
			}
			files := make(map[string]fileDigest)
			walker := remote.Walk(base)
			for walker.Step() {
				if err := walker.Err(); err != nil {
					return err
				}
				info := walker.Stat()
				if info.IsDir() {
					continue
				}
				if !info.Mode().IsRegular() {
					return fmt.Errorf("nonregular boot file %s", walker.Path())
				}
				file, err := remote.Open(walker.Path())
				if err != nil {
					return err
				}
				hash := sha256.New()
				_, readErr := io.Copy(hash, file)
				if err := errors.Join(readErr, file.Close()); err != nil {
					return err
				}
				name := strings.ToLower(strings.TrimPrefix(path.Clean(walker.Path()), base+"/"))
				files[name] = fileDigest{SHA256: hex.EncodeToString(hash.Sum(nil)), Size: info.Size()}
			}
			out.Trees[tree] = files
		}
		return nil
	})
}
