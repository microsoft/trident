package builder

import (
	"tridenttools/cmd/mkcosi/metadata"
	"tridenttools/pkg/ref"
)

type BuildUbuntu struct {
	Common CommonOpts `embed:""`
}

func (b *BuildUbuntu) Run() error {
	return buildCosiFile(b)
}

func (b *BuildUbuntu) CommonOpts() CommonOpts {
	return b.Common
}

func (b *BuildUbuntu) IsVerity() bool {
	return false
}

func (b *BuildUbuntu) ExpectedImages() []ExpectedImage {
	return []ExpectedImage{
		{
			Name:       "esp",
			PartType:   metadata.PartitionTypeEsp,
			MountPoint: "/boot/efi",
		},
		{
			Name:        "boot",
			PartType:    metadata.PartitionTypeLinuxGeneric,
			MountPoint:  "/boot",
			GrubCfgPath: ref.Of("grub/grub.cfg"),
		},
		{
			Name:          "root",
			PartType:      metadata.PartitionTypeRoot,
			MountPoint:    "/",
			OsReleasePath: ref.Of("etc/os-release"),
		},
	}
}
