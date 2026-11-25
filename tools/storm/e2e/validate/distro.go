package validate

import (
	"tridenttools/storm/utils/env"
	"tridenttools/storm/utils/sshutils"

	"golang.org/x/crypto/ssh"
)

func ValidateAzureLinuxVersion(sa *SoftAsserter, client *ssh.Client, version string) {
	const check = "distro/identity"
	output, err := sshutils.RunCommand(client, "cat /etc/os-release")
	if err != nil {
		sa.Fail(check, err)
		return
	}
	if output.Status != 0 {
		sa.Failf(check, "reading os-release exited %d: %s", output.Status, output.Stderr)
		return
	}
	release, err := env.ParseOsReleaseString([]byte(output.Stdout))
	if err != nil {
		sa.Fail(check, err)
		return
	}
	sa.Assert(check, release.Id == "azurelinux" && release.VersionId == version,
		"expected azurelinux %s, got ID=%q VERSION_ID=%q", version, release.Id, release.VersionId)
}
