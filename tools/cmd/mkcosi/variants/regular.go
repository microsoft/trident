package variants

type BuildRegular struct {
	Common CommonOpts `embed:""`
}

func (b *BuildRegular) Run() error {
	return buildCosiFile(b)
}

func (b *BuildRegular) CommonOpts() CommonOpts {
	return b.Common
}

func (b *BuildRegular) ExpectedImages() []ExpectedImage {
	return []ExpectedImage{
		{
			Name:       "esp.rawzst",
			PartType:   PartitionTypeEsp,
			MountPoint: "/boot/efi",
		},
		{
			Name:       "root.rawzst",
			PartType:   PartitionTypeRoot,
			MountPoint: "/",
		},
	}
}
