package variants

import (
	"argus_toolkit/cmd/mkcosi/metadata"
	"argus_toolkit/pkg/ref"
)

type BuildVerity struct {
	Common CommonOpts `embed:""`
}

func (b *BuildVerity) Run() error {
	return buildCosiFile(b)
}

func (b *BuildVerity) CommonOpts() CommonOpts {
	return b.Common
}

func (b *BuildVerity) ExpectedImages() []ExpectedImage {
	return []ExpectedImage{
		{
			Name:       "verity_esp",
			PartType:   metadata.PartitionTypeEsp,
			MountPoint: "/boot/efi",
		},
		{
			Name:        "verity_boot",
			PartType:    metadata.PartitionTypeXbootldr,
			MountPoint:  "/boot",
			GrubCfgPath: ref.Of("grub2/grub.cfg"),
		},
		{
			Name:            "verity_root",
			PartType:        metadata.PartitionTypeRoot,
			MountPoint:      "/",
			OsReleasePath:   ref.Of("etc/os-release"),
			VerityImageName: ref.Of("verity_roothash"),
		},
		{
			Name:                "verity_var",
			PartType:            metadata.PartitionTypeVar,
			MountPoint:          "/var",
			ContainsRpmDatabase: true,
		},
	}
}

func (b *BuildVerity) IsVerity() bool {
	return true
}
