package cmd

import (
	"fmt"
	"tridenttools/cmd/mkcosi/explainer"
)

type ExplainCmd struct {
	Source string `arg:"" help:"Path to the COSI file to read" type:"existingfile" required:""`
}

func (r *ExplainCmd) Run() error {
	err := explainer.ExplainCosiFile(r.Source)
	if err != nil {
		return fmt.Errorf("failed to explain COSI file: %w", err)
	}

	return nil
}
