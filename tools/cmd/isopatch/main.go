/*
Copyright © 2023 Microsoft Corporation
*/
package main

import (
	"fmt"
	"os"
	"strings"
	"tridenttools/pkg/isopatcher"

	log "github.com/sirupsen/logrus"
	"github.com/spf13/cobra"
)

type patchSpec struct {
	sourceFile string
	destPath   string
}

var (
	isoFile    string
	outputFile string
	patchSpecs []string
)

var rootCmd = &cobra.Command{
	Use:   "isopatch",
	Short: "Patch ISO files with custom content",
	Long: `isopatch is a general-purpose utility for patching ISO images with
custom files. It uses a placeholder system to inject files into the ISO
without rebuilding it.

By default, patches files in-place. Use --output to create a new ISO.

Patch format: <source-file>:<destination-path>
  - source-file: Path to the file containing the content to inject
  - destination-path: Path within the ISO where the content should be placed

Example:
  isopatch --iso installer.iso \
    --patch config.yaml:/etc/trident/config.yaml \
    --patch script.sh:/trident_cdrom/pre-trident-script.sh
  
  # Create a new ISO instead of modifying in-place:
  isopatch --iso installer.iso --output patched.iso \
    --patch config.yaml:/etc/trident/config.yaml`,
	PreRun: func(cmd *cobra.Command, args []string) {
		log.SetLevel(log.DebugLevel)

		if len(isoFile) == 0 {
			log.Fatal("ISO file not specified (use --iso)")
		}

		// Check for patch requests
		if len(patchSpecs) == 0 {
			log.Fatal("No patch operations specified (use --patch)")
		}
	},
	Run: func(cmd *cobra.Command, args []string) {
		var patches []patchSpec
		for _, spec := range patchSpecs {
			parts := strings.SplitN(spec, ":", 2)
			if len(parts) != 2 {
				log.WithField("spec", spec).Fatal("Invalid patch format. Expected <source-file>:<destination-path>")
			}
			patches = append(patches, patchSpec{
				sourceFile: parts[0],
				destPath:   parts[1],
			})
		}

		// Check ISO
		log.WithField("file", isoFile).Info("Reading ISO file")
		iso, err := os.ReadFile(isoFile)
		if err != nil {
			log.WithError(err).Fatalf("failed to read ISO file")
		}

		// Apply patches
		patchCount := 0
		for _, patch := range patches {
			log.WithFields(log.Fields{
				"source": patch.sourceFile,
				"dest":   patch.destPath,
			}).Info("Applying patch")

			contents, err := os.ReadFile(patch.sourceFile)
			if err != nil {
				log.WithError(err).WithField("file", patch.sourceFile).Fatalf("failed to read source file")
			}

			err = isopatcher.PatchFile(iso, patch.destPath, contents)
			if err != nil {
				log.WithError(err).WithFields(log.Fields{
					"source": patch.sourceFile,
					"dest":   patch.destPath,
				}).Fatalf("failed to patch file into ISO")
			}

			patchCount++
			log.WithField("dest", patch.destPath).Info("Successfully patched")
		}

		// Determine output file (in-place if not specified)
		targetFile := isoFile
		if len(outputFile) != 0 {
			targetFile = outputFile
		}

		// Write patched ISO
		log.WithField("file", targetFile).Info("Writing patched ISO")
		err = os.WriteFile(targetFile, iso, 0644)
		if err != nil {
			log.WithError(err).Fatalf("failed to write patched ISO")
		}

		log.WithField("file", targetFile).Info("Successfully patched ISO")

		// Show changes
		fmt.Printf("\n✓ Applied %d patch(es) to ISO: %s\n", patchCount, targetFile)
		for _, patch := range patches {
			fmt.Printf("  %s → %s\n", patch.sourceFile, patch.destPath)
		}
	},
}

func init() {
	rootCmd.Flags().StringVarP(&isoFile, "iso", "i", "", "Input ISO file to patch")
	rootCmd.Flags().StringVarP(&outputFile, "output", "o", "", "Output ISO file (default: modify input in-place)")
	rootCmd.Flags().StringArrayVarP(&patchSpecs, "patch", "p", []string{}, "Patch specification: <source-file>:<destination-path>")

	rootCmd.MarkFlagRequired("iso")
	rootCmd.MarkFlagRequired("patch")
}

func main() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}
