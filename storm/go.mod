module storm

go 1.23.5

require (
	github.com/alecthomas/kong v1.8.1
	github.com/sirupsen/logrus v1.9.3
	gopkg.in/yaml.v3 v3.0.1
)

require (
	github.com/mattn/go-colorable v0.1.13 // indirect
	github.com/mattn/go-isatty v0.0.20 // indirect
)

require (
	github.com/fatih/color v1.18.0
	golang.org/x/crypto v0.31.0
	golang.org/x/sys v0.30.0 // indirect
	golang.org/x/term v0.29.0
)

// Deal with CVE-2024-45338
replace golang.org/x/net v0.21.0 => golang.org/x/net v0.33.0
