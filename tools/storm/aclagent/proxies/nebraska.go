package proxies

import (
	"bytes"
	"context"
	"database/sql"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/flatcar/nebraska/backend/pkg/api"
	"github.com/flatcar/nebraska/backend/pkg/api/admin"
	"github.com/flatcar/nebraska/backend/pkg/omaha"
	"gopkg.in/guregu/null.v4"
	"gopkg.in/yaml.v3"
)

// nebraskaTrack must match trident-acl-agent's DEFAULT_NEBRASKA_TRACK
// (crates/trident-acl-agent/src/lib.rs). Real Nebraska resolves a group by
// matching this string against the group's Track column, not its Name.
const nebraskaTrack = "west-us"

// defaultTeamID is the team seeded by Nebraska's own db migrations
// (0005_default_team_id.sql); application rows have a NOT NULL team_id FK.
const defaultTeamID = "d89342dc-9214-441d-a4af-bdd837a3b239"

// noUpdateVersion is a sentinel package version used when Scenario.Available
// is false. Leaving a channel with no package at all makes real Nebraska
// return ErrNoPackageFound, which maps to "error-noPackageFound" - a hard
// error trident-acl-agent's client treats as a failure, not "no update
// available". Seeding a package this far below any real image version
// instead exercises Nebraska's actual semver-comparison path in
// GetUpdatePackage and still cleanly yields "noupdate", since the
// instance's real reported version can never be lower than 0.0.1.
const noUpdateVersion = "0.0.1"

type NebraskaScenario struct {
	Available   bool   `yaml:"available"`
	Version     string `yaml:"version,omitempty"`
	URL         string `yaml:"url,omitempty"`
	SHA384      string `yaml:"sha384,omitempty"`
	PackageName string `yaml:"package-name,omitempty"`
}

func LoadNebraskaScenario(path string) (*NebraskaScenario, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read Nebraska scenario %s: %w", path, err)
	}
	var scenario NebraskaScenario
	if err := yaml.Unmarshal(data, &scenario); err != nil {
		return nil, fmt.Errorf("failed to parse Nebraska scenario yaml: %w", err)
	}
	return &scenario, nil
}

// NebraskaProxy wraps the real github.com/flatcar/nebraska/backend Omaha
// server, backed by an ephemeral Postgres container that this proxy manages
// itself, instead of a hand-rolled fake. This exercises trident-acl-agent
// against Nebraska's actual instance/event/update-grant state machine
// (RegisterInstance, RegisterEvent, GetUpdatePackage's in-progress gating
// and semver-driven grant/completion logic) rather than an approximation of
// it, at the cost of needing Docker plus a couple seconds of container
// startup/migration time per run.
type NebraskaProxy struct {
	Scenario *NebraskaScenario

	containerID string
	dbURL       string
	api         *api.API
	handler     *omaha.Handler
	appID       string
}

// ExpectedUpdateStatusSequence is the ordered sequence of Nebraska instance
// statuses trident-acl-agent's real update flow is expected to drive the
// seeded instance through end-to-end: GetUpdatePackage grants the update
// (UpdateGranted) on the first check, stage reports DownloadStarted/
// DownloadFinished (Downloading/Downloaded), finalize reports Installed
// (Installed), and the post-reboot commit reports the terminal Completed
// event (Complete). Pass to ValidateStatusHistory after a full run-ab-update
// scenario completes.
var ExpectedUpdateStatusSequence = []int{
	api.InstanceStatusUpdateGranted,
	api.InstanceStatusDownloading,
	api.InstanceStatusDownloaded,
	api.InstanceStatusInstalled,
	api.InstanceStatusComplete,
}

// AppID returns the DB-generated application ID trident-acl-agent must be
// configured with. Real Nebraska generates application IDs server-side
// (admin.Service.AddApp does not accept a caller-supplied ID), so callers
// must read this back after ListenAndServe seeds the app, rather than
// hardcoding an app_id string as the old fake mock allowed.
func (p *NebraskaProxy) AppID() string {
	return p.appID
}

func (p *NebraskaProxy) Handler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		defer r.Body.Close()
		ip := r.RemoteAddr
		if host, _, err := net.SplitHostPort(r.RemoteAddr); err == nil {
			ip = host
		}
		var buf bytes.Buffer
		if err := p.handler.Handle(r.Body, &buf, ip); err != nil {
			http.Error(w, fmt.Sprintf("nebraska omaha handler error: %v", err), http.StatusInternalServerError)
			return
		}
		w.Header().Set("Content-Type", "application/xml")
		_, _ = w.Write(buf.Bytes())
	})
}

// ListenAndServe starts an ephemeral Postgres container, applies Nebraska's
// db migrations against it, seeds an application/package/channel/group
// matching p.Scenario, and starts serving the real Nebraska Omaha handler on
// listenAddr. The Postgres container and http server are both torn down
// when ctx is cancelled.
func (p *NebraskaProxy) ListenAndServe(ctx context.Context, listenAddr string) (net.Listener, error) {
	dbURL, containerID, err := startEphemeralPostgres(ctx)
	if err != nil {
		return nil, fmt.Errorf("failed to start ephemeral Postgres for Nebraska: %w", err)
	}
	p.containerID = containerID
	p.dbURL = dbURL

	// api.New only reads NEBRASKA_DB_URL from the environment - there is no
	// functional option to set a custom DSN - so this is the only way to
	// point it at our ephemeral container. Safe here because exactly one
	// NebraskaProxy is ever instantiated per storm test process.
	if err := os.Setenv("NEBRASKA_DB_URL", dbURL); err != nil {
		stopEphemeralPostgres(containerID)
		return nil, fmt.Errorf("failed to set NEBRASKA_DB_URL: %w", err)
	}

	a, err := api.NewWithMigrations(api.OptionInitDB)
	if err != nil {
		stopEphemeralPostgres(containerID)
		return nil, fmt.Errorf("failed to initialize Nebraska API against ephemeral Postgres: %w", err)
	}
	p.api = a
	p.handler = omaha.NewHandler(a)

	if err := p.seed(); err != nil {
		stopEphemeralPostgres(containerID)
		return nil, fmt.Errorf("failed to seed Nebraska scenario: %w", err)
	}

	listener, err := net.Listen("tcp", listenAddr)
	if err != nil {
		stopEphemeralPostgres(containerID)
		return nil, fmt.Errorf("failed to listen on %s: %w", listenAddr, err)
	}
	server := &http.Server{Handler: p.Handler()}
	go func() {
		<-ctx.Done()
		_ = server.Shutdown(context.Background())
		stopEphemeralPostgres(containerID)
	}()
	go func() { _ = server.Serve(listener) }()
	return listener, nil
}

// seed creates the application/package/channel/group real Nebraska needs to
// answer update checks, per p.Scenario. The package version is the
// scenario's target version when an update should be offered, or the
// noUpdateVersion sentinel otherwise (see its doc comment).
func (p *NebraskaProxy) seed() error {
	svc := admin.NewService(p.api.Reads())

	app, err := svc.AddApp(&api.Application{
		Name:   "trident-acl-agent-storm-test",
		TeamID: defaultTeamID,
	})
	if err != nil {
		return fmt.Errorf("failed to seed application: %w", err)
	}
	p.appID = app.ID

	version := p.Scenario.Version
	if version == "" {
		version = "1.0.0"
	}
	if !p.Scenario.Available {
		version = noUpdateVersion
	}
	packageName := p.Scenario.PackageName
	if packageName == "" {
		packageName = "acl.cosi"
	}
	baseURL := p.Scenario.URL
	if baseURL == "" {
		baseURL = "https://example.invalid/images/"
	}

	// The package.hash column is varchar(64) (sized for base64 SHA1 or hex
	// SHA256, what real Nebraska/Omaha packages normally carry), but
	// Scenario.SHA384 is a 96-character hex string and would overflow it.
	// trident-acl-agent's Package wire struct doesn't even parse this field
	// (see crates/trident-acl-agent/src/nebraska/wire.rs) - image integrity
	// is checked via the COSI metadata instead - so it's safe to leave
	// unset here rather than truncate it into something misleading.
	pkg, err := svc.AddPackage(&api.Package{
		Type:          api.PkgTypeOther,
		URL:           baseURL,
		Version:       version,
		Filename:      null.StringFrom(packageName),
		ApplicationID: app.ID,
		Arch:          api.ArchAMD64,
	})
	if err != nil {
		return fmt.Errorf("failed to seed package: %w", err)
	}

	channel, err := svc.AddChannel(&api.Channel{
		Name:          "storm",
		ApplicationID: app.ID,
		PackageID:     null.StringFrom(pkg.ID),
		Arch:          api.ArchAMD64,
	})
	if err != nil {
		return fmt.Errorf("failed to seed channel: %w", err)
	}

	if _, err := svc.AddGroup(&api.Group{
		Name:                      "storm",
		ApplicationID:             app.ID,
		ChannelID:                 null.StringFrom(channel.ID),
		Track:                     nebraskaTrack,
		PolicyUpdatesEnabled:      true,
		PolicyPeriodInterval:      "15 minutes",
		PolicyMaxUpdatesPerPeriod: 100,
		PolicyUpdateTimeout:       "60 minutes",
	}); err != nil {
		return fmt.Errorf("failed to seed group: %w", err)
	}

	return nil
}

// StatusHistory returns every status the seeded application's instance(s)
// have transitioned through, in chronological order, read directly from
// Nebraska's instance_status_history table. There's no dbreads query for
// "history across all instances of an app" (only a single-instance one that
// takes an instance ID, which storm doesn't predict - it's derived from
// hashing the VM's /etc/machine-id, see crates/trident-acl-agent/src/lib.rs),
// so this queries the table by application_id directly instead, using a
// fresh connection to the same ephemeral Postgres container ListenAndServe
// started.
func (p *NebraskaProxy) StatusHistory() ([]int, error) {
	db, err := sql.Open("pgx", p.dbURL)
	if err != nil {
		return nil, fmt.Errorf("failed to open Nebraska db for status history query: %w", err)
	}
	defer db.Close()

	rows, err := db.Query(
		`select status from instance_status_history where application_id = $1 order by created_ts asc, id asc`,
		p.appID,
	)
	if err != nil {
		return nil, fmt.Errorf("failed to query instance_status_history: %w", err)
	}
	defer rows.Close()

	var statuses []int
	for rows.Next() {
		var status int
		if err := rows.Scan(&status); err != nil {
			return nil, fmt.Errorf("failed to scan instance_status_history row: %w", err)
		}
		statuses = append(statuses, status)
	}
	return statuses, rows.Err()
}

// ValidateStatusHistory asserts that the seeded application's real Nebraska
// instance_status_history exactly matches want, in order. Nebraska only ever
// appends a new history row when an instance's status actually changes (see
// updateInstanceData in the vendored api package), so this is a stable,
// duplicate-free ordering to assert against - it fails if trident-acl-agent
// skips a status transition, reports one out of order, or a status update
// silently gets rejected/ignored by Nebraska (e.g. sent while no update is
// in progress).
func (p *NebraskaProxy) ValidateStatusHistory(want []int) error {
	got, err := p.StatusHistory()
	if err != nil {
		return fmt.Errorf("failed to validate Nebraska instance status history: %w", err)
	}
	if len(got) != len(want) {
		return fmt.Errorf("unexpected Nebraska instance status history: want %s, got %s", formatStatuses(want), formatStatuses(got))
	}
	for i := range want {
		if got[i] != want[i] {
			return fmt.Errorf("unexpected Nebraska instance status history: want %s, got %s", formatStatuses(want), formatStatuses(got))
		}
	}
	return nil
}

// statusName renders a Nebraska instance status int using its api.InstanceStatus*
// name, for readable ValidateStatusHistory error messages.
func statusName(status int) string {
	switch status {
	case api.InstanceStatusUndefined:
		return "Undefined"
	case api.InstanceStatusUpdateGranted:
		return "UpdateGranted"
	case api.InstanceStatusError:
		return "Error"
	case api.InstanceStatusComplete:
		return "Complete"
	case api.InstanceStatusInstalled:
		return "Installed"
	case api.InstanceStatusDownloaded:
		return "Downloaded"
	case api.InstanceStatusDownloading:
		return "Downloading"
	case api.InstanceStatusOnHold:
		return "OnHold"
	default:
		return fmt.Sprintf("Unknown(%d)", status)
	}
}

func formatStatuses(statuses []int) string {
	names := make([]string, len(statuses))
	for i, s := range statuses {
		names[i] = statusName(s)
	}
	return "[" + strings.Join(names, " -> ") + "]"
}

// startEphemeralPostgres starts a disposable Postgres container for this
// Nebraska instance to use, waits for it to accept connections, and returns
// a connection URL for it plus its container ID (for teardown).
func startEphemeralPostgres(ctx context.Context) (dbURL string, containerID string, err error) {
	out, err := exec.CommandContext(ctx, "docker", "run", "-d", "--rm",
		"-e", "POSTGRES_PASSWORD=nebraska",
		"-e", "POSTGRES_DB=nebraska",
		"-p", "127.0.0.1::5432",
		"postgres:16-alpine",
	).Output()
	if err != nil {
		return "", "", fmt.Errorf("docker run postgres: %w", err)
	}
	containerID = strings.TrimSpace(string(out))

	portOut, err := exec.CommandContext(ctx, "docker", "port", containerID, "5432/tcp").Output()
	if err != nil {
		stopEphemeralPostgres(containerID)
		return "", "", fmt.Errorf("docker port: %w", err)
	}
	// docker port prints e.g. "127.0.0.1:32771" (or several such lines);
	// take the port from the last colon-separated field of the first line.
	firstLine := strings.SplitN(strings.TrimSpace(string(portOut)), "\n", 2)[0]
	fields := strings.Split(firstLine, ":")
	port := fields[len(fields)-1]

	dbURL = fmt.Sprintf("postgres://postgres:nebraska@127.0.0.1:%s/nebraska?sslmode=disable&connect_timeout=10", port)

	if err := waitForPostgres(ctx, dbURL); err != nil {
		stopEphemeralPostgres(containerID)
		return "", "", err
	}

	return dbURL, containerID, nil
}

// waitForPostgres polls dbURL until it accepts connections or ctx/deadline
// expires. Relies on the "pgx" driver already being registered with
// database/sql as a side effect of importing github.com/flatcar/nebraska's
// api package (which blank-imports github.com/jackc/pgx/v5/stdlib).
func waitForPostgres(ctx context.Context, dbURL string) error {
	deadline := time.Now().Add(30 * time.Second)
	var lastErr error
	for time.Now().Before(deadline) {
		if err := pingOnce(dbURL); err != nil {
			lastErr = err
		} else {
			return nil
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(500 * time.Millisecond):
		}
	}
	return fmt.Errorf("timed out waiting for ephemeral Postgres to accept connections: %w", lastErr)
}

func pingOnce(dbURL string) error {
	db, err := sql.Open("pgx", dbURL)
	if err != nil {
		return err
	}
	defer db.Close()
	return db.Ping()
}

func stopEphemeralPostgres(containerID string) {
	if containerID == "" {
		return
	}
	_ = exec.Command("docker", "rm", "-f", containerID).Run()
}
