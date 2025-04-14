package core

import (
	"fmt"
	"regexp"

	"github.com/sirupsen/logrus"
)

// Basic name regex for storm entities
var NAME_REGEX = regexp.MustCompile("^[a-zA-Z0-9_-]+$")

type InvalidNameError struct {
	Name   string
	Entity string
}

func (e InvalidNameError) Error() string {
	return fmt.Sprintf("Invalid name '%s' for %s, only alphanumeric characters, dashes and underscores are allowed", e.Name, e.Entity)
}

func ValidateEntityName(name string, entity string) error {
	if !NAME_REGEX.MatchString(name) {
		return InvalidNameError{
			Name:   name,
			Entity: entity,
		}
	}

	return nil
}

type Named interface {
	// Returns the unique name of the entity
	Name() string
}

type Argumented interface {
	// Returns a pointer to an instance of a kong-annotated struct to parse
	// additional command line arguments into.
	Args() any
}

type LoggerProvider interface {
	// Logger returns the logger to be used for logging.
	Logger() *logrus.Logger
}
