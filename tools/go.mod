module tridenttools

go 1.24

toolchain go1.24.2

replace storm => ../storm

// Deal with CVE-2024-45338, CVE-2025-22870, CVE-2025-22872
replace golang.org/x/net => golang.org/x/net v0.39.0

require (
	github.com/dustin/go-humanize v1.0.1
	github.com/Azure/azure-sdk-for-go/sdk/azidentity v1.10.1
	github.com/Azure/azure-sdk-for-go/sdk/storage/azblob v1.6.1
	github.com/bmc-toolbox/bmclib/v2 v2.0.1-0.20230530141715-da28e42c453f
	github.com/ddddddO/gtree v1.11.7
	github.com/fatih/color v1.18.0
	github.com/google/uuid v1.6.0
	github.com/pkg/errors v0.9.1
	github.com/pkg/sftp v1.13.9
	github.com/sirupsen/logrus v1.9.3
	github.com/spf13/cobra v1.8.1
	github.com/spf13/viper v1.19.0
	gopkg.in/yaml.v2 v2.4.0
	storm v1.0.0
)

require (
	github.com/Azure/azure-sdk-for-go/sdk/azcore v1.18.0 // indirect
	github.com/Azure/azure-sdk-for-go/sdk/internal v1.11.1 // indirect
	github.com/AzureAD/microsoft-authentication-library-for-go v1.4.2 // indirect
	github.com/goccy/go-yaml v1.17.1 // indirect
	github.com/golang-jwt/jwt/v5 v5.2.2 // indirect
	github.com/kylelemons/godebug v1.1.0 // indirect
	github.com/pkg/browser v0.0.0-20240102092130-5ac0b6a4141c // indirect
	golang.org/x/sync v0.14.0 // indirect
)

require (
	github.com/asaskevich/govalidator v0.0.0-20230301143203-a9d515a09cc2
	github.com/cavaliercoder/go-cpio v0.0.0-20180626203310-925f9528c45e
	github.com/jinzhu/copier v0.4.0
	github.com/juliangruber/go-intersect v1.1.0
	github.com/klauspost/pgzip v1.2.6
	github.com/muesli/crunchy v0.4.0
	github.com/ulikunitz/xz v0.5.12
	github.com/xrash/smetrics v0.0.0-20170218160415-a3153f7040e9 // indirect
	gonum.org/v1/gonum v0.16.0
)

require (
	github.com/alecthomas/kingpin/v2 v2.4.0
	github.com/alecthomas/units v0.0.0-20211218093645-b94a6e3cc137 // indirect
	github.com/bendahl/uinput v1.7.0
	github.com/davecgh/go-spew v1.1.2-0.20180830191138-d8f796af33cc // indirect
	github.com/digitalocean/go-libvirt v0.0.0-20250512231903-57024326652b
	github.com/gdamore/encoding v1.0.0 // indirect
	github.com/gdamore/tcell v1.4.0
	github.com/kr/fs v0.1.0 // indirect
	github.com/lucasb-eyer/go-colorful v1.2.0 // indirect
	github.com/mattn/go-runewidth v0.0.7 // indirect
	github.com/moby/sys/mountinfo v0.7.2
	github.com/pmezard/go-difflib v1.0.1-0.20181226105442-5d4384ee4fb2 // indirect
	github.com/rivo/tview v0.0.0-20200219135020-0ba8301b415c
	github.com/rivo/uniseg v0.4.7 // indirect
	github.com/stretchr/testify v1.10.0
	github.com/vishvananda/netns v0.0.4 // indirect
	github.com/xhit/go-str2duration/v2 v2.1.0 // indirect
	golang.org/x/term v0.32.0 // indirect
	libvirt.org/libvirt-go-xml v7.4.0+incompatible
)

require (
	github.com/VictorLowther/simplexml v0.0.0-20180716164440-0bff93621230 // indirect
	github.com/VictorLowther/soap v0.0.0-20150314151524-8e36fca84b22 // indirect
	github.com/alecthomas/kong v1.8.1
	github.com/bmc-toolbox/common v0.0.0-20240806132831-ba8adc6a35e3 // indirect
	github.com/fsnotify/fsnotify v1.7.0 // indirect
	github.com/go-logr/logr v1.4.2 // indirect
	github.com/hashicorp/errwrap v1.1.0 // indirect
	github.com/hashicorp/go-multierror v1.1.1 // indirect
	github.com/hashicorp/hcl v1.0.0 // indirect
	github.com/inconshreveable/mousetrap v1.1.0 // indirect
	github.com/jacobweinstock/iamt v0.0.0-20230502042727-d7cdbe67d9ef // indirect
	github.com/jacobweinstock/registrar v0.4.7 // indirect
	github.com/klauspost/compress v1.17.10
	github.com/magiconair/properties v1.8.7 // indirect
	github.com/mattn/go-colorable v0.1.13 // indirect
	github.com/mattn/go-isatty v0.0.20 // indirect
	github.com/mitchellh/mapstructure v1.5.0 // indirect
	github.com/pelletier/go-toml/v2 v2.2.4 // indirect
	github.com/sagikazarmark/locafero v0.6.0 // indirect
	github.com/sagikazarmark/slog-shim v0.1.0 // indirect
	github.com/satori/go.uuid v1.2.0 // indirect
	github.com/sourcegraph/conc v0.3.0 // indirect
	github.com/spf13/afero v1.11.0 // indirect
	github.com/spf13/cast v1.7.0 // indirect
	github.com/spf13/pflag v1.0.5 // indirect
	github.com/stmcginnis/gofish v0.19.0
	github.com/subosito/gotenv v1.6.0 // indirect
	github.com/vishvananda/netlink v1.3.0
	go.uber.org/multierr v1.11.0 // indirect
	golang.org/x/crypto v0.38.0
	golang.org/x/exp v0.0.0-20240823005443-9b4947da3948
	golang.org/x/net v0.40.0 // indirect
	golang.org/x/sys v0.33.0
	golang.org/x/text v0.25.0 // indirect
	gopkg.in/ini.v1 v1.67.0
	gopkg.in/yaml.v3 v3.0.1
)
