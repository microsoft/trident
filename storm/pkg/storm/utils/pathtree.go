package utils

import "strings"

type PathTree map[string]interface{}

func NewPathTree() PathTree {
	return make(map[string]interface{})
}

func (t PathTree) Add(path string) {
	segments := strings.Split(path, "/")
	current := t

	for _, segment := range segments {
		if _, ok := current[segment]; !ok {
			current[segment] = make(map[string]interface{})
		}
		current = current[segment].(map[string]interface{})
	}
}

func (t PathTree) Contains(path string) bool {
	segments := strings.Split(path, "/")
	current := t

	for _, segment := range segments {
		if _, ok := current[segment]; !ok {
			return false
		}
		current = current[segment].(map[string]interface{})
	}

	return true
}
