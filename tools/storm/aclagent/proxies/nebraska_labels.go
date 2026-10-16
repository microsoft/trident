package proxies

import (
	"bytes"
	"encoding/xml"
	"fmt"
	"io"
	"strings"
)

// labelCosiPackage adapts upstream Nebraska's hash-only package format without
// replacing its update-grant or event handling. Removing hash makes an
// accidental switch back to the agent's legacy source fail rather than pass.
func labelCosiPackage(response []byte, packageName string) ([]byte, error) {
	if packageName == "" {
		packageName = "acl.cosi"
	}
	decoder := xml.NewDecoder(bytes.NewReader(response))
	var buf bytes.Buffer
	encoder := xml.NewEncoder(&buf)
	var path []string
	packages, matched := 0, 0
	for {
		token, err := decoder.Token()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, fmt.Errorf("decode Omaha response: %w", err)
		}
		hash := ""
		switch element := token.(type) {
		case xml.StartElement:
			path = append(path, element.Name.Local)
			if strings.Join(path, "/") == "response/app/updatecheck/manifest/packages/package" {
				packages++
				name := ""
				for _, attr := range element.Attr {
					if attr.Name.Local == "name" {
						name = attr.Value
					}
				}
				if name == packageName {
					matched++
					attrs := make([]xml.Attr, 0, len(element.Attr))
					for _, attr := range element.Attr {
						if attr.Name.Local == "hash" {
							hash = attr.Value
						} else {
							attrs = append(attrs, attr)
						}
					}
					if hash == "" {
						return nil, fmt.Errorf("COSI package %q has no metadata hash to label", packageName)
					}
					element.Attr = attrs
					token = element
				}
			}
		case xml.EndElement:
			path = path[:len(path)-1]
		}
		if err := encoder.EncodeToken(token); err != nil {
			return nil, fmt.Errorf("encode Omaha response: %w", err)
		}
		if hash != "" {
			labels := struct {
				XMLName xml.Name `xml:"labels"`
				Label   struct {
					Key   string `xml:"key,attr"`
					Value string `xml:"value,attr"`
				} `xml:"label"`
			}{}
			labels.Label.Key = "cosi.metadata.sha384"
			labels.Label.Value = hash
			if err := encoder.Encode(labels); err != nil {
				return nil, fmt.Errorf("encode COSI metadata label: %w", err)
			}
		}
	}
	if packages == 0 {
		return response, nil
	}
	if matched != 1 {
		return nil, fmt.Errorf("expected one COSI package %q, got %d", packageName, matched)
	}
	if err := encoder.Flush(); err != nil {
		return nil, fmt.Errorf("flush Omaha response: %w", err)
	}
	return buf.Bytes(), nil
}
