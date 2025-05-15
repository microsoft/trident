package builder

import (
	"tridenttools/cmd/mkcosi/metadata"
	"tridenttools/pkg/ref"
)

type BuildRegular struct {
	Common CommonOpts `embed:""`
}

func (b *BuildRegular) Run() error {
	return buildCosiFile(b)
}

func (b *BuildRegular) CommonOpts() CommonOpts {
	return b.Common
}

func (b *BuildRegular) IsVerity() bool {
	return false
}

func (b *BuildRegular) ExpectedImages() []ExpectedImage {
	return []ExpectedImage{
		{
			Name:       "esp",
			PartType:   metadata.PartitionTypeEsp,
			MountPoint: "/boot/efi",
		},
		{
			Name:                "root",
			PartType:            metadata.PartitionTypeRoot,
			MountPoint:          "/",
			OsReleasePath:       ref.Of("etc/os-release"),
			GrubCfgPath:         ref.Of("boot/grub2/grub.cfg"),
			ContainsRpmDatabase: true,
		},
	}
}
