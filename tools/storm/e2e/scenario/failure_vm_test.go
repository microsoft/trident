package scenario

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"github.com/digitalocean/go-libvirt"
	"github.com/google/uuid"
	"github.com/klauspost/compress/zstd"
	"github.com/microsoft/storm"
	stormssh "github.com/microsoft/storm/utils/ssh"
	"golang.org/x/crypto/ssh"

	"tridenttools/pkg/virtdeploy"
)

const (
	failureVMTestEnv  = "TRIDENT_TEST_FAILURE_VM"
	failureVMChildEnv = "TRIDENT_TEST_FAILURE_VM_CHILD"
	failureVMDirEnv   = "TRIDENT_TEST_FAILURE_VM_DIR"
	failureVMNsEnv    = "TRIDENT_TEST_FAILURE_VM_NAMESPACE"
	failureVMPathEnv  = "TRIDENT_TEST_FAILURE_VM_VOLUME_DIR"
	failureVMReason   = "intentional failure for disk capture"
)

func TestFailureArtifactsSurviveVMCleanup(t *testing.T) {
	if os.Getenv(failureVMChildEnv) == "1" {
		runFailureVMChild()
		return
	}
	if os.Getenv(failureVMTestEnv) != "1" {
		t.Skip("set TRIDENT_TEST_FAILURE_VM=1 to run against local qemu:///system")
	}

	dir := t.TempDir()
	volumeRoot, err := os.MkdirTemp("/var/tmp", "storm-capture-test-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := os.RemoveAll(volumeRoot); err != nil {
			t.Error(err)
		}
	})
	if err := os.Chmod(volumeRoot, 0755); err != nil {
		t.Fatal(err)
	}
	volumeDir := filepath.Join(volumeRoot, "pool")
	if err := os.Mkdir(volumeDir, 0755); err != nil {
		t.Fatal(err)
	}
	namespace := "storm-capture-test-" + uuid.NewString()
	t.Cleanup(func() {
		if err := virtdeploy.DeleteResources(namespace); err != nil {
			t.Error(err)
		}
	})
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestFailureArtifactsSurviveVMCleanup$")
	cmd.Env = append(os.Environ(),
		failureVMChildEnv+"=1", failureVMDirEnv+"="+dir, failureVMNsEnv+"="+namespace,
		failureVMPathEnv+"="+volumeDir,
		"TF_BUILD=False",
	)
	output, err := cmd.CombinedOutput()
	if err == nil || ctx.Err() != nil {
		t.Fatalf("expected the intentional scenario failure, got %v (context: %v)\n%s", err, ctx.Err(), output)
	}
	if !bytes.Contains(output, []byte(failureVMReason)) {
		t.Fatalf("original failure was lost:\n%s", output)
	}
	if bytes.Contains(output, []byte("OnFailure() hook failed")) {
		t.Fatalf("failure capture failed:\n%s", output)
	}

	file, err := os.Open(filepath.Join(dir, failureDiskArtifact))
	if err != nil {
		t.Fatalf("disk did not survive cleanup: %v\n%s", err, output)
	}
	defer file.Close()
	decoder, err := zstd.NewReader(file)
	if err != nil {
		t.Fatal(err)
	}
	defer decoder.Close()
	hash := sha256.New()
	if _, err := io.Copy(hash, decoder); err != nil {
		t.Fatal(err)
	}
	expected, err := os.ReadFile(filepath.Join(dir, "source.sha256"))
	if err != nil {
		t.Fatal(err)
	}
	if hex.EncodeToString(hash.Sum(nil)) != string(expected) {
		t.Fatal("recovered disk differs from the volume deleted during cleanup")
	}
	key, err := os.ReadFile(filepath.Join(dir, failureKeyArtifact))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := ssh.ParsePrivateKey(key); err != nil {
		t.Fatalf("invalid recovery key: %v", err)
	}
	if _, err := os.Stat(filepath.Join(volumeDir, namespace+"-vm-0-volume-0.qcow2")); !os.IsNotExist(err) {
		t.Fatalf("source volume survived cleanup: %v", err)
	}
	t.Log("intentional product failure preserved; disk and SSH key survived VM cleanup; disk checksum matches")
}

type failureVMScenario struct {
	TridentE2EScenario
	dir       string
	namespace string
	volumeDir string
}

func (s *failureVMScenario) Args() any { return nil }
func (s *failureVMScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("create-vm", func(storm.TestCase) error {
		uri, err := url.Parse(string(libvirt.QEMUSystem))
		if err != nil {
			return err
		}
		lv, err := libvirt.ConnectToURI(uri)
		if err != nil {
			return err
		}
		name := s.namespace + "-vm-0"
		pool, err := lv.StoragePoolDefineXML(fmt.Sprintf(
			`<pool type="dir"><name>%s-pool</name><target><path>%s</path></target></pool>`,
			s.namespace, s.volumeDir), 0)
		if err != nil {
			lv.Disconnect()
			return err
		}
		if err := lv.StoragePoolCreate(pool, 0); err != nil {
			lv.Disconnect()
			return err
		}
		vol, err := lv.StorageVolCreateXML(pool, fmt.Sprintf(
			`<volume><name>%s-volume-0.qcow2</name><capacity unit="MiB">4</capacity><target><format type="qcow2"/></target></volume>`,
			name), 0)
		if err != nil {
			lv.Disconnect()
			return err
		}
		path, err := lv.StorageVolGetPath(vol)
		if err != nil {
			lv.Disconnect()
			return err
		}
		domain, err := lv.DomainDefineXML(fmt.Sprintf(`
<domain type="kvm">
  <name>%s</name>
  <memory unit="MiB">128</memory>
  <vcpu>1</vcpu>
  <os><type>hvm</type></os>
  <devices>
    <disk type="file" device="disk">
      <driver name="qemu" type="qcow2"/>
      <source file="%s"/>
      <target dev="vda" bus="virtio"/>
    </disk>
  </devices>
</domain>`, name, path))
		if err != nil {
			lv.Disconnect()
			return err
		}
		s.testHost = &testHostVirtDeploy{
			namespace: s.namespace, vm: virtdeploy.VirtDeployVMStatus{Name: name, Domain: domain}, lv: lv,
		}
		if _, err := lv.DomainCreateWithFlags(domain, uint32(libvirt.DomainStartPaused)); err != nil {
			return err
		}
		key, err := stormssh.GenerateKey()
		if err != nil {
			return err
		}
		s.sshPrivateKey = key.PEM
		return nil
	})
	r.RegisterTestCase("fail", func(tc storm.TestCase) error {
		tc.Fail(failureVMReason)
		return nil
	})
	return nil
}

func (s *failureVMScenario) Setup(storm.SetupCleanupContext) error { return nil }
func (s *failureVMScenario) Cleanup(ctx storm.SetupCleanupContext) error {
	if host, ok := s.testHost.(*testHostVirtDeploy); ok {
		hash := sha256.New()
		if err := host.WriteOSDisk(hash); err != nil {
			return err
		}
		if err := os.WriteFile(filepath.Join(s.dir, "source.sha256"), []byte(hex.EncodeToString(hash.Sum(nil))), 0600); err != nil {
			return err
		}
	}
	return s.TridentE2EScenario.Cleanup(ctx)
}

func runFailureVMChild() {
	dir := os.Getenv(failureVMDirEnv)
	s := &failureVMScenario{
		TridentE2EScenario: TridentE2EScenario{name: "capture-failure"},
		dir:                dir,
		namespace:          os.Getenv(failureVMNsEnv),
		volumeDir:          os.Getenv(failureVMPathEnv),
	}
	suite := storm.CreateSuite("failure-capture-test")
	suite.AddScenario(s)
	os.Args = []string{os.Args[0], "run", s.Name(), "-o", dir}
	suite.Run()
}
