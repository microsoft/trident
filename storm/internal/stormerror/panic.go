package stormerror

import "fmt"

type PanicError struct {
	any
	Stack []byte
}

func NewPanicError(any any, stack []byte) PanicError {
	return PanicError{
		any:   any,
		Stack: stack,
	}
}

func (pe PanicError) Error() string {
	return fmt.Sprintf("panic occurred: %v", pe.any)
}
