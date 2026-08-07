package proxies

import (
	"context"
	"encoding/xml"
	"fmt"
	"net"
	"net/http"
	"os"
	"sync"

	"gopkg.in/yaml.v3"
)

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

// instanceState is this fake server's per-machineid memory of an in-flight
// update, mirroring (in miniature) the piece of real Nebraska's instance
// state machine that trident-acl-agent's nebraska client actually depends
// on: once a progress event (Downloading/Downloaded/Installed) is received
// for a machineid, every update check for that machineid reports
// "error-updateInProgressOnInstance" until a terminal event (Complete or
// Error) is received. This is deliberately not a full re-implementation of
// Nebraska's state machine (no groups/channels/rollout rules, no other
// statuses) - just enough to exercise the request/response shapes
// trident-acl-agent's nebraska client actually sends and reads.
type instanceState struct {
	updateInProgress bool
}

type NebraskaProxy struct {
	Scenario *NebraskaScenario

	mu        sync.Mutex
	instances map[string]*instanceState
}

// instanceFor returns (creating if necessary) the tracked state for
// machineID. Must be called with p.mu held.
func (p *NebraskaProxy) instanceFor(machineID string) *instanceState {
	if p.instances == nil {
		p.instances = make(map[string]*instanceState)
	}
	inst, ok := p.instances[machineID]
	if !ok {
		inst = &instanceState{}
		p.instances[machineID] = inst
	}
	return inst
}

func (p *NebraskaProxy) Handler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		defer r.Body.Close()
		var request omahaRequest
		if err := xml.NewDecoder(r.Body).Decode(&request); err != nil {
			http.Error(w, fmt.Sprintf("failed to parse Omaha request: %v", err), http.StatusBadRequest)
			return
		}
		reqApp := omahaRequestApp{AppID: "test"}
		if len(request.Apps) > 0 {
			reqApp = request.Apps[0]
			if reqApp.AppID == "" {
				reqApp.AppID = "test"
			}
		}

		response, err := p.buildResponse(reqApp)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		w.Header().Set("Content-Type", "application/xml")
		_, _ = w.Write(response)
	})
}

func (p *NebraskaProxy) ListenAndServe(ctx context.Context, listenAddr string) (net.Listener, error) {
	listener, err := net.Listen("tcp", listenAddr)
	if err != nil {
		return nil, fmt.Errorf("failed to listen on %s: %w", listenAddr, err)
	}
	server := &http.Server{Handler: p.Handler()}
	go func() {
		<-ctx.Done()
		_ = server.Shutdown(context.Background())
	}()
	go func() { _ = server.Serve(listener) }()
	return listener, nil
}

// buildResponse handles a single parsed <app>: acknowledges any events
// (updating this machineid's tracked in-progress state per their whitelisted
// eventtype/eventresult pair, in the same order real Nebraska processes them
// - events before the update check, within one request/response) and then,
// if the request carried an <updatecheck>, answers it either with
// "error-updateInProgressOnInstance" (if an update is still in flight for
// this machineid) or the configured scenario's offer/noupdate.
func (p *NebraskaProxy) buildResponse(reqApp omahaRequestApp) ([]byte, error) {
	p.mu.Lock()
	defer p.mu.Unlock()

	inst := p.instanceFor(reqApp.MachineID)

	var eventAcks []eventAck
	for _, event := range reqApp.Events {
		// Nebraska acks every whitelisted event with status="ok" regardless
		// of which pair it is (see nebraska::event's module docs on the
		// client side) - it never reports an event as rejected over the
		// wire, even for a pair it silently discards. This fake only
		// receives events trident-acl-agent's client actually sends
		// (already whitelisted client-side), so unconditionally acking is
		// faithful enough here.
		eventAcks = append(eventAcks, eventAck{Status: "ok"})

		switch {
		case event.EventType == 3:
			// Terminal event (any of the three whitelisted eventresults):
			// clears update_in_progress and re-arms the instance so a
			// subsequent check can grant again.
			inst.updateInProgress = false
		case event.EventType == 13 || event.EventType == 14 || event.EventType == 800:
			// Progress event (Downloading/Downloaded/Installed): marks the
			// instance as mid-update.
			inst.updateInProgress = true
		}
	}

	app := omahaApp{AppID: reqApp.AppID, Status: "ok", Events: eventAcks}

	if reqApp.UpdateCheck != nil {
		if inst.updateInProgress {
			app.UpdateCheck = &updateCheck{Status: "error-updateInProgressOnInstance"}
		} else {
			app.UpdateCheck = p.Scenario.buildUpdateCheck()
		}
	}

	response := omahaResponse{
		XMLName:  xml.Name{Local: "response"},
		Protocol: "3.0",
		Server:   "tester",
		Daystart: daystart{ElapsedSeconds: 0},
		Apps:     []omahaApp{app},
	}
	payload, err := xml.MarshalIndent(response, "", "  ")
	if err != nil {
		return nil, err
	}
	return append([]byte(xml.Header), payload...), nil
}

// buildUpdateCheck builds the <updatecheck> element for a scenario with no
// in-flight update, per the configured Available/Version/URL/SHA384/PackageName.
func (s *NebraskaScenario) buildUpdateCheck() *updateCheck {
	if !s.Available {
		return &updateCheck{Status: "noupdate"}
	}
	version := s.Version
	if version == "" {
		version = "1.0.0"
	}
	packageName := s.PackageName
	if packageName == "" {
		packageName = "acl.cosi"
	}
	baseURL := s.URL
	if baseURL == "" {
		baseURL = "https://example.invalid/images/"
	}
	hash := s.SHA384
	if hash == "" {
		hash = "ignored"
	}
	return &updateCheck{
		Status:   "ok",
		URLs:     &urls{Entries: []urlEntry{{Codebase: baseURL}}},
		Manifest: &manifest{Version: version, Packages: &packages{Entries: []packageEntry{{Hash: hash, Name: packageName, Size: 1, Required: true}}}},
	}
}

type omahaRequest struct {
	Apps []omahaRequestApp `xml:"app"`
}
type omahaRequestApp struct {
	AppID           string              `xml:"appid,attr"`
	Version         string              `xml:"version,attr"`
	Track           string              `xml:"track,attr"`
	MachineID       string              `xml:"machineid,attr"`
	PreviousVersion string              `xml:"previousversion,attr"`
	Events          []omahaRequestEvent `xml:"event"`
	Ping            *struct{}           `xml:"ping"`
	UpdateCheck     *struct{}           `xml:"updatecheck"`
}
type omahaRequestEvent struct {
	EventType   int `xml:"eventtype,attr"`
	EventResult int `xml:"eventresult,attr"`
}
type omahaResponse struct {
	XMLName  xml.Name   `xml:"response"`
	Protocol string     `xml:"protocol,attr"`
	Server   string     `xml:"server,attr"`
	Daystart daystart   `xml:"daystart"`
	Apps     []omahaApp `xml:"app"`
}
type daystart struct {
	ElapsedSeconds int `xml:"elapsed_seconds,attr"`
}
type omahaApp struct {
	AppID       string       `xml:"appid,attr"`
	Status      string       `xml:"status,attr"`
	Events      []eventAck   `xml:"event,omitempty"`
	UpdateCheck *updateCheck `xml:"updatecheck,omitempty"`
}
type eventAck struct {
	Status string `xml:"status,attr"`
}
type updateCheck struct {
	Status   string    `xml:"status,attr"`
	URLs     *urls     `xml:"urls,omitempty"`
	Manifest *manifest `xml:"manifest,omitempty"`
}
type urls struct {
	Entries []urlEntry `xml:"url"`
}
type urlEntry struct {
	Codebase string `xml:"codebase,attr"`
}
type manifest struct {
	Version  string    `xml:"version,attr"`
	Packages *packages `xml:"packages,omitempty"`
}
type packages struct {
	Entries []packageEntry `xml:"package"`
}
type packageEntry struct {
	Hash     string `xml:"hash,attr,omitempty"`
	Name     string `xml:"name,attr"`
	Size     int    `xml:"size,attr"`
	Required bool   `xml:"required,attr"`
}
