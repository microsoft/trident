package ref

import "reflect"

func Of[E any](e E) *E {
	return &e
}

func IsNilInterface(v any) bool {
	if v == nil {
		return true
	}
	rv := reflect.ValueOf(v)
	switch rv.Kind() {
	case reflect.Ptr, reflect.Map, reflect.Slice, reflect.Func,
		reflect.Interface, reflect.Chan:
		return rv.IsNil()
	default:
		return false
	}
}
