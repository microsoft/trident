package validate

import "testing"

func TestValidateAzureLinuxVersion(t *testing.T) {
	for _, tc := range []struct {
		name        string
		release     string
		wantFailure bool
	}{
		{"azl4", "ID=azurelinux\nVERSION_ID=\"4.0\"\n", false},
		{"wrong-version", "ID=azurelinux\nVERSION_ID=3.0\n", true},
		{"wrong-distro", "ID=fedora\nVERSION_ID=4.0\n", true},
		{"missing-identity", "NAME=Linux\n", true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			client := expectationTestClient(t, map[string]string{"cat /etc/os-release": tc.release})
			var sa SoftAsserter
			ValidateAzureLinuxVersion(&sa, client, "4.0")
			if sa.HasFailures() != tc.wantFailure {
				t.Fatalf("unexpected identity result: %s", sa.Summary())
			}
		})
	}
}
