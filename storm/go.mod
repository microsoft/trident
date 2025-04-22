module storm

go 1.23.5

require (
	github.com/alecthomas/kong v1.8.1
	github.com/fatih/color v1.18.0
	github.com/pkg/sftp v1.13.9
	github.com/sirupsen/logrus v1.9.3
	golang.org/x/crypto v0.37.0
	golang.org/x/term v0.31.0
	gopkg.in/yaml.v3 v3.0.1
)

require (
	github.com/kr/fs v0.1.0 // indirect
	github.com/mattn/go-colorable v0.1.13 // indirect
	github.com/mattn/go-isatty v0.0.20 // indirect
	golang.org/x/sys v0.32.0 // indirect
)

// Deal with CVE-2024-45338, CVE-2025-22870, CVE-2025-22872
replace golang.org/x/net => golang.org/x/net v0.39.0
