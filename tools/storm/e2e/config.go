package e2e

type configs map[string]machineConfig

type hw struct {
	bm rt
	vm rt
}

type rt struct {
	host      pl
	container pl
}

type pl struct {
	pr_e2e bool
	ci     bool
	pre    bool
}

var (
	plOnlyPrerelease = pl{
		pr_e2e: true,
	}
)

var TestConfigs = configs{
	"base": {
		bm: rt{
			host:      plOnlyPrerelease,
			container: plOnlyPrerelease,
		},
	},
}
