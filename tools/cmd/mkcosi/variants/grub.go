package variants

import (
	"fmt"
	"regexp"
)

func extractRoothash(grubcfg string) (string, error) {
	regex := regexp.MustCompile(`roothash=(\w+)`)
	matches := regex.FindSubmatch([]byte(grubcfg))
	if len(matches) != 2 {
		return "", fmt.Errorf("failed to extract roothash from grub.cfg")
	}

	return string(matches[1]), nil
}
