package virtdeploy

import "fmt"

type storageVolume struct {
	name   string
	path   string
	size   uint
	osDisk string
}
