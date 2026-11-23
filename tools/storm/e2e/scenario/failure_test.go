package scenario

import (
	"bytes"
	"errors"
	"io"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"tridenttools/storm/utils/trident"

	"github.com/klauspost/compress/zstd"
	"github.com/microsoft/storm"
	"github.com/microsoft/storm/pkg/storm/artifacts"
)

type failureTestContext struct {
	storm.FailureContext
	broker artifacts.ArtifactBroker
	paused bool
}

func (c failureTestContext) ArtifactBroker() artifacts.ArtifactBroker { return c.broker }
func (c failureTestContext) CleanupPaused() bool                      { return c.paused }

type failureTestHost struct {
	testHostInfo
	vm testVmHostInfo
}

func (h failureTestHost) VmInfo() testVmHostInfo { return h.vm }
func (h failureTestHost) IPAddress() net.IP      { return net.IPv4(127, 0, 0, 1) }

type failureTestVM struct {
	testVmHostInfo
	write func(io.Writer) error
}

func (v failureTestVM) VmName() string                { return "test-vm" }
func (v failureTestVM) WriteOSDisk(w io.Writer) error { return v.write(w) }

type failureTestBroker struct {
	artifacts.ArtifactBroker
	t        *testing.T
	dir      string
	closeErr error
}

func (b failureTestBroker) PublishArtifactData(name string, data []byte) {
	b.t.Helper()
	path := filepath.Join(b.dir, name)
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		b.t.Fatal(err)
	}
	if err := os.WriteFile(path, data, 0600); err != nil {
		b.t.Fatal(err)
	}
}

func (b failureTestBroker) StreamArtifactData(name string) io.WriteCloser {
	b.t.Helper()
	path := filepath.Join(b.dir, name)
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		b.t.Fatal(err)
	}
	file, err := os.Create(path)
	if err != nil {
		b.t.Fatal(err)
	}
	return failureTestWriter{file, b.closeErr}
}

type failureTestWriter struct {
	*os.File
	closeErr error
}

func (w failureTestWriter) Close() error {
	return errors.Join(w.File.Close(), w.closeErr)
}

func TestOnFailureCapturesDiskAndKey(t *testing.T) {
	disk := bytes.Repeat([]byte("qcow2 fixture"), 1000)
	broker := failureTestBroker{t: t, dir: t.TempDir()}
	s := &TridentE2EScenario{
		testHost: failureTestHost{vm: failureTestVM{write: func(w io.Writer) error {
			_, err := w.Write(disk)
			return err
		}}},
		sshPrivateKey: []byte("ephemeral test key"),
	}
	if err := s.OnFailure(failureTestContext{broker: broker}); err != nil {
		t.Fatal(err)
	}

	compressed, err := os.ReadFile(filepath.Join(broker.dir, failureDiskArtifact))
	if err != nil {
		t.Fatal(err)
	}
	decoder, err := zstd.NewReader(nil)
	if err != nil {
		t.Fatal(err)
	}
	defer decoder.Close()
	got, err := decoder.DecodeAll(compressed, nil)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, disk) {
		t.Fatal("decompressed disk does not match the source")
	}
	key, err := os.ReadFile(filepath.Join(broker.dir, failureKeyArtifact))
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(key, s.sshPrivateKey) {
		t.Fatal("SSH key was not preserved")
	}
}

func TestOnFailurePreservesPausedVM(t *testing.T) {
	s := &TridentE2EScenario{
		testHost: failureTestHost{vm: failureTestVM{write: func(io.Writer) error {
			t.Fatal("disk capture must not stop a VM when cleanup is paused")
			return nil
		}}},
	}
	// No broker: paused capture must not even open the disk artifact.
	if err := s.OnFailure(failureTestContext{paused: true}); err != nil {
		t.Fatal(err)
	}
}

func TestOnFailureBeforeVMCreation(t *testing.T) {
	if err := (&TridentE2EScenario{}).OnFailure(failureTestContext{}); err != nil {
		t.Fatal(err)
	}
}

func TestOnFailureReportsDiskAndCloseErrors(t *testing.T) {
	diskErr := errors.New("volume download failed")
	closeErr := errors.New("artifact close failed")
	broker := failureTestBroker{t: t, dir: t.TempDir(), closeErr: closeErr}
	s := &TridentE2EScenario{testHost: failureTestHost{vm: failureTestVM{
		write: func(w io.Writer) error {
			if _, err := w.Write([]byte("partial disk")); err != nil {
				t.Fatal(err)
			}
			return diskErr
		},
	}}}

	err := s.OnFailure(failureTestContext{broker: broker})
	if !errors.Is(err, diskErr) || !errors.Is(err, closeErr) {
		t.Fatalf("capture lost an error: %v", err)
	}
	note, err := os.ReadFile(filepath.Join(broker.dir, failureErrorArtifact))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(note), diskErr.Error()) {
		t.Fatalf("partial artifact has no error note: %s", note)
	}
}

func TestOnFailureContainerLogErrorDoesNotPreventDiskCapture(t *testing.T) {
	broker := failureTestBroker{t: t, dir: t.TempDir()}
	written := false
	s := &TridentE2EScenario{
		runtime:       trident.RuntimeTypeContainer,
		sshPrivateKey: []byte("invalid test key"),
		testHost: failureTestHost{vm: failureTestVM{write: func(w io.Writer) error {
			written = true
			_, err := w.Write([]byte("disk"))
			return err
		}}},
	}
	err := s.OnFailure(failureTestContext{broker: broker})
	if err == nil || !strings.Contains(err.Error(), "SSH key") {
		t.Fatalf("expected the log-connection error, got %v", err)
	}
	if !written {
		t.Fatal("log capture failure prevented disk capture")
	}
}
