package virtdeploy

type cdrom struct {
	// Device path for the CDROM inside the VM
	device string
	// Optional path for a CDROM ISO on the host. If empty, a blank CDROM slot
	// will be created.
	path string
}
