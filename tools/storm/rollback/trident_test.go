package rollback

import (
	"testing"

	"github.com/alecthomas/kong"
)

func TestImageCustomizerContainerNameOption(t *testing.T) {
	for _, name := range []string{"", "trident-lifecycle-123-mic"} {
		t.Run("name="+name, func(t *testing.T) {
			var args TridentRollbackScenarioArgs
			parser, err := kong.New(&args)
			if err != nil {
				t.Fatal(err)
			}
			var flags []string
			if name != "" {
				flags = []string{"--image-customizer-container-name", name}
			}
			if _, err := parser.Parse(flags); err != nil {
				t.Fatal(err)
			}
			if args.ImageCustomizerContainerName != name {
				t.Fatalf("container name = %q, want %q", args.ImageCustomizerContainerName, name)
			}
		})
	}
}
