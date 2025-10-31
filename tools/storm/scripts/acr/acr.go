package acr

import (
	"fmt"
	"os/exec"

	"github.com/sirupsen/logrus"
)

type AcrScriptSet struct {
	AcrPush   AcrPushScript   `cmd:"" help:"Pushes images to an ACR"`
	AcrDelete AcrDeleteScript `cmd:"" help:"Deletes images from an ACR"`
}

func generateTagBase(buildId string, config string, deploymentEnvironment string) string {
	return fmt.Sprintf("v%s.%s.%s", buildId, config, deploymentEnvironment)
}

func loginToACR(acrName string) error {
	logrus.Infof("Logging in to ACR: %s", acrName)
	cmd := exec.Command("az", "acr", "login", "-n", acrName)
	return cmd.Run()
}
