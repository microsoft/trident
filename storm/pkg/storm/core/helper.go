package core

type Helper interface {
	Argumented
	TestRegistrant
}

// BaseHelper is a partial implementation of the Helper interface. It is
// meant to be used for composition when not all methods of the Helper
// interface are needed. It does NOT provide a default implementation for the
// Name() and RegisterTestCases() methods.
type BaseHelper struct{}

func (h *BaseHelper) Args() any {
	return nil
}
