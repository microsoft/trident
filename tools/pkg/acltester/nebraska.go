package acltester

import (
	"context"
	"encoding/xml"
	"fmt"
	"net"
	"net/http"
	"os"

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

type NebraskaProxy struct {
	Scenario *NebraskaScenario
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
		appID := "test"
		if len(request.Apps) > 0 && request.Apps[0].AppID != "" {
			appID = request.Apps[0].AppID
		}
		response, err := p.Scenario.BuildResponse(appID)
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
	go func() {
		_ = server.Serve(listener)
	}()
	return listener, nil
}

func (s *NebraskaScenario) BuildResponse(appID string) ([]byte, error) {
	response := omahaResponse{
		XMLName:  xml.Name{Local: "response"},
		Protocol: "3.0",
		Server:   "tester",
		Daystart: daystart{ElapsedSeconds: 0},
		Apps: []omahaApp{{
			AppID:  appID,
			Status: "ok",
		}},
	}

	if !s.Available {
		response.Apps[0].UpdateCheck = &updateCheck{Status: "noupdate"}
	} else {
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
		response.Apps[0].UpdateCheck = &updateCheck{
			Status: "ok",
			URLs:   &urls{Entries: []urlEntry{{Codebase: baseURL}}},
			Manifest: &manifest{
				Version: version,
				Packages: &packages{Entries: []packageEntry{{
					Hash:     hash,
					Name:     packageName,
					Size:     1,
					Required: true,
				}}},
			},
		}
	}

	payload, err := xml.MarshalIndent(response, "", "  ")
	if err != nil {
		return nil, err
	}
	return append([]byte(xml.Header), payload...), nil
}

type omahaRequest struct {
	Apps []struct {
		AppID string `xml:"appid,attr"`
	} `xml:"app"`
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
	UpdateCheck *updateCheck `xml:"updatecheck,omitempty"`
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
