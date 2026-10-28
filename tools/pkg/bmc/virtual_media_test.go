package bmc

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/bmc-toolbox/bmclib/v2"
)

func TestVirtualMediaResourceLocation(t *testing.T) {
	for _, location := range []string{"Managers", "Systems", "Neither"} {
		t.Run(location, func(t *testing.T) {
			const isoURL = "http://192.0.2.1/provision.iso"
			const sessionPath = "/redfish/v1/SessionService/Sessions/test"
			mediaPath := "/redfish/v1/" + location + "/1/VirtualMedia"
			insertions := make(chan string, 2)
			server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("Content-Type", "application/json")
				var body any
				switch r.URL.Path {
				case "/redfish/v1/":
					body = map[string]any{
						"RedfishVersion": "1.16.0",
						"Managers":       map[string]string{"@odata.id": "/redfish/v1/Managers"},
						"Systems":        map[string]string{"@odata.id": "/redfish/v1/Systems"},
						"SessionService": map[string]string{
							"@odata.id": "/redfish/v1/SessionService",
						},
						"Links": map[string]any{
							"Sessions": map[string]string{"@odata.id": "/redfish/v1/SessionService/Sessions"},
						},
					}
				case "/redfish/v1/SessionService":
					body = map[string]any{"Sessions": map[string]string{"@odata.id": "/redfish/v1/SessionService/Sessions"}}
				case "/redfish/v1/SessionService/Sessions":
					if r.Method != http.MethodPost {
						t.Errorf("unexpected session method: %s", r.Method)
					}
					w.Header().Set("Location", sessionPath)
					w.Header().Set("X-Auth-Token", "test-token")
					w.WriteHeader(http.StatusCreated)
					body = map[string]string{"@odata.id": sessionPath, "Id": "test"}
				case sessionPath:
					if r.Method == http.MethodDelete {
						w.WriteHeader(http.StatusNoContent)
						return
					}
					body = map[string]string{"@odata.id": sessionPath, "Id": "test"}
				case "/redfish/v1/Managers", "/redfish/v1/Systems", mediaPath:
					body = map[string]any{
						"Members":             []map[string]string{{"@odata.id": r.URL.Path + "/1"}},
						"Members@odata.count": 1,
					}
				case "/redfish/v1/Managers/1", "/redfish/v1/Systems/1":
					resource := map[string]any{"@odata.id": r.URL.Path, "Id": "1", "Name": "Test resource"}
					if strings.HasPrefix(r.URL.Path, "/redfish/v1/"+location+"/") {
						resource["VirtualMedia"] = map[string]string{"@odata.id": mediaPath}
					}
					body = resource
				case mediaPath + "/1":
					body = map[string]any{
						"@odata.id":  mediaPath + "/1",
						"Id":         "1",
						"MediaTypes": []string{"CD", "DVD"},
						"Actions": map[string]any{
							"#VirtualMedia.InsertMedia": map[string]string{
								"target": mediaPath + "/1/Actions/VirtualMedia.InsertMedia",
							},
						},
					}
				case mediaPath + "/1/Actions/VirtualMedia.InsertMedia":
					var payload struct {
						Image          string
						Inserted       bool
						WriteProtected bool
					}
					if r.Method != http.MethodPost || json.NewDecoder(r.Body).Decode(&payload) != nil ||
						payload.Image != isoURL || !payload.Inserted || !payload.WriteProtected {
						t.Errorf("unexpected InsertMedia request: %s %+v", r.Method, payload)
						w.WriteHeader(http.StatusBadRequest)
						return
					}
					insertions <- r.URL.Path
					w.WriteHeader(http.StatusNoContent)
					return
				default:
					t.Errorf("unexpected Redfish request: %s %s", r.Method, r.URL.Path)
					http.NotFound(w, r)
					return
				}
				if err := json.NewEncoder(w).Encode(body); err != nil {
					t.Errorf("encode Redfish response: %v", err)
				}
			}))
			defer server.Close()

			address, err := url.Parse(server.URL)
			if err != nil {
				t.Fatal(err)
			}
			client := bmclib.NewClient(address.Hostname(), "test", "test", bmclib.WithRedfishPort(address.Port()))
			client.Registry.Drivers = client.Registry.For("gofish")
			ctx := context.Background()
			if err := client.Open(ctx); err != nil {
				t.Fatal(err)
			}
			defer client.Close(ctx)

			ok, err := client.SetVirtualMedia(ctx, "CD", isoURL)
			if location == "Neither" {
				if err == nil || ok || len(insertions) != 0 {
					t.Fatalf("expected discovery failure without insertion, got ok=%v err=%v", ok, err)
				}
				return
			}
			if err != nil || !ok {
				t.Fatalf("insert ISO: ok=%v err=%v", ok, err)
			}
			if len(insertions) != 1 {
				t.Fatalf("expected one ISO insertion, got %d", len(insertions))
			}
		})
	}
}
