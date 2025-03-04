package utils

import "strings"

// A filter that matches strings.
type StringFilter struct {
	emptyIsAny bool
	contents   map[string]bool
}

func NewStringFilterFromSlice(slice []string) *StringFilter {
	contents := make(map[string]bool)
	for _, item := range slice {
		contents[item] = true
	}

	return &StringFilter{true, contents}
}

// Force the filter to match nothing if it is empty.
func (f *StringFilter) SetStrict() {
	f.emptyIsAny = false
}

func (f *StringFilter) Match(item string) bool {
	if len(f.contents) == 0 {
		return f.emptyIsAny
	}

	_, ok := f.contents[item]
	return ok
}

func (f *StringFilter) MatchAny(items []string) bool {
	if len(f.contents) == 0 {
		return f.emptyIsAny
	}

	for _, item := range items {
		if f.Match(item) {
			return true
		}
	}

	return false
}

// A filter that matches paths.
type PathFilter struct {
	emptyIsAny bool
	recursive  bool
	contents   map[string]bool
}

// Force the filter to match nothing if it is empty.
func (f *PathFilter) SetStrict() {
	f.emptyIsAny = false
}

func NewPathFilterFromSlice(slice []string, recursive bool) *PathFilter {
	contents := make(map[string]bool)
	for _, item := range slice {
		contents[item] = true
	}

	return &PathFilter{true, recursive, contents}
}

func (f *PathFilter) Match(item string) bool {
	if len(f.contents) == 0 {
		return f.emptyIsAny
	}

	if f.recursive {
		for path := range f.contents {
			if pathIsBase(path, item) {
				return true
			}
		}

		return false
	} else {
		_, ok := f.contents[item]
		return ok
	}
}

func (f *PathFilter) MatchAny(items []string) bool {
	if len(f.contents) == 0 {
		return f.emptyIsAny
	}

	for _, item := range items {
		if f.Match(item) {
			return true
		}
	}

	return false
}

// Returns whether `base` is a base of `path`.
//
// For example:
//
//	pathIsBase("a/b/c", "a/b/c/d/e") == true
//	pathIsBase("a/b/c", "a/b/c") == true
//	pathIsBase("a/b/c", "a/b") == false
//	pathIsBase("a/b/z", "a/b/c/d") == false
func pathIsBase(base, path string) bool {
	// First check as pure strings
	if !strings.HasPrefix(path, base) {
		// If the base is not a prefix, then it is not a base
		return false
	}

	baseComponents := strings.Split(base, "/")
	pathComponents := strings.Split(path, "/")

	for i, baseComponent := range baseComponents {
		if i >= len(pathComponents) {
			return false
		}

		// If the base component is not equal to the path component, then it is not a base
		if baseComponent != pathComponents[i] {
			return false
		}
	}

	return true
}
