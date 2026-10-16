package proxies

import (
	"bytes"
	"encoding/base64"
	"encoding/xml"
	"fmt"
	"io"
	"strings"

	"github.com/sirupsen/logrus"
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
	var metadataSHA384 []byte
	packages, matched := 0, 0
	packageStart, packageEnd, packageDepth := 0, 0, 0
	for {
		token, err := decoder.Token()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, fmt.Errorf("decode Omaha response: %w", err)
		}
		hash := ""
		packageEnded := false
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
			packageEnded = packageDepth != 0 && len(path) == packageDepth
			path = path[:len(path)-1]
		}
		if hash != "" {
			metadataSHA384, err = base64.StdEncoding.DecodeString(hash)
			if err != nil {
				return nil, fmt.Errorf("decode COSI metadata SHA-384: %w", err)
			}
			if len(metadataSHA384) != 48 {
				return nil, fmt.Errorf("COSI metadata SHA-384 must be 48 bytes, got %d", len(metadataSHA384))
			}
			if err := encoder.Flush(); err != nil {
				return nil, fmt.Errorf("flush Omaha response: %w", err)
			}
			packageStart = buf.Len()
			packageDepth = len(path)
		}
		if err := encoder.EncodeToken(token); err != nil {
			return nil, fmt.Errorf("encode Omaha response: %w", err)
		}
		if packageEnded {
			if err := encoder.Flush(); err != nil {
				return nil, fmt.Errorf("flush Omaha response: %w", err)
			}
			packageEnd = buf.Len()
			packageDepth = 0
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
	logrus.Infof("Nebraska COSI package XML: %s; metadata_sha384=%x", buf.Bytes()[packageStart:packageEnd], metadataSHA384)
	return buf.Bytes(), nil
}
