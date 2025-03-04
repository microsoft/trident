package core

import "github.com/sirupsen/logrus"

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
