package variants

import "errors"

type BuildVerity struct {
	Common CommonOpts `embed:""`
}

func (b *BuildVerity) Run() error {
	return errors.New("not implemented")
}

func (b *BuildVerity) CommonOpts() CommonOpts {
	return b.Common
}

func (b *BuildVerity) ExpectedImages() []ExpectedImage {
	return []ExpectedImage{}
}
