package cmd

import (
	"fmt"
	"io"
	"os"
	"tridenttools/cmd/mkcosi/builder"
	"tridenttools/cmd/mkcosi/cosi"
	"tridenttools/cmd/mkcosi/vhd"

	log "github.com/sirupsen/logrus"
)

type AddVpcCmd struct {
	Source string `arg:"" help:"Path to the source COSI file." type:"existingfile" required:""`
	Output string `arg:"" help:"Path to write the new COSI file to." type:"path" required:""`
}

func (r *AddVpcCmd) Run() error {
	// Regenerate the COSI file to get a clean state.
	cosi, err := cosi.ReadCosiFile(r.Source)
	if err != nil {
		return fmt.Errorf("failed to read file: %w", err)
	}
	defer cosi.Close()

	log.WithField("source", r.Source).Info("Read COSI file")

	outFile, err := os.Create(r.Output)
	if err != nil {
		return fmt.Errorf("failed to create output file: %w", err)
	}
	defer outFile.Close()

	cw := &countWriter{Inner: outFile}

	err = builder.BuildCosi(cw, &cosi.Metadata)
	if err != nil {
		return fmt.Errorf("failed to build COSI file: %w", err)
	}

	// Once the COSI file is built, we know its final size.
	finalSize := cw.Count

	log.WithField("size", finalSize).Info("Adding VPC footer to COSI file")

	footer, err := vhd.CreateVpcFooter(finalSize)
	if err != nil {
		return fmt.Errorf("failed to create VPC footer: %w", err)
	}

	_, err = outFile.Write(footer[:])
	if err != nil {
		return fmt.Errorf("failed to write VPC footer: %w", err)
	}

	log.WithField("output", r.Output).Info("COSI file with VPC footer created successfully")

	return nil
}

type countWriter struct {
	Count uint64
	Inner io.Writer
}

func (cw *countWriter) Write(p []byte) (int, error) {
	n, err := cw.Inner.Write(p)
	cw.Count += uint64(n)
	return n, err
}
