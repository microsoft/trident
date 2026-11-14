// Package storm_acr is the storm suite's ACR script set.
//
// It is separate from scripts/acr, which the legacy pytest suite calls with
// its own long-standing argument contract. Keeping them apart means a change
// made for this suite cannot break the pipeline that still gates main.
//
// The pipeline supplies only facts about the run; which artifacts a
// configuration hosts in ACR is decided in plan.go rather than by a shell
// branch in YAML.
package storm_acr

import (
	"context"
	"fmt"
	"path/filepath"

	"github.com/microsoft/storm/pkg/storm/core"
	"github.com/sirupsen/logrus"

	"tridenttools/storm/utils/acr"
)

type StormAcrScriptSet struct {
	Push   PushScript   `cmd:"" name:"storm-acr-push" help:"Push the artifacts a Trident configuration hosts in ACR"`
	Delete DeleteScript `cmd:"" name:"storm-acr-delete" help:"Delete the artifacts a Trident configuration hosted in ACR"`
}

// runArgs are the facts about a run that both commands need.
type runArgs struct {
	Config                string `required:"" help:"Trident configuration's name (e.g. 'extensions')"`
	DeploymentEnvironment string `required:"" help:"Deployment environment" enum:"virtualMachine,bareMetal"`
	RuntimeEnv            string `required:"" help:"Runtime environment (host or container)"`
	AcrName               string `required:"" help:"Azure Container Registry name"`
	BuildId               string `required:"" help:"Build ID"`
	SourceDir             string `required:"" help:"Trident source directory the artifacts were built into" type:"existingdir"`
	RefreshToken          string `required:"" env:"ACR_REFRESH_TOKEN" help:"ACR refresh token, from 'az acr login --expose-token'"`
}

type PushScript struct {
	runArgs
	TagVarName  string `required:"" help:"ADO variable name in which to store the images' tag base"`
	RepoVarName string `help:"ADO variable name in which to store the repository name"`
	UrlVarName  string `help:"ADO variable name in which to store the pushed image's OCI URL"`
}

type DeleteScript struct {
	runArgs
}

func (s *PushScript) Run(suite core.SuiteContext) error {
	plan, needed := planFor(s.Config, s.RuntimeEnv, s.SourceDir)
	if !needed {
		logrus.Infof("Configuration %q hosts no artifacts in ACR; nothing to push.", s.Config)
		return nil
	}

	client, err := acr.NewClient(s.AcrName, s.RefreshToken)
	if err != nil {
		return err
	}

	ctx := context.Background()
	tagBase := TagBase(s.BuildId, s.Config, s.DeploymentEnvironment)
	var firstRef string
	for i, path := range plan.Files {
		tag := fmt.Sprintf("%s.%d", tagBase, i+1)
		ref, err := client.Push(ctx, plan.Repository, tag, path)
		if err != nil {
			return fmt.Errorf("failed to push %s: %w", filepath.Base(path), err)
		}
		logrus.Infof("Pushed %s to %s", filepath.Base(path), ref)

		// Verify rather than trust the push: a tag that is absent afterwards
		// surfaces much later as an install failure on the target host.
		ok, err := client.Exists(ctx, plan.Repository, tag)
		if err != nil {
			return fmt.Errorf("failed to verify %s:%s: %w", plan.Repository, tag, err)
		}
		if !ok {
			return fmt.Errorf("%s:%s is absent immediately after being pushed", plan.Repository, tag)
		}
		if i == 0 {
			firstRef = ref
		}
	}

	if suite.AzureDevops() {
		setVar(s.TagVarName, tagBase)
		if plan.EmitRepo {
			setVar(s.RepoVarName, plan.Repository)
		}
		if plan.EmitUrl {
			setVar(s.UrlVarName, firstRef)
		}
	}
	return nil
}

func (s *DeleteScript) Run(core.SuiteContext) error {
	plan, needed := planFor(s.Config, s.RuntimeEnv, s.SourceDir)
	if !needed {
		logrus.Infof("Configuration %q hosts no artifacts in ACR; nothing to clean up.", s.Config)
		return nil
	}

	client, err := acr.NewClient(s.AcrName, s.RefreshToken)
	if err != nil {
		return err
	}

	ctx := context.Background()
	tagBase := TagBase(s.BuildId, s.Config, s.DeploymentEnvironment)

	// Delete every tag the push would have created, not every tag it did:
	// cleanup also runs after a run that died partway through pushing. An
	// absent tag is not an error.
	var errs []error
	for i := range plan.Files {
		tag := fmt.Sprintf("%s.%d", tagBase, i+1)
		if err := client.Delete(ctx, plan.Repository, tag); err != nil {
			errs = append(errs, err)
			continue
		}
		logrus.Infof("Deleted %s:%s (or it was already absent)", plan.Repository, tag)
	}
	if len(errs) > 0 {
		return fmt.Errorf("failed to delete %d image(s), first error: %w", len(errs), errs[0])
	}
	return nil
}

// setVar emits an Azure DevOps variable assignment. It must start at column
// zero, which is why this lives in a script rather than a test case: storm
// indents output captured from a case, and an indented command is silently
// ignored by the agent.
func setVar(name, value string) {
	if name == "" {
		return
	}
	fmt.Printf("##vso[task.setvariable variable=%s]%s\n", name, value)
}
