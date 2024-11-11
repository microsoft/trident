package variants

import (
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
			Name:       "verity_esp.rawzst",
			PartType:   PartitionTypeEsp,
			MountPoint: "/boot/efi",
		},
		{
			Name:        "verity_boot.rawzst",
			PartType:    PartitionTypeXbootldr,
			MountPoint:  "/boot",
			GrubCfgPath: ref.Of("grub2/grub.cfg"),
		},
		{
			Name:            "verity_root.rawzst",
			PartType:        PartitionTypeRoot,
			MountPoint:      "/",
			OsReleasePath:   ref.Of("etc/os-release"),
			VerityImageName: ref.Of("verity_roothash.rawzst"),
		},
		{
			Name:                "verity_var.rawzst",
			PartType:            PartitionTypeVar,
			MountPoint:          "/var",
			ContainsRpmDatabase: true,
		},
	}
}

func (b *BuildVerity) IsVerity() bool {
	return true
}
