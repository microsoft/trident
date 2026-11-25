package validate

import (
	"io"
	"net"
	"strings"
	"testing"
	"time"

	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/utils/sshutils"
	tridentutil "tridenttools/storm/utils/trident"

	"golang.org/x/crypto/ssh"
)

func TestValidateBaseUsesIndependentExpectations(t *testing.T) {
	expected, err := hostconfig.NewHostConfigFromYaml([]byte(`
storage:
  disks:
  - id: os
    partitions:
    - id: root
      size: grow
os:
  users:
  - name: alice
`))
	if err != nil {
		t.Fatal(err)
	}
	hs, err := tridentutil.NewHostStatusFromYaml([]byte(`
servicingState: provisioned
partitionPaths: {}
spec:
  storage:
    disks: []
  os:
    users: []
`))
	if err != nil {
		t.Fatal(err)
	}
	client := expectationTestClient(t, map[string]string{
		"sudo blkid":      "",
		"lsblk -J -b":     `{"blockdevices":[]}`,
		"cat /etc/passwd": "root:x:0:0:root:/root:/bin/sh\n",
		"cat /etc/group":  "root:x:0:\n",
	})
	var sa SoftAsserter
	ValidateBase(&sa, client, hs, expected, tridentutil.ServicingStateProvisioned, tridentutil.AbVolumeA)
	for _, check := range []string{"partitions/status-path", "partitions/system-present", "users/present"} {
		if !strings.Contains(sa.Summary(), "FAIL  "+check) {
			t.Fatalf("omitting expected state bypassed %s:\n%s", check, sa.Summary())
		}
	}
}

func TestValidateBaseRejectsMalformedReportedConfig(t *testing.T) {
	expected, err := hostconfig.NewHostConfigFromYaml([]byte("storage: {}\nos: {}\n"))
	if err != nil {
		t.Fatal(err)
	}
	for _, source := range []string{
		"servicingState: provisioned\n",
		"spec: invalid\n",
		"spec: {}\n",
		"spec: {storage: [], os: {}}\n",
		"spec: {storage: {}, os: null}\n",
	} {
		hs, err := tridentutil.NewHostStatusFromYaml([]byte(source))
		if err != nil {
			t.Fatal(err)
		}
		var sa SoftAsserter
		ValidateBase(&sa, nil, hs, expected, tridentutil.ServicingStateProvisioned, tridentutil.AbVolumeA)
		if !strings.Contains(sa.Summary(), "FAIL  host-status/") {
			t.Fatalf("malformed reported config was not rejected:\n%s", sa.Summary())
		}
	}
}

func expectationTestClient(t *testing.T, outputs map[string]string) *ssh.Client {
	t.Helper()
	key, _, err := sshutils.GenerateRsaKeyPair(2048)
	if err != nil {
		t.Fatal(err)
	}
	signer, err := ssh.ParsePrivateKey(key)
	if err != nil {
		t.Fatal(err)
	}
	config := &ssh.ServerConfig{NoClientAuth: true}
	config.AddHostKey(signer)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { listener.Close() })
	done := make(chan struct{})
	go func() {
		defer close(done)
		conn, err := listener.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		server, channels, requests, err := ssh.NewServerConn(conn, config)
		if err != nil {
			t.Error(err)
			return
		}
		defer server.Close()
		go ssh.DiscardRequests(requests)
		for incoming := range channels {
			channel, requests, err := incoming.Accept()
			if err != nil {
				t.Error(err)
				return
			}
			for request := range requests {
				if request.Type != "exec" {
					request.Reply(false, nil)
					continue
				}
				var command struct{ Command string }
				if err := ssh.Unmarshal(request.Payload, &command); err != nil {
					t.Error(err)
				}
				request.Reply(true, nil)
				output, ok := outputs[command.Command]
				var status uint32
				if !ok {
					t.Errorf("unexpected command: %s", command.Command)
					status = 1
				}
				io.WriteString(channel, output)
				channel.SendRequest("exit-status", false, ssh.Marshal(struct{ Status uint32 }{status}))
				channel.Close()
				break
			}
		}
	}()
	client, err := ssh.Dial("tcp", listener.Addr().String(), &ssh.ClientConfig{
		User:            "test",
		HostKeyCallback: ssh.InsecureIgnoreHostKey(),
		Timeout:         3 * time.Second,
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		client.Close()
		<-done
	})
	return client
}
