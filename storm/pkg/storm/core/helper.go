package core

type Helper interface {
	Named
	Argumented
	// Run the helper
	Run(HelperContext) error
}

// BaseHelper is a partial implementation of the Helper interface. It is
// meant to be used for composition when not all methods of the Helper
// interface are needed. It does NOT provide a default implementation for the
// Name() and Run() methods.
type BaseHelper struct{}

func (h *BaseHelper) Args() any {
	return nil
}
